use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Instant,
};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;

use crate::{
    engines::{
        brush, colmap,
        ffmpeg::{
            extract_additional_frames, extract_selected_frames, extract_uniform_frames,
            validate_extraction,
        },
        ffprobe::probe_video,
        EngineKind, EnginePaths,
    },
    error::{Result, SplatError},
    pipeline::{
        effectiveness::{
            PlannerBridgeMetrics, PlannerBridgeStatus, PlannerEffectivenessTracker,
            PlannerFinalResultMetrics, PlannerFramePlanMetrics, PlannerGpuVendor,
            PlannerInitialReconstructionMetrics,
        },
        estimate::{
            estimate_calibrated_brush_stage_ms, estimate_calibrated_brush_stage_ms_for_images,
            estimate_runtime, estimate_runtime_for_images, RuntimeEstimate,
        },
        progress::stage_progress_range,
        runtime::{RuntimeSnapshot, RuntimeTracker},
        EventKind, EventLevel, PipelineEngine, PipelineEvent, PipelineStage,
    },
    planner::{
        bridge_trigger_ratio, plan_bridge_backfill, read_registered_source_indices,
        write_bridge_pair_list, BridgeBackfillStatus,
    },
    presets::{
        resolve_brush_training_preset, resolve_brush_training_preset_for_plan,
        resolve_planner_resolution_plan, Quality, ResolvedBrushTrainingPreset,
        PLANNER_RESOLUTION_POLICY_VERSION,
    },
    process::{ProcessManager, ProcessObserver, ProcessUpdate},
    project::{
        catalog, manager::atomic_replace_file, FrameState, PipelineStateFile,
        ProjectImportObserver, ProjectInputType, ProjectManager, ProjectMetadata, ProjectOutput,
        ProjectPaths, ProjectStatus, ReshootProvenance, ReshootState,
    },
    reconstruction::{
        colmap_model::{
            read_registered_images, read_single_camera, reusable_model_files, ColmapCamera,
        },
        ply::inspect_gaussian_ply,
        validator::{ReconstructionQuality, ReconstructionReport, ReconstructionValidator},
    },
    video::{
        prepare_scanned_image_sequence, scaled_video_dimensions, scan_image_sequence,
        validate_prepared_image_sequence, validate_reshoot_image_sequence, video_can_scale_to,
        FramePlan, FrameSelectionStrategy, ImagePreparationObserver, ImagePreparationPhase,
        ImageSequenceInfo, ImageSequenceNaming, PlannedFrame, QualityV2FrameSelection,
        UniformRatioFrameSelection, VideoInfo,
    },
};

pub struct PreparedFrames {
    pub input_type: ProjectInputType,
    pub video: Option<VideoInfo>,
    pub image_sequence: Option<ImageSequenceInfo>,
    pub plan: FramePlan,
    pub extracted_frames: u64,
    pub image_format: String,
    pub mask_count: u64,
    pub has_alpha: bool,
    pub working_width: u32,
    pub working_height: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineResult {
    pub project_id: String,
    pub project_path: PathBuf,
    pub final_ply: PathBuf,
    pub file_size: u64,
    pub splat_count: u64,
    pub input_images: u64,
    pub registered_images: u64,
    pub registered_ratio: f64,
    pub points_3d: u64,
    pub duration_ms: u64,
    pub completed_at: chrono::DateTime<Utc>,
    pub warning: Option<String>,
    pub logs_directory: PathBuf,
    #[serde(skip)]
    pub(crate) source_duration_seconds: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReshootSourceInfo {
    pub project_id: String,
    pub project_name: String,
    pub quality: Quality,
    pub planner_enabled: bool,
    pub camera_id: u32,
    pub camera_model: String,
    pub width: u64,
    pub height: u64,
    pub source_image_count: u64,
    pub eligible: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReshootInputType {
    Video,
    Images,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReshootInputInfo {
    pub input_type: ReshootInputType,
    pub image_count: Option<u64>,
    pub duration: Option<f64>,
    pub prepared_width: u32,
    pub prepared_height: u32,
    pub estimated_frames: u64,
    pub has_alpha: bool,
    pub mask_count: u64,
    pub compatible: bool,
    pub incompatibility_reason: Option<String>,
    pub estimate: RuntimeEstimate,
}

#[derive(Clone)]
struct EventSink {
    emit: Arc<dyn Fn(PipelineEvent) + Send + Sync>,
    sequence: Arc<AtomicU64>,
    last_progress_milli_percent: Arc<AtomicU64>,
    last_stage: Arc<std::sync::Mutex<Option<PipelineStage>>>,
    dispatch: Arc<std::sync::Mutex<()>>,
    started: Instant,
}

impl EventSink {
    #[allow(clippy::too_many_arguments)]
    fn send(
        &self,
        stage: PipelineStage,
        engine: Option<PipelineEngine>,
        kind: EventKind,
        level: EventLevel,
        stage_progress: Option<f32>,
        indeterminate: bool,
        message: impl Into<String>,
        current: Option<u64>,
        total: Option<u64>,
        unit: Option<&str>,
    ) {
        if !matches!(
            stage,
            PipelineStage::Completed | PipelineStage::Failed | PipelineStage::Cancelled
        ) {
            *self
                .last_stage
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(stage);
        }
        let (start, end) = stage_progress_range(stage);
        let progress = stage_progress
            .map(|value| start + (end - start) * value.clamp(0.0, 1.0))
            .unwrap_or(start);
        self.last_progress_milli_percent.fetch_max(
            (progress.max(0.0) * 1_000.0).round() as u64,
            Ordering::Relaxed,
        );
        let _dispatch = self
            .dispatch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (self.emit)(PipelineEvent {
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed) + 1,
            timestamp: Utc::now(),
            kind,
            level,
            stage,
            engine,
            progress,
            stage_progress: stage_progress.map(|value| value.clamp(0.0, 1.0) * 100.0),
            indeterminate,
            message: message.into(),
            current,
            total,
            unit: unit.map(str::to_owned),
            elapsed_ms: self.started.elapsed().as_millis() as u64,
            acceleration: None,
            runtime: None,
        });
    }

    fn stage(&self, stage: PipelineStage, progress: f32, message: impl Into<String>) {
        self.send(
            stage,
            Some(PipelineEngine::System),
            EventKind::Stage,
            EventLevel::Info,
            Some(progress),
            false,
            message,
            None,
            None,
            None,
        );
    }

    fn acceleration(&self, status: crate::engines::ColmapAccelerationStatus) {
        let _dispatch = self
            .dispatch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (self.emit)(PipelineEvent {
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed) + 1,
            timestamp: Utc::now(),
            kind: EventKind::Capability,
            level: if status.use_gpu() {
                EventLevel::Info
            } else {
                EventLevel::Warning
            },
            stage: PipelineStage::Created,
            engine: Some(PipelineEngine::Colmap),
            progress: 0.0,
            stage_progress: None,
            indeterminate: false,
            message: status.reason.clone(),
            current: None,
            total: None,
            unit: None,
            elapsed_ms: self.started.elapsed().as_millis() as u64,
            acceleration: Some(status),
            runtime: None,
        });
    }

    fn runtime(&self, stage: PipelineStage, engine: PipelineEngine, snapshot: RuntimeSnapshot) {
        let _dispatch = self
            .dispatch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (self.emit)(PipelineEvent {
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed) + 1,
            timestamp: Utc::now(),
            kind: EventKind::Runtime,
            level: EventLevel::Info,
            stage,
            engine: Some(engine),
            progress: self.last_progress_milli_percent.load(Ordering::Relaxed) as f32 / 1_000.0,
            stage_progress: None,
            indeterminate: true,
            message: String::new(),
            current: None,
            total: None,
            unit: None,
            elapsed_ms: self.started.elapsed().as_millis() as u64,
            acceleration: None,
            runtime: Some(snapshot),
        });
    }

    fn terminal(&self, error: &SplatError) {
        let cancelled = matches!(error, SplatError::Cancelled);
        let _dispatch = self
            .dispatch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (self.emit)(PipelineEvent {
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed) + 1,
            timestamp: Utc::now(),
            kind: EventKind::Stage,
            level: if cancelled {
                EventLevel::Warning
            } else {
                EventLevel::Error
            },
            stage: if cancelled {
                PipelineStage::Cancelled
            } else {
                PipelineStage::Failed
            },
            engine: Some(PipelineEngine::System),
            progress: self.last_progress_milli_percent.load(Ordering::Relaxed) as f32 / 1_000.0,
            stage_progress: None,
            indeterminate: false,
            message: error.to_string(),
            current: None,
            total: None,
            unit: None,
            elapsed_ms: self.started.elapsed().as_millis() as u64,
            acceleration: None,
            runtime: None,
        });
    }
}

#[derive(Debug, Clone)]
pub struct PipelineFailureContext {
    pub failed_stage: Option<PipelineStage>,
    pub project_id: Option<uuid::Uuid>,
    pub project_path: Option<PathBuf>,
    pub logs_directory: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct ActiveProjectContext {
    project_id: uuid::Uuid,
    workspace_task_id: Option<uuid::Uuid>,
    project_path: PathBuf,
    logs_directory: PathBuf,
}

pub struct PipelineRunner {
    engines: EnginePaths,
    process_manager: ProcessManager,
    events: EventSink,
    active_project: Arc<std::sync::Mutex<Option<ActiveProjectContext>>>,
    current_acceleration: Arc<std::sync::Mutex<Option<crate::engines::ColmapAccelerationStatus>>>,
    workspace_task_id: Option<uuid::Uuid>,
    planner_enabled: bool,
    effectiveness: Option<PlannerEffectivenessTracker>,
}

fn inherit_reshoot_planner_settings(source: &PipelineStateFile, target: &mut PipelineStateFile) {
    target.planner_enabled = source.planner_enabled;
    target.resolution_policy_version = source.resolution_policy_version;
}

fn reshoot_project_name(source_name: &str) -> String {
    format!("{source_name}_补拍")
}

impl PipelineRunner {
    pub fn new(engines: EnginePaths, emit: impl Fn(PipelineEvent) + Send + Sync + 'static) -> Self {
        Self::new_with_planner(engines, true, emit)
    }

    pub fn new_with_planner(
        engines: EnginePaths,
        planner_enabled: bool,
        emit: impl Fn(PipelineEvent) + Send + Sync + 'static,
    ) -> Self {
        Self {
            engines,
            process_manager: ProcessManager::new(),
            events: EventSink {
                emit: Arc::new(emit),
                sequence: Arc::new(AtomicU64::new(0)),
                last_progress_milli_percent: Arc::new(AtomicU64::new(0)),
                last_stage: Arc::new(std::sync::Mutex::new(None)),
                dispatch: Arc::new(std::sync::Mutex::new(())),
                started: Instant::now(),
            },
            active_project: Arc::new(std::sync::Mutex::new(None)),
            current_acceleration: Arc::new(std::sync::Mutex::new(None)),
            workspace_task_id: None,
            planner_enabled,
            effectiveness: None,
        }
    }

    pub fn with_effectiveness_tracker(mut self, tracker: PlannerEffectivenessTracker) -> Self {
        self.effectiveness = Some(tracker);
        self
    }

    pub fn with_workspace_task_id(mut self, workspace_task_id: Option<uuid::Uuid>) -> Self {
        self.workspace_task_id = workspace_task_id;
        self
    }

    pub fn current_project_identity(&self) -> Option<(uuid::Uuid, Option<uuid::Uuid>)> {
        self.active_project
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|project| (project.project_id, project.workspace_task_id))
    }

    pub fn cancel(&self) {
        self.process_manager.cancel();
    }

    pub fn emit_terminal(&self, error: &SplatError) {
        self.events.terminal(error);
    }

    pub fn failure_context(&self) -> PipelineFailureContext {
        let failed_stage = *self
            .events
            .last_stage
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let project = self
            .active_project
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        PipelineFailureContext {
            failed_stage,
            project_id: project.as_ref().map(|value| value.project_id),
            project_path: project.as_ref().map(|value| value.project_path.clone()),
            logs_directory: project.map(|value| value.logs_directory),
        }
    }

    pub fn current_acceleration(&self) -> Option<crate::engines::ColmapAccelerationStatus> {
        self.current_acceleration
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub async fn verify_pipeline_engines(
        &self,
    ) -> Result<crate::engines::ColmapAccelerationStatus> {
        let statuses = self.engines.check_all_for_task().await;
        for required in [
            EngineKind::Ffmpeg,
            EngineKind::Ffprobe,
            EngineKind::Colmap,
            EngineKind::Brush,
        ] {
            let status = statuses
                .iter()
                .find(|status| status.kind == required)
                .expect("all engine kinds returned");
            if !status.exists {
                return Err(SplatError::EngineMissing(status.path.display().to_string()));
            }
            if !status.can_start {
                return Err(SplatError::EngineStart {
                    engine: format!("{required:?}"),
                    detail: status.detail.clone(),
                });
            }
        }
        colmap::require_verified_cli(&self.engines.colmap)?;
        brush::require_verified_cli(&self.engines.brush)?;
        let acceleration = statuses
            .into_iter()
            .find(|status| status.kind == EngineKind::Colmap)
            .and_then(|status| status.acceleration)
            .ok_or_else(|| SplatError::UnsupportedEngine("无法确定 COLMAP 自动加速状态".into()))?;
        *self
            .current_acceleration
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(acceleration.clone());
        Ok(acceleration)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn prepare_frames(
        &self,
        input: &Path,
        quality: Quality,
        output: &Path,
        masks: &Path,
        logs: Option<&Path>,
        planner_enabled: bool,
        probed_video: Option<VideoInfo>,
        target_dimensions: Option<(u32, u32)>,
    ) -> Result<PreparedFrames> {
        self.events
            .stage(PipelineStage::ProbingVideo, 0.0, "正在读取视频信息");
        let video = match probed_video {
            Some(video) => video,
            None => {
                probe_video(
                    &self.engines.ffprobe,
                    input,
                    logs.map(|path| path.join("ffprobe.log")),
                    &self.process_manager,
                )
                .await?
            }
        };
        let probe_message = if video.has_alpha {
            format!(
                "视频 {:.1} 秒 · {:.2} FPS · {}×{} · 检测到 Alpha 通道（{}）",
                video.duration, video.fps, video.width, video.height, video.pixel_format
            )
        } else {
            format!(
                "视频 {:.1} 秒 · {:.2} FPS · {}×{}",
                video.duration, video.fps, video.width, video.height
            )
        };
        self.events
            .stage(PipelineStage::ProbingVideo, 1.0, probe_message);

        self.events
            .stage(PipelineStage::PlanningFrames, 0.0, "正在规划均匀抽帧");
        let plan = if planner_enabled {
            QualityV2FrameSelection.create_plan(&video, &quality.preset())
        } else {
            UniformRatioFrameSelection.create_plan(&video, &quality.preset())
        };
        self.events.stage(
            PipelineStage::PlanningFrames,
            1.0,
            format!(
                "预计提取 {} 帧（目标 {:.2} FPS，候选池 {} 帧 / 最高约 {:.2} FPS）",
                plan.estimated_frames,
                plan.sampling_fps,
                plan.candidate_frames.len(),
                plan.candidate_frames.len() as f64 / video.duration.max(f64::EPSILON)
            ),
        );

        self.events.stage(
            PipelineStage::ExtractingFrames,
            0.0,
            if video.has_alpha {
                "FFmpeg 正在同步提取透明 PNG 画面与 COLMAP Mask"
            } else {
                "FFmpeg 开始提取画面"
            },
        );
        let observer = self.process_observer(
            PipelineStage::ExtractingFrames,
            PipelineEngine::Ffmpeg,
            Some(plan.estimated_frames),
            ObserverMode::Ffmpeg,
        );
        let extraction = if planner_enabled {
            extract_selected_frames(
                &self.engines.ffmpeg,
                input,
                output,
                masks,
                &plan.selected_frames,
                video.has_alpha,
                target_dimensions,
                logs.map(|path| path.join("ffmpeg.log")),
                &self.process_manager,
                Some(observer),
            )
            .await?
        } else {
            extract_uniform_frames(
                &self.engines.ffmpeg,
                input,
                output,
                masks,
                &plan,
                video.has_alpha,
                target_dimensions,
                logs.map(|path| path.join("ffmpeg.log")),
                &self.process_manager,
                Some(observer),
            )
            .await?
        };
        if let Some(expected) = target_dimensions {
            if (extraction.width, extraction.height) != expected {
                return Err(SplatError::Process(format!(
                    "FFmpeg produced {}x{} frames; expected {}x{}",
                    extraction.width, extraction.height, expected.0, expected.1
                )));
            }
        }
        self.events.stage(
            PipelineStage::ExtractingFrames,
            1.0,
            if extraction.has_alpha {
                format!(
                    "已提取 {} 张透明 PNG 和 {} 张 Mask",
                    extraction.frame_count, extraction.mask_count
                )
            } else {
                format!("已提取 {} 帧", extraction.frame_count)
            },
        );
        Ok(PreparedFrames {
            input_type: ProjectInputType::Video,
            video: Some(video),
            image_sequence: None,
            plan,
            extracted_frames: extraction.frame_count,
            image_format: extraction.image_format.as_str().into(),
            mask_count: extraction.mask_count,
            has_alpha: extraction.has_alpha,
            working_width: extraction.width,
            working_height: extraction.height,
        })
    }

    pub async fn prepare_images(
        &self,
        input: &Path,
        quality: Quality,
        output: &Path,
        masks: &Path,
        probe_already_complete: bool,
    ) -> Result<PreparedFrames> {
        if !probe_already_complete {
            self.events
                .stage(PipelineStage::ProbingVideo, 0.0, "正在快速读取图片头");
        }
        let source = input.to_path_buf();
        let cancellation = self.process_manager.child_token();
        let scan =
            tokio::task::spawn_blocking(move || scan_image_sequence(&source, Some(&cancellation)))
                .await
                .map_err(|error| SplatError::Process(format!("图片序列分析任务失败：{error}")))??;
        let image_sequence = scan.info.clone();
        self.events.stage(
            PipelineStage::ProbingVideo,
            1.0,
            format!(
                "图片序列 {} 张 · {}×{}{}",
                image_sequence.image_count,
                image_sequence.width,
                image_sequence.height,
                if image_sequence.has_alpha {
                    " · 检测到 Alpha 通道"
                } else {
                    ""
                }
            ),
        );
        let plan = crate::video::create_image_plan(&image_sequence, &quality.preset());
        self.events.stage(
            PipelineStage::PlanningFrames,
            1.0,
            format!("将处理全部 {} 张图片", image_sequence.image_count),
        );
        self.events.stage(
            PipelineStage::ExtractingFrames,
            0.0,
            if image_sequence.has_alpha {
                "正在建立画面链接并检查 Alpha 通道"
            } else {
                "正在建立画面链接"
            },
        );
        let frames = output.to_path_buf();
        let mask_root = masks.to_path_buf();
        let events = self.events.clone();
        let observer: ImagePreparationObserver = Arc::new(move |progress| {
            let message = match progress.phase {
                ImagePreparationPhase::LinkingFrames => format!(
                    "正在建立画面链接或复制 {}/{} 张",
                    progress.current, progress.total
                ),
                ImagePreparationPhase::InspectingAlpha => format!(
                    "正在检测 Alpha 并生成 Mask {}/{} 张",
                    progress.current, progress.total
                ),
                ImagePreparationPhase::WritingOpaqueMasks => format!(
                    "正在补全不透明 Mask {}/{} 张",
                    progress.current, progress.total
                ),
                ImagePreparationPhase::Validating if progress.current == progress.total => {
                    "图片与 Mask 完整性校验完成".into()
                }
                ImagePreparationPhase::Validating => "正在校验图片与 Mask".into(),
            };
            events.send(
                PipelineStage::ExtractingFrames,
                Some(PipelineEngine::System),
                EventKind::Stage,
                EventLevel::Info,
                Some(progress.stage_progress),
                false,
                message,
                Some(progress.current),
                Some(progress.total),
                Some("images"),
            );
        });
        let cancellation = self.process_manager.child_token();
        let prepared = tokio::task::spawn_blocking(move || {
            prepare_scanned_image_sequence(
                scan,
                &frames,
                &mask_root,
                ImageSequenceNaming::Primary,
                Some(observer),
                Some(&cancellation),
            )
        })
        .await
        .map_err(|error| SplatError::Process(format!("图片序列准备任务失败：{error}")))??;
        self.events.stage(
            PipelineStage::ExtractingFrames,
            1.0,
            if prepared.has_alpha {
                format!(
                    "已准备 {} 张图片和 {} 张 Mask",
                    prepared.image_count, prepared.mask_count
                )
            } else {
                format!("已准备 {} 张图片", prepared.image_count)
            },
        );
        let working_width = image_sequence.width;
        let working_height = image_sequence.height;
        Ok(PreparedFrames {
            input_type: ProjectInputType::Images,
            video: None,
            image_sequence: Some(image_sequence),
            plan,
            extracted_frames: prepared.image_count,
            image_format: "images".into(),
            mask_count: prepared.mask_count,
            has_alpha: prepared.has_alpha,
            working_width,
            working_height,
        })
    }

    pub async fn generate(
        &self,
        input: &Path,
        quality: Quality,
        projects_root: &Path,
    ) -> Result<PipelineResult> {
        self.generate_with_manager(
            input,
            quality,
            ProjectManager::with_root(projects_root.to_path_buf()),
        )
        .await
    }

    pub async fn generate_for_diagnostics(
        &self,
        input: &Path,
        quality: Quality,
        projects_root: &Path,
    ) -> Result<PipelineResult> {
        self.generate_with_manager(
            input,
            quality,
            ProjectManager::for_diagnostics(projects_root.to_path_buf()),
        )
        .await
    }

    async fn generate_with_manager(
        &self,
        input: &Path,
        quality: Quality,
        project_manager: ProjectManager,
    ) -> Result<PipelineResult> {
        let acceleration = self.verify_pipeline_engines().await?;
        self.events.acceleration(acceleration.clone());
        let (paths, mut metadata) = if input.is_dir() {
            self.events
                .stage(PipelineStage::ProbingVideo, 0.0, "正在快速读取图片头");
            let events = self.events.clone();
            let observer: ProjectImportObserver = Arc::new(move |progress| {
                let ratio = if progress.total == 0 {
                    1.0
                } else {
                    progress.current as f32 / progress.total as f32
                };
                events.send(
                    PipelineStage::ProbingVideo,
                    Some(PipelineEngine::System),
                    EventKind::Stage,
                    EventLevel::Info,
                    Some(0.1 + 0.9 * ratio),
                    false,
                    format!("正在导入图片 {}/{} 张", progress.current, progress.total),
                    Some(progress.current),
                    Some(progress.total),
                    Some("images"),
                );
            });
            project_manager
                .create_with_progress(
                    input,
                    quality,
                    Some(observer),
                    Some(self.process_manager.child_token()),
                )
                .await?
        } else {
            project_manager.create(input, quality).await?
        };
        metadata.workspace_task_id = self.workspace_task_id;
        project_manager
            .write_metadata(&paths.metadata, &metadata)
            .await?;
        let mut state = project_manager.read_state(&paths.state).await?;
        state.planner_enabled = self.planner_enabled;
        state.resolution_policy_version = self
            .planner_enabled
            .then_some(PLANNER_RESOLUTION_POLICY_VERSION);
        project_manager.write_state(&paths.state, &state).await?;
        self.execute_project(project_manager, paths, &mut metadata, state, &acceleration)
            .await
    }

    /// 从已完成项目派生一个高清补拍项目。
    ///
    /// 源项目**只读**：它的输入画面与 final.ply 从不会被移动或覆盖。派生项目拥有自己的
    /// 画面目录，由"源项目的画面 + 用户提供的补拍素材"融合而成，随后重跑整条重建流水线。
    pub async fn inspect_reshoot_source(
        &self,
        project_id: uuid::Uuid,
    ) -> Result<ReshootSourceInfo> {
        let (project, metadata) = catalog::load_registered_project(project_id).await?;
        let mut info = ReshootSourceInfo {
            project_id: project_id.to_string(),
            project_name: metadata.name.clone(),
            quality: metadata.quality,
            planner_enabled: false,
            camera_id: 0,
            camera_model: String::new(),
            width: 0,
            height: 0,
            source_image_count: 0,
            eligible: false,
            reason: None,
        };
        if !catalog::project_is_durably_completed(&project, &metadata).await {
            info.reason = Some("只有已完成且结果完整的项目可以进行高清补拍".into());
            return Ok(info);
        }
        let source_state: PipelineStateFile =
            serde_json::from_slice(&tokio::fs::read(project.join("state.json")).await?)?;
        info.planner_enabled = source_state.planner_enabled;
        let frames = project.join("work/frames");
        let database = project.join("work/colmap/database.db");
        if !database.is_file()
            || std::fs::metadata(&database).map_or(true, |value| value.len() == 0)
        {
            info.reason = Some("原项目缺少相机重建数据库，无法进行增量补拍".into());
            return Ok(info);
        }
        let reconstruction =
            match reusable_reconstruction(&project, &frames, metadata.output.as_ref()).await {
                Ok(value) => value,
                Err(error) => {
                    info.reason = Some(format!("原项目的相机重建结果不可用：{error}"));
                    return Ok(info);
                }
            };
        let expected_registered_images = reconstruction.report.registered_images;
        let model = reconstruction.model;
        let database_for_validation = database.clone();
        let parsed = tokio::task::spawn_blocking(move || {
            let camera = read_single_camera(&model)?;
            let images = read_registered_images(&model)?;
            validate_database_camera(&database_for_validation, &camera)?;
            Ok::<_, SplatError>((camera, images))
        })
        .await
        .map_err(|error| SplatError::Process(format!("读取原项目相机信息失败：{error}")))??;
        if parsed.1.len() as u64 != expected_registered_images {
            info.reason = Some("原项目的注册图片记录与稀疏模型统计不一致".into());
            return Ok(info);
        }
        if parsed.1.iter().any(|image| image.camera_id != parsed.0.id) {
            info.reason = Some("原项目包含多个相机，暂时不能进行同相机增量补拍".into());
            return Ok(info);
        }
        info.camera_id = parsed.0.id;
        info.camera_model = parsed.0.model;
        info.width = parsed.0.width;
        info.height = parsed.0.height;
        info.source_image_count = parsed.1.len() as u64;
        info.eligible = true;
        Ok(info)
    }

    pub async fn probe_reshoot_input(
        &self,
        project_id: uuid::Uuid,
        input: &Path,
        input_type: ReshootInputType,
    ) -> Result<ReshootInputInfo> {
        let source = self.inspect_reshoot_source(project_id).await?;
        if !source.eligible {
            return Err(SplatError::Process(
                source
                    .reason
                    .unwrap_or_else(|| "原项目不能进行高清补拍".into()),
            ));
        }
        let (source_project, source_metadata) =
            catalog::load_registered_project(project_id).await?;
        ensure_reshoot_input_is_new(
            &source_project,
            &source_metadata.source_path,
            input,
            input_type,
        )
        .await?;
        let samples = catalog::runtime_samples().await;
        let (image_count, duration, width, height, estimated_frames, has_alpha, estimate) =
            match input_type {
                ReshootInputType::Images => {
                    let path = input.to_path_buf();
                    let images = tokio::task::spawn_blocking(move || {
                        crate::video::analyze_image_sequence(&path)
                    })
                    .await
                    .map_err(|error| SplatError::Process(format!("分析补拍图片失败：{error}")))??;
                    let plan = crate::video::create_image_plan(&images, &source.quality.preset());
                    let estimate = estimate_runtime_for_images(
                        source.source_image_count + images.image_count,
                        images.width.max(images.height),
                        &FramePlan {
                            retention_ratio: 1.0,
                            sampling_fps: 0.0,
                            estimated_frames: source.source_image_count + images.image_count,
                            ..FramePlan::default()
                        },
                        source.quality,
                        &samples,
                    );
                    (
                        Some(images.image_count),
                        None,
                        images.width,
                        images.height,
                        plan.estimated_frames,
                        images.has_alpha,
                        estimate,
                    )
                }
                ReshootInputType::Video => {
                    let video =
                        probe_video(&self.engines.ffprobe, input, None, &self.process_manager)
                            .await?;
                    let plan =
                        UniformRatioFrameSelection.create_plan(&video, &source.quality.preset());
                    let target_width = u32::try_from(source.width).unwrap_or(u32::MAX);
                    let target_height = u32::try_from(source.height).unwrap_or(u32::MAX);
                    let can_scale = video_can_scale_to(&video, target_width, target_height);
                    let dimensions = if can_scale {
                        (target_width, target_height)
                    } else {
                        scaled_video_dimensions(&video, target_width.max(target_height))
                    };
                    let estimate = estimate_runtime(
                        &video,
                        &FramePlan {
                            retention_ratio: plan.retention_ratio,
                            sampling_fps: plan.sampling_fps,
                            estimated_frames: source.source_image_count + plan.estimated_frames,
                            ..FramePlan::default()
                        },
                        source.quality,
                        &samples,
                    );
                    (
                        None,
                        Some(video.duration),
                        dimensions.0,
                        dimensions.1,
                        plan.estimated_frames,
                        video.has_alpha,
                        estimate,
                    )
                }
            };
        let compatible = u64::from(width) == source.width && u64::from(height) == source.height;
        Ok(ReshootInputInfo {
            input_type,
            image_count,
            duration,
            prepared_width: width,
            prepared_height: height,
            estimated_frames,
            has_alpha,
            mask_count: if has_alpha { estimated_frames } else { 0 },
            compatible,
            incompatibility_reason: (!compatible).then(|| {
                format!(
                    "补拍画面必须与原项目保持相同分辨率：需要 {}×{}，当前为 {}×{}",
                    source.width, source.height, width, height
                )
            }),
            estimate,
        })
    }

    pub async fn generate_incremental_reshoot(
        &self,
        source_project_id: uuid::Uuid,
        reshoot_input: &Path,
        input_type: ReshootInputType,
        projects_root: &Path,
    ) -> Result<PipelineResult> {
        let source = self.inspect_reshoot_source(source_project_id).await?;
        if !source.eligible {
            return Err(SplatError::Process(
                source
                    .reason
                    .unwrap_or_else(|| "原项目不能进行高清补拍".into()),
            ));
        }
        if input_type == ReshootInputType::Images {
            self.events
                .stage(PipelineStage::ProbingVideo, 0.0, "正在快速读取补拍图片头");
        }
        let input = self
            .probe_reshoot_input(source_project_id, reshoot_input, input_type)
            .await?;
        if !input.compatible {
            return Err(SplatError::Process(
                input
                    .incompatibility_reason
                    .unwrap_or_else(|| "补拍素材与原项目相机尺寸不一致".into()),
            ));
        }
        let acceleration = self.verify_pipeline_engines().await?;
        self.events.acceleration(acceleration.clone());
        let (source_root, source_metadata) =
            catalog::load_registered_project(source_project_id).await?;
        let source_state: PipelineStateFile =
            serde_json::from_slice(&tokio::fs::read(source_root.join("state.json")).await?)?;
        let project_manager = ProjectManager::with_root(projects_root.to_path_buf());
        let (paths, mut metadata) = if input_type == ReshootInputType::Images {
            let events = self.events.clone();
            let observer: ProjectImportObserver = Arc::new(move |progress| {
                let ratio = if progress.total == 0 {
                    1.0
                } else {
                    progress.current as f32 / progress.total as f32
                };
                events.send(
                    PipelineStage::ProbingVideo,
                    Some(PipelineEngine::System),
                    EventKind::Stage,
                    EventLevel::Info,
                    Some(0.1 + 0.9 * ratio),
                    false,
                    format!(
                        "正在导入补拍图片 {}/{} 张",
                        progress.current, progress.total
                    ),
                    Some(progress.current),
                    Some(progress.total),
                    Some("images"),
                );
            });
            project_manager
                .create_with_progress(
                    reshoot_input,
                    source_metadata.quality,
                    Some(observer),
                    Some(self.process_manager.child_token()),
                )
                .await?
        } else {
            project_manager
                .create(reshoot_input, source_metadata.quality)
                .await?
        };
        let stored_source = metadata.source_path.clone();
        metadata.name = reshoot_project_name(&source_metadata.name);
        metadata.workspace_task_id = self.workspace_task_id;
        metadata.transform = source_metadata.transform;
        metadata.editing = Default::default();
        metadata.reshoot = Some(ReshootProvenance {
            source_project_id,
            source_project_path: source_root.clone(),
            source_final_ply: source_root.join("final.ply"),
            reshoot_source_path: stored_source,
            camera_id: source.camera_id,
            camera_model: source.camera_model,
            width: source.width,
            height: source.height,
            has_alpha: input.has_alpha,
            mask_count: input.mask_count,
            source_image_count: source.source_image_count,
            reshoot_frame_count: input.estimated_frames,
            registered_reshoot_count: 0,
        });
        project_manager
            .write_metadata(&paths.metadata, &metadata)
            .await?;
        let mut state = project_manager.read_state(&paths.state).await?;
        inherit_reshoot_planner_settings(&source_state, &mut state);
        state.reshoot = Some(ReshootState {
            source_image_count: source.source_image_count,
            reshoot_frame_count: input.estimated_frames,
            mask_count: input.mask_count,
            has_alpha: input.has_alpha,
            ..Default::default()
        });
        project_manager.write_state(&paths.state, &state).await?;
        self.execute_project(project_manager, paths, &mut metadata, state, &acceleration)
            .await
    }

    pub async fn resume(&self, project_id: uuid::Uuid) -> Result<PipelineResult> {
        let acceleration = self.verify_pipeline_engines().await?;
        self.events.acceleration(acceleration.clone());
        let (project, mut metadata) = catalog::load_registered_project(project_id).await?;
        if catalog::project_is_durably_completed(&project, &metadata).await {
            return Err(SplatError::Process("该项目已经完成，无需继续".into()));
        }
        let source_available = match metadata.input_type {
            ProjectInputType::Video => metadata.source_path.is_file(),
            ProjectInputType::Images => metadata.source_path.is_dir(),
        };
        if !source_available {
            return Err(SplatError::Process("项目源素材缺失，无法继续".into()));
        }
        let paths = ProjectPaths::existing(project_id, project.clone());
        let project_manager = ProjectManager::with_root(
            project
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| project.clone()),
        );
        let state = project_manager.read_state(&paths.state).await?;
        if state.preset != metadata.quality {
            return Err(SplatError::Process(
                "项目档位与检查点不一致，无法安全继续".into(),
            ));
        }
        self.execute_project(project_manager, paths, &mut metadata, state, &acceleration)
            .await
    }

    async fn execute_project(
        &self,
        project_manager: ProjectManager,
        paths: ProjectPaths,
        metadata: &mut ProjectMetadata,
        state: PipelineStateFile,
        acceleration: &crate::engines::ColmapAccelerationStatus,
    ) -> Result<PipelineResult> {
        *self
            .active_project
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(ActiveProjectContext {
            project_id: paths.id,
            workspace_task_id: metadata.workspace_task_id,
            project_path: paths.project.clone(),
            logs_directory: paths.logs.clone(),
        });
        let started = Instant::now();
        let previous_duration = metadata.duration_ms.unwrap_or(0);
        metadata.status = ProjectStatus::Running;
        metadata.started_at = Some(Utc::now());
        metadata.completed_at = None;
        metadata.failure_message = None;
        project_manager
            .write_metadata(&paths.metadata, metadata)
            .await?;
        let result = if metadata.reshoot.is_some() {
            self.run_incremental_reshoot_project(
                &project_manager,
                &paths,
                metadata,
                state,
                acceleration,
            )
            .await
        } else {
            self.run_project(&project_manager, &paths, metadata, state, acceleration)
                .await
        };

        if let Err(error) = &result {
            let cancelled = matches!(error, SplatError::Cancelled);
            metadata.status = if cancelled {
                ProjectStatus::Cancelled
            } else {
                ProjectStatus::Failed
            };
            metadata.completed_at = Some(Utc::now());
            metadata.duration_ms =
                Some(previous_duration.saturating_add(started.elapsed().as_millis() as u64));
            metadata.failure_message = Some(error.to_string());
            let _ = project_manager
                .write_metadata(&paths.metadata, metadata)
                .await;
            let mut state = project_manager
                .read_state(&paths.state)
                .await
                .unwrap_or_else(|_| {
                    PipelineStateFile::created_for(metadata.quality, metadata.input_type)
                });
            state = mark_state_terminal(state, cancelled);
            let _ = project_manager.write_state(&paths.state, &state).await;
        }
        result
    }

    async fn run_incremental_reshoot_project(
        &self,
        project_manager: &ProjectManager,
        paths: &ProjectPaths,
        metadata: &mut ProjectMetadata,
        mut state: PipelineStateFile,
        acceleration: &crate::engines::ColmapAccelerationStatus,
    ) -> Result<PipelineResult> {
        let provenance = metadata
            .reshoot
            .clone()
            .ok_or_else(|| SplatError::Process("补拍项目缺少来源信息".into()))?;
        let mut checkpoint = state.reshoot.clone().unwrap_or_default();
        let source_root = &provenance.source_project_path;
        let source_frames = source_root.join("work/frames");
        let source_database = source_root.join("work/colmap/database.db");
        let source_metadata: ProjectMetadata =
            serde_json::from_slice(&tokio::fs::read(source_root.join("project.json")).await?)?;
        let source_reconstruction =
            reusable_reconstruction(source_root, &source_frames, source_metadata.output.as_ref())
                .await?;
        if !source_database.is_file()
            || std::fs::metadata(&source_database).map_or(true, |value| value.len() == 0)
        {
            return Err(SplatError::Process(
                "原项目缺少可复用的 COLMAP 数据库".into(),
            ));
        }
        let source_model_for_validation = source_reconstruction.model.clone();
        let source_database_for_validation = source_database.clone();
        tokio::task::spawn_blocking(move || {
            let camera = read_single_camera(&source_model_for_validation)?;
            validate_database_camera(&source_database_for_validation, &camera)
        })
        .await
        .map_err(|error| SplatError::Process(format!("校验补拍基线模型失败：{error}")))??;
        let source_model_files = reusable_model_files(&source_reconstruction.model)?;
        let base_model = paths.colmap.join("base-model");
        let database = paths.colmap.join("database.db");
        let base_database = paths.colmap.join("base-database.db");
        let reshoot_masks = paths.work.join("reshoot-masks");
        let reshoot_list = paths.colmap.join("reshoot-images.txt");
        let mapper_list = paths.colmap.join("mapper-images.txt");

        let snapshot_frames_valid = count_image_files(&paths.frames)
            .await
            .is_ok_and(|count| count >= provenance.source_image_count);
        let snapshot_database_valid = tokio::fs::metadata(&database)
            .await
            .is_ok_and(|value| value.is_file() && value.len() > 0);
        let base_database_valid = tokio::fs::metadata(&base_database)
            .await
            .is_ok_and(|value| value.is_file() && value.len() > 0);
        if !checkpoint.source_snapshot_complete
            || !snapshot_frames_valid
            || !snapshot_database_valid
            || !base_database_valid
            || !model_snapshot_matches(&source_model_files, &base_model)
        {
            self.events.stage(
                PipelineStage::ExtractingFrames,
                0.0,
                "正在复用原项目的相机与重建结果",
            );
            reset_directory(&paths.frames).await?;
            reset_directory(&paths.colmap).await?;
            reset_directory(&reshoot_masks).await?;
            copy_image_files(&source_frames, &paths.frames).await?;
            tokio::fs::copy(&source_database, &base_database).await?;
            tokio::fs::copy(&base_database, &database).await?;
            tokio::fs::create_dir_all(&base_model).await?;
            for source in &source_model_files {
                let name = source.file_name().expect("model file has name");
                tokio::fs::copy(&source, base_model.join(name)).await?;
            }
            checkpoint = ReshootState {
                source_snapshot_complete: true,
                source_image_count: provenance.source_image_count,
                reshoot_frame_count: provenance.reshoot_frame_count,
                mask_count: provenance.mask_count,
                has_alpha: provenance.has_alpha,
                ..Default::default()
            };
            state.reshoot = Some(checkpoint.clone());
            project_manager.write_state(&paths.state, &state).await?;
        }

        if checkpoint.supplemental_frames_complete
            && validate_reshoot_image_sequence(
                &paths.frames,
                &reshoot_masks,
                checkpoint.reshoot_frame_count,
                checkpoint.has_alpha,
            )
            .is_err()
        {
            checkpoint.supplemental_frames_complete = false;
            checkpoint.supplemental_features_complete = false;
            checkpoint.incremental_matching_complete = false;
            checkpoint.incremental_reconstruction_complete = false;
            state.brush_complete = false;
        }
        let prepared = if checkpoint.supplemental_frames_complete {
            validate_reshoot_image_sequence(
                &paths.frames,
                &reshoot_masks,
                checkpoint.reshoot_frame_count,
                checkpoint.has_alpha,
            )?
        } else {
            remove_reshoot_images(&paths.frames).await?;
            reset_directory(&reshoot_masks).await?;
            self.events.stage(
                PipelineStage::ExtractingFrames,
                0.0,
                if provenance.has_alpha {
                    "正在检查补拍 Alpha 通道并按需生成 Mask"
                } else {
                    "正在准备补拍画面"
                },
            );
            let prepared = match metadata.input_type {
                ProjectInputType::Images => {
                    let source = metadata.source_path.clone();
                    let cancellation = self.process_manager.child_token();
                    let scan = tokio::task::spawn_blocking(move || {
                        scan_image_sequence(&source, Some(&cancellation))
                    })
                    .await
                    .map_err(|error| SplatError::Process(format!("补拍图片扫描失败：{error}")))??;
                    let frames = paths.frames.clone();
                    let masks = reshoot_masks.clone();
                    let events = self.events.clone();
                    let observer: ImagePreparationObserver = Arc::new(move |progress| {
                        let message = match progress.phase {
                            ImagePreparationPhase::LinkingFrames => format!(
                                "正在建立补拍画面链接或复制 {}/{} 张",
                                progress.current, progress.total
                            ),
                            ImagePreparationPhase::InspectingAlpha => format!(
                                "正在检测补拍 Alpha 并生成 Mask {}/{} 张",
                                progress.current, progress.total
                            ),
                            ImagePreparationPhase::WritingOpaqueMasks => format!(
                                "正在补全补拍不透明 Mask {}/{} 张",
                                progress.current, progress.total
                            ),
                            ImagePreparationPhase::Validating
                                if progress.current == progress.total =>
                            {
                                "补拍图片与 Mask 完整性校验完成".into()
                            }
                            ImagePreparationPhase::Validating => "正在校验补拍图片与 Mask".into(),
                        };
                        events.send(
                            PipelineStage::ExtractingFrames,
                            Some(PipelineEngine::System),
                            EventKind::Stage,
                            EventLevel::Info,
                            Some(progress.stage_progress),
                            false,
                            message,
                            Some(progress.current),
                            Some(progress.total),
                            Some("images"),
                        );
                    });
                    let cancellation = self.process_manager.child_token();
                    tokio::task::spawn_blocking(move || {
                        prepare_scanned_image_sequence(
                            scan,
                            &frames,
                            &masks,
                            ImageSequenceNaming::Reshoot,
                            Some(observer),
                            Some(&cancellation),
                        )
                    })
                    .await
                    .map_err(|error| SplatError::Process(format!("补拍图片准备失败：{error}")))??
                }
                ProjectInputType::Video => {
                    let temporary = paths.work.join("reshoot-extract");
                    let temporary_masks = paths.work.join("reshoot-extract-masks");
                    reset_directory(&temporary).await?;
                    reset_directory(&temporary_masks).await?;
                    let supplemental_video = probe_video(
                        &self.engines.ffprobe,
                        &metadata.source_path,
                        Some(paths.logs.join("ffprobe.log")),
                        &self.process_manager,
                    )
                    .await?;
                    let target_width = u32::try_from(provenance.width).map_err(|_| {
                        SplatError::Process("Reshoot target width exceeds supported range".into())
                    })?;
                    let target_height = u32::try_from(provenance.height).map_err(|_| {
                        SplatError::Process("Reshoot target height exceeds supported range".into())
                    })?;
                    if !video_can_scale_to(&supplemental_video, target_width, target_height) {
                        return Err(SplatError::Process(format!(
                            "Reshoot video cannot be scaled to the source camera resolution {}x{} without upscaling or changing aspect ratio",
                            target_width, target_height
                        )));
                    }
                    let extracted = self
                        .prepare_frames(
                            &metadata.source_path,
                            metadata.quality,
                            &temporary,
                            &temporary_masks,
                            Some(&paths.logs),
                            state.planner_enabled,
                            Some(supplemental_video),
                            Some((target_width, target_height)),
                        )
                        .await?;
                    move_extracted_reshoot(
                        &temporary,
                        &temporary_masks,
                        &paths.frames,
                        &reshoot_masks,
                        extracted.has_alpha,
                    )
                    .await?;
                    validate_reshoot_image_sequence(
                        &paths.frames,
                        &reshoot_masks,
                        extracted.extracted_frames,
                        extracted.has_alpha,
                    )?
                }
            };
            validate_image_dimensions(
                &paths.frames,
                "reshoot_",
                provenance.width,
                provenance.height,
            )
            .await?;
            checkpoint.supplemental_frames_complete = true;
            checkpoint.reshoot_frame_count = prepared.image_count;
            checkpoint.mask_count = prepared.mask_count;
            checkpoint.has_alpha = prepared.has_alpha;
            if let Some(reshoot) = metadata.reshoot.as_mut() {
                reshoot.reshoot_frame_count = prepared.image_count;
                reshoot.mask_count = prepared.mask_count;
                reshoot.has_alpha = prepared.has_alpha;
            }
            state.stage = PipelineStage::ExtractingFrames;
            state.reshoot = Some(checkpoint.clone());
            project_manager.write_state(&paths.state, &state).await?;
            project_manager
                .write_metadata(&paths.metadata, metadata)
                .await?;
            self.events.send(
                PipelineStage::ExtractingFrames,
                Some(PipelineEngine::System),
                EventKind::Progress,
                EventLevel::Info,
                Some(1.0),
                false,
                format!("补拍画面准备完成 · {} 张", prepared.image_count),
                Some(prepared.image_count),
                Some(prepared.image_count),
                Some("images"),
            );
            prepared
        };
        let reshoot_names = image_names_with_prefix(&paths.frames, "reshoot_").await?;
        write_image_list(&reshoot_list, &reshoot_names).await?;

        let backend_label = if acceleration.use_gpu() { "GPU" } else { "CPU" };
        if !checkpoint.supplemental_features_complete {
            tokio::fs::copy(&base_database, &database).await?;
            self.events.stage(
                PipelineStage::ExtractingFeatures,
                0.0,
                format!("正在使用 {backend_label} 提取补拍画面的信息"),
            );
            colmap::extract_incremental_features(
                &self.engines.colmap,
                &database,
                Path::new("../frames"),
                Path::new("reshoot-images.txt"),
                provenance.camera_id,
                prepared.has_alpha.then_some(Path::new("../reshoot-masks")),
                paths.logs.join("colmap.log"),
                &self.process_manager,
                Some(self.process_observer(
                    PipelineStage::ExtractingFeatures,
                    PipelineEngine::Colmap,
                    Some(prepared.image_count),
                    ObserverMode::BracketProgress,
                )),
                acceleration.gpu_index(),
            )
            .await?;
            checkpoint.supplemental_features_complete = true;
            state.stage = PipelineStage::ExtractingFeatures;
            state.reshoot = Some(checkpoint.clone());
            project_manager.write_state(&paths.state, &state).await?;
        }

        if !checkpoint.incremental_matching_complete {
            self.events
                .stage(PipelineStage::Matching, 0.0, "正在补充新旧画面之间的联系");
            colmap::match_exhaustive(
                &self.engines.colmap,
                &database,
                paths.logs.join("colmap.log"),
                &self.process_manager,
                Some(self.process_observer(
                    PipelineStage::Matching,
                    PipelineEngine::Colmap,
                    Some(prepared.image_count),
                    ObserverMode::Matching,
                )),
                acceleration.gpu_index(),
            )
            .await?;
            self.events.send(
                PipelineStage::Matching,
                Some(PipelineEngine::Colmap),
                EventKind::Progress,
                EventLevel::Info,
                Some(1.0),
                false,
                "补拍图像匹配完成",
                None,
                None,
                None,
            );
            checkpoint.incremental_matching_complete = true;
            state.stage = PipelineStage::Matching;
            state.reshoot = Some(checkpoint.clone());
            project_manager.write_state(&paths.state, &state).await?;
        }

        let sparse = paths.colmap.join("sparse");
        if checkpoint.incremental_reconstruction_complete
            && best_sparse_model(&paths.frames, &sparse).await.is_err()
        {
            checkpoint.incremental_reconstruction_complete = false;
            state.brush_complete = false;
        }
        if !checkpoint.incremental_reconstruction_complete {
            let base = base_model.clone();
            let original_names = tokio::task::spawn_blocking(move || read_registered_images(&base))
                .await
                .map_err(|error| SplatError::Process(format!("读取原项目画面列表失败：{error}")))??
                .into_iter()
                .map(|image| image.name)
                .collect::<Vec<_>>();
            let mut mapper_names = original_names;
            mapper_names.extend(reshoot_names.iter().cloned());
            write_image_list(&mapper_list, &mapper_names).await?;
            reset_directory(&sparse).await?;
            self.events.stage(
                PipelineStage::Reconstructing,
                0.0,
                "正在把补拍画面注册到原相机重建中",
            );
            colmap::map_incremental(
                &self.engines.colmap,
                &database,
                Path::new("../frames"),
                Path::new("base-model"),
                &sparse,
                Path::new("mapper-images.txt"),
                state.planner_enabled,
                paths.logs.join("colmap.log"),
                &self.process_manager,
                Some(self.process_observer(
                    PipelineStage::Reconstructing,
                    PipelineEngine::Colmap,
                    Some(mapper_names.len() as u64),
                    ObserverMode::Mapper,
                )),
                acceleration.gpu_index(),
            )
            .await?;
            checkpoint.incremental_reconstruction_complete = true;
            state.stage = PipelineStage::Reconstructing;
            state.reshoot = Some(checkpoint.clone());
            project_manager.write_state(&paths.state, &state).await?;
        }

        let (model, report) = best_sparse_model(&paths.frames, &sparse).await?;
        let model_copy = model.clone();
        let registered_reshoot_count =
            tokio::task::spawn_blocking(move || read_registered_images(&model_copy))
                .await
                .map_err(|error| SplatError::Process(format!("读取补拍注册结果失败：{error}")))??
                .into_iter()
                .filter(|image| image.name.starts_with("reshoot_"))
                .count() as u64;
        if registered_reshoot_count == 0 {
            return Err(SplatError::Process("补拍画面没有成功加入原项目。请使用同一设备和镜头重新拍摄，并确保新画面与原画面有足够重叠。".into()));
        }
        checkpoint.registered_reshoot_count = registered_reshoot_count;
        state.reconstruction_complete = true;
        state.reshoot = Some(checkpoint.clone());
        project_manager.write_state(&paths.state, &state).await?;
        if let Some(provenance) = metadata.reshoot.as_mut() {
            provenance.registered_reshoot_count = registered_reshoot_count;
            provenance.reshoot_frame_count = prepared.image_count;
            provenance.mask_count = prepared.mask_count;
            provenance.has_alpha = prepared.has_alpha;
        }

        if state.brush_complete && brush_candidate(&paths.brush).is_none() {
            state.brush_complete = false;
        }
        let candidate = if state.brush_complete {
            brush_candidate(&paths.brush)
                .ok_or_else(|| SplatError::Process("Brush 检查点文件缺失".into()))?
        } else {
            reset_directory(&paths.brush).await?;
            let dataset = prepare_brush_dataset(&paths.brush, &paths.frames, &model).await?;
            let source_long_edge = provenance
                .width
                .max(provenance.height)
                .min(u64::from(u32::MAX)) as u32;
            let resolved_brush = state.brush_training.resolved.unwrap_or_else(|| {
                resolve_brush_training_preset(
                    metadata.quality,
                    state.planner_enabled,
                    acceleration.usable_gpu_total_memory_mb(),
                    source_long_edge,
                    report.points_3d,
                )
            });
            state.brush_training.resolved = Some(resolved_brush);
            project_manager.write_state(&paths.state, &state).await?;
            self.events.send(
                PipelineStage::TrainingSplats,
                Some(PipelineEngine::Brush),
                EventKind::Stage,
                EventLevel::Info,
                None,
                true,
                "正在使用全部已注册画面重新训练高斯",
                Some(0),
                Some(resolved_brush.preset.total_steps as u64),
                Some("iterations"),
            );
            let candidate = self
                .train_brush_with_oom_retry(
                    project_manager,
                    paths,
                    &mut state,
                    metadata.quality,
                    acceleration,
                    &dataset,
                    report.points_3d,
                    resolved_brush,
                    crate::pipeline::estimate::estimate_brush_stage_ms(metadata.quality),
                )
                .await?;
            state.brush_complete = true;
            state.stage = PipelineStage::TrainingSplats;
            project_manager.write_state(&paths.state, &state).await?;
            candidate
        };

        let ply = inspect_gaussian_ply(&candidate)?;
        let final_ply = paths.project.join("final.ply");
        atomic_replace_file(&candidate, &final_ply).await?;
        state.stage = PipelineStage::Completed;
        project_manager.write_state(&paths.state, &state).await?;
        let completed_at = Utc::now();
        let duration_ms = metadata.duration_ms.unwrap_or(0).saturating_add(
            metadata
                .started_at
                .map(|started| (completed_at - started).num_milliseconds().max(0) as u64)
                .unwrap_or(0),
        );
        let reshoot_ratio = registered_reshoot_count as f64 / prepared.image_count.max(1) as f64;
        let warning = (reshoot_ratio < crate::reconstruction::good_registered_ratio()).then(|| format!(
            "补拍画面成功加入 {registered_reshoot_count}/{}，结果已生成，但部分补拍画面未能找到与原项目的联系。",
            prepared.image_count
        ));
        metadata.status = ProjectStatus::Completed;
        metadata.completed_at = Some(completed_at);
        metadata.duration_ms = Some(duration_ms);
        metadata.output = Some(ProjectOutput {
            final_ply: final_ply.clone(),
            file_size: ply.file_size,
            splat_count: ply.splat_count,
            input_images: report.input_images,
            registered_images: report.registered_images,
            registered_ratio: report.registered_ratio,
            points_3d: report.points_3d,
        });
        project_manager
            .write_metadata(&paths.metadata, metadata)
            .await?;
        self.events
            .stage(PipelineStage::Completed, 1.0, "高清补拍处理完成");
        Ok(PipelineResult {
            project_id: paths.id.to_string(),
            project_path: paths.project.clone(),
            final_ply,
            file_size: ply.file_size,
            splat_count: ply.splat_count,
            input_images: report.input_images,
            registered_images: report.registered_images,
            registered_ratio: report.registered_ratio,
            points_3d: report.points_3d,
            duration_ms,
            completed_at,
            warning,
            logs_directory: paths.logs.clone(),
            source_duration_seconds: None,
        })
    }

    async fn run_project(
        &self,
        project_manager: &ProjectManager,
        paths: &ProjectPaths,
        metadata: &mut ProjectMetadata,
        mut state: PipelineStateFile,
        acceleration: &crate::engines::ColmapAccelerationStatus,
    ) -> Result<PipelineResult> {
        let quality = metadata.quality;
        if state.input_type != metadata.input_type {
            return Err(SplatError::Process(
                "项目输入类型与检查点不一致，无法安全继续".into(),
            ));
        }
        recover_interrupted_publish(paths, &state).await?;
        normalize_checkpoints(paths, &mut state).await?;
        project_manager.write_state(&paths.state, &state).await?;
        let uses_current_resolution_policy = state.planner_enabled
            && state.resolution_policy_version == Some(PLANNER_RESOLUTION_POLICY_VERSION);
        let mut preprobed_video = None;
        if uses_current_resolution_policy
            && state.resolution_plan.is_none()
            && metadata.input_type == ProjectInputType::Video
        {
            let video = probe_video(
                &self.engines.ffprobe,
                &metadata.source_path,
                Some(paths.logs.join("ffprobe.log")),
                &self.process_manager,
            )
            .await?;
            let (source_width, source_height) = oriented_video_dimensions(&video);
            let resolution = resolve_planner_resolution_plan(
                quality,
                acceleration.usable_gpu_total_memory_mb(),
                source_width,
                source_height,
                true,
            );
            state.video = Some(video.clone());
            state.resolution_plan = Some(resolution);
            project_manager.write_state(&paths.state, &state).await?;
            preprobed_video = Some(video);
        }
        let target_dimensions = state
            .resolution_plan
            .map(|plan| (plan.working_width, plan.working_height));
        let mut prepared =
            if let Some(prepared) = prepared_frames_from_checkpoint(paths, &state).await? {
                self.events.stage(
                    PipelineStage::ExtractingFrames,
                    1.0,
                    format!("已复用 {} 帧检查点", prepared.extracted_frames),
                );
                prepared
            } else {
                reset_directory(&paths.frames).await?;
                reset_directory(&paths.masks).await?;
                reset_directory(&paths.colmap).await?;
                reset_directory(&paths.brush).await?;
                let prepared = match metadata.input_type {
                    ProjectInputType::Video => {
                        self.prepare_frames(
                            &metadata.source_path,
                            quality,
                            &paths.frames,
                            &paths.masks,
                            Some(&paths.logs),
                            state.planner_enabled,
                            preprobed_video.or_else(|| state.video.clone()),
                            target_dimensions,
                        )
                        .await?
                    }
                    ProjectInputType::Images => {
                        self.prepare_images(
                            &metadata.source_path,
                            quality,
                            &paths.frames,
                            &paths.masks,
                            state.image_sequence.is_some(),
                        )
                        .await?
                    }
                };
                state.input_type = prepared.input_type;
                state.video = prepared.video.clone();
                state.image_sequence = prepared.image_sequence.clone();
                if uses_current_resolution_policy && state.resolution_plan.is_none() {
                    state.resolution_plan = Some(resolve_planner_resolution_plan(
                        quality,
                        acceleration.usable_gpu_total_memory_mb(),
                        prepared.working_width,
                        prepared.working_height,
                        false,
                    ));
                }
                let mut frames = FrameState::from(&prepared.plan);
                frames.extracted_frames = Some(prepared.extracted_frames);
                frames.initial_extracted_frames = prepared.extracted_frames;
                frames.image_format = Some(prepared.image_format.clone());
                frames.mask_count = Some(prepared.mask_count);
                frames.has_alpha = prepared.has_alpha;
                state.frames = Some(frames);
                state.features_complete = false;
                state.matching_complete = false;
                state.reconstruction_complete = false;
                state.brush_complete = false;
                state.stage = PipelineStage::ExtractingFrames;
                project_manager.write_state(&paths.state, &state).await?;
                prepared
            };
        let source_duration_seconds = prepared.video.as_ref().map(|video| video.duration);
        if let Some(tracker) = &self.effectiveness {
            let (source_width, source_height, source_item_count) =
                match (prepared.video.as_ref(), prepared.image_sequence.as_ref()) {
                    (Some(video), _) => (
                        u64::from(video.width),
                        u64::from(video.height),
                        video.total_frames,
                    ),
                    (_, Some(images)) => (
                        u64::from(images.width),
                        u64::from(images.height),
                        images.image_count,
                    ),
                    _ => (0, 0, prepared.extracted_frames),
                };
            let preset = quality.preset();
            let is_planned_video = state.planner_enabled && prepared.video.is_some();
            let initial_selected_count = state
                .frames
                .as_ref()
                .map(|frames| frames.initial_extracted_frames)
                .filter(|count| *count > 0)
                .unwrap_or(prepared.extracted_frames);
            let candidate_count = if is_planned_video {
                prepared.plan.candidate_frames.len() as u64
            } else {
                initial_selected_count
            };
            tracker.record_frame_plan(PlannerFramePlanMetrics {
                source_width,
                source_height,
                source_item_count,
                configured_target_fps: is_planned_video.then_some(preset.initial_fps).flatten(),
                configured_candidate_fps: is_planned_video
                    .then_some(preset.rescue_max_fps)
                    .flatten(),
                effective_target_fps: prepared
                    .video
                    .as_ref()
                    .map(|video| initial_selected_count as f64 / video.duration.max(0.001)),
                effective_candidate_fps: prepared
                    .video
                    .as_ref()
                    .map(|video| candidate_count as f64 / video.duration.max(0.001)),
                initial_selected_count,
                candidate_count,
                minimum_frame_override_applied: is_planned_video
                    && prepared.plan.minimum_frame_override_applied,
                minimum_frame_target_unreachable: is_planned_video
                    && source_item_count < crate::video::minimum_selected_frames(),
            });
        }

        let database = paths.colmap.join("database.db");
        let sparse = paths.colmap.join("sparse");
        let colmap_log = paths.logs.join("colmap.log");
        let preset = quality.preset();
        let sfm_max_image_size = state
            .resolution_plan
            .map(|plan| plan.sfm_max_image_size)
            .unwrap_or(preset.sfm_max_image_size);
        if let Some(resolution) = state.resolution_plan {
            self.events.send(
                PipelineStage::ExtractingFeatures,
                Some(PipelineEngine::System),
                EventKind::Log,
                EventLevel::Info,
                None,
                true,
                format!(
                    "[ResolutionPlan] version={} source={}x{} working={}x{} sfmMax={} brushMax={} profile={}",
                    resolution.policy_version,
                    prepared
                        .video
                        .as_ref()
                        .map(|video| oriented_video_dimensions(video).0)
                        .or_else(|| prepared.image_sequence.as_ref().map(|images| images.width))
                        .unwrap_or(prepared.working_width),
                    prepared
                        .video
                        .as_ref()
                        .map(|video| oriented_video_dimensions(video).1)
                        .or_else(|| prepared.image_sequence.as_ref().map(|images| images.height))
                        .unwrap_or(prepared.working_height),
                    prepared.working_width,
                    prepared.working_height,
                    resolution.sfm_max_image_size,
                    resolution.brush_initial_max_resolution,
                    resolution.brush_initial_profile.label(),
                ),
                None,
                None,
                None,
            );
        }
        // COLMAP's bundled bitmap loader cannot reliably open non-ASCII absolute
        // paths on Windows. The process working directory is work/colmap, so this
        // ASCII-only relative path preserves Unicode/UNC project roots without
        // moving any project data outside the project directory.
        let colmap_images = Path::new("../frames");
        let colmap_masks = prepared.has_alpha.then_some(Path::new("../masks"));

        let backend_label = if acceleration.use_gpu() { "GPU" } else { "CPU" };
        let gpu_index = acceleration.gpu_index();
        if state.features_complete {
            self.events.stage(
                PipelineStage::ExtractingFeatures,
                1.0,
                "已复用特征提取检查点",
            );
        } else {
            reset_directory(&paths.colmap).await?;
            self.events.stage(
                PipelineStage::ExtractingFeatures,
                0.0,
                format!("COLMAP 正在使用 {backend_label} 提取特征"),
            );
            let observer = Some(self.process_observer(
                PipelineStage::ExtractingFeatures,
                PipelineEngine::Colmap,
                Some(prepared.extracted_frames),
                ObserverMode::BracketProgress,
            ));
            if state.planner_enabled {
                colmap::extract_features_quality_v2(
                    &self.engines.colmap,
                    &database,
                    colmap_images,
                    colmap_masks,
                    None,
                    sfm_max_image_size,
                    preset.sfm_max_features,
                    colmap_log.clone(),
                    &self.process_manager,
                    observer,
                    gpu_index,
                )
                .await?;
            } else {
                colmap::extract_features(
                    &self.engines.colmap,
                    &database,
                    colmap_images,
                    colmap_masks,
                    colmap_log.clone(),
                    &self.process_manager,
                    observer,
                    gpu_index,
                )
                .await?;
            }
            state.stage = PipelineStage::ExtractingFeatures;
            state.features_complete = true;
            project_manager.write_state(&paths.state, &state).await?;
            self.events.stage(
                PipelineStage::ExtractingFeatures,
                1.0,
                format!("{backend_label} 特征提取完成"),
            );
        }

        if state.matching_complete {
            self.events.stage(
                PipelineStage::Matching,
                1.0,
                if prepared.input_type == ProjectInputType::Images {
                    "已复用穷举匹配检查点"
                } else {
                    "已复用顺序匹配检查点"
                },
            );
        } else {
            self.events.stage(
                PipelineStage::Matching,
                0.0,
                format!(
                    "COLMAP 正在进行 {backend_label} {}",
                    if prepared.input_type == ProjectInputType::Images {
                        "穷举匹配"
                    } else {
                        "顺序匹配"
                    }
                ),
            );
            let observer = Some(self.process_observer(
                PipelineStage::Matching,
                PipelineEngine::Colmap,
                Some(prepared.extracted_frames),
                ObserverMode::BracketProgress,
            ));
            if prepared.input_type == ProjectInputType::Images {
                colmap::match_exhaustive(
                    &self.engines.colmap,
                    &database,
                    colmap_log.clone(),
                    &self.process_manager,
                    observer,
                    gpu_index,
                )
                .await?;
            } else {
                colmap::match_sequential(
                    &self.engines.colmap,
                    &database,
                    colmap_log.clone(),
                    &self.process_manager,
                    observer,
                    gpu_index,
                )
                .await?;
            }
            state.stage = PipelineStage::Matching;
            state.matching_complete = true;
            project_manager.write_state(&paths.state, &state).await?;
            self.events.stage(
                PipelineStage::Matching,
                1.0,
                if prepared.input_type == ProjectInputType::Images {
                    "穷举匹配完成"
                } else {
                    "顺序匹配完成"
                },
            );
        }

        let allow_two_view_tracks = state.planner_enabled && preset.sfm_allow_two_view_tracks;
        if state.reconstruction_complete {
            self.events
                .stage(PipelineStage::Reconstructing, 1.0, "已复用相机重建检查点");
        } else {
            reset_directory(&sparse).await?;
            self.events.stage(
                PipelineStage::Reconstructing,
                0.0,
                if allow_two_view_tracks {
                    "正在增量重建相机轨迹（High：允许两视图三角化）"
                } else {
                    "正在增量重建相机轨迹"
                },
            );
            colmap::map(
                &self.engines.colmap,
                &database,
                colmap_images,
                &sparse,
                allow_two_view_tracks,
                state.planner_enabled,
                colmap_log.clone(),
                &self.process_manager,
                Some(self.process_observer(
                    PipelineStage::Reconstructing,
                    PipelineEngine::Colmap,
                    Some(prepared.extracted_frames),
                    ObserverMode::Mapper,
                )),
                gpu_index,
            )
            .await?;
            state.stage = PipelineStage::Reconstructing;
            state.reconstruction_complete = true;
            project_manager.write_state(&paths.state, &state).await?;
            self.events
                .stage(PipelineStage::Reconstructing, 1.0, "增量重建完成");
        }

        self.events.stage(
            PipelineStage::ValidatingReconstruction,
            0.0,
            "正在核验注册率和三维点",
        );
        let initial_input_images = state
            .frames
            .as_ref()
            .map(|frames| frames.initial_extracted_frames)
            .filter(|count| *count > 0)
            .unwrap_or(prepared.extracted_frames);
        let (initial_model, initial_report) =
            best_sparse_model_with_input_count(&sparse, initial_input_images).await?;
        if let Some(tracker) = &self.effectiveness {
            tracker.record_gpu_vendor(if acceleration.detected_nvidia_device_count > 0 {
                PlannerGpuVendor::Nvidia
            } else if cfg!(target_os = "macos") {
                PlannerGpuVendor::Apple
            } else {
                PlannerGpuVendor::Unknown
            });
            tracker.record_initial_reconstruction(PlannerInitialReconstructionMetrics {
                input_images: initial_report.input_images,
                registered_images: initial_report.registered_images,
                points_3d: initial_report.points_3d,
                backend: acceleration.backend,
                allow_two_view_tracks,
            });
        }
        let initial_report_for_metrics = initial_report.clone();
        let (model, report) = self
            .maybe_run_bridge_backfill(
                project_manager,
                paths,
                metadata,
                &mut state,
                &mut prepared,
                &database,
                colmap_images,
                colmap_masks,
                &colmap_log,
                gpu_index,
                initial_model,
                initial_report,
            )
            .await?;
        if let Some(tracker) = &self.effectiveness {
            let bridge_applicable =
                state.planner_enabled && prepared.input_type == ProjectInputType::Video;
            let plan = state.bridge_backfill.plan.as_ref();
            tracker.record_bridge(PlannerBridgeMetrics {
                status: if bridge_applicable {
                    PlannerBridgeStatus::from(state.bridge_backfill.status)
                } else {
                    PlannerBridgeStatus::NotApplicable
                },
                trigger_ratio: bridge_trigger_ratio(),
                available_budget: plan.map(|value| value.available_budget).unwrap_or(0),
                requested_frames: plan
                    .map(|value| value.selected_frame_indices.len() as u64)
                    .unwrap_or(0),
                added_frames: if matches!(
                    state.bridge_backfill.status,
                    BridgeBackfillStatus::Completed | BridgeBackfillStatus::FailedRolledBack
                ) {
                    plan.map(|value| value.selected_frame_indices.len() as u64)
                        .unwrap_or(0)
                } else {
                    0
                },
                internal_bridge_count: plan.map(|value| value.internal_bridge_count).unwrap_or(0),
                edge_extension_count: plan.map(|value| value.edge_extension_count).unwrap_or(0),
                duration_ms: state.bridge_backfill.duration_ms,
                initial_input_images: initial_report_for_metrics.input_images,
                initial_registered_images: initial_report_for_metrics.registered_images,
                initial_points_3d: initial_report_for_metrics.points_3d,
                final_input_images: report.input_images,
                final_registered_images: report.registered_images,
                final_points_3d: report.points_3d,
                adopted: state.bridge_backfill.status == BridgeBackfillStatus::Completed,
            });
        }
        let warning = (report.quality == ReconstructionQuality::Warning).then(|| {
            format!(
                "注册率 {:.1}%：低于 {:.1}%，将继续训练，但结果质量可能受影响",
                report.registered_ratio * 100.0,
                crate::reconstruction::good_registered_ratio() * 100.0
            )
        });
        self.events.stage(
            PipelineStage::ValidatingReconstruction,
            1.0,
            format!(
                "注册 {}/{} 张 · 三维点 {}",
                report.registered_images, report.input_images, report.points_3d
            ),
        );

        let candidate = if state.brush_complete {
            if let Some(tracker) = &self.effectiveness {
                if let Some(initial) = state
                    .brush_training
                    .initial_resolved
                    .or(state.brush_training.resolved)
                {
                    tracker.record_brush(initial, false);
                }
                if let Some(resolved) = state.brush_training.resolved {
                    tracker.record_brush(resolved, state.brush_training.oom_retry_used);
                }
            }
            self.events.stage(
                PipelineStage::TrainingSplats,
                1.0,
                "已复用 Brush 训练检查点",
            );
            brush_candidate(&paths.brush)
                .ok_or_else(|| SplatError::Process("Brush 检查点文件缺失，无法继续发布".into()))?
        } else {
            reset_directory(&paths.brush).await?;
            let dataset = prepare_brush_dataset(&paths.brush, &paths.frames, &model).await?;
            let source_long_edge = prepared
                .video
                .as_ref()
                .map(|video| video.width.max(video.height))
                .or_else(|| {
                    prepared
                        .image_sequence
                        .as_ref()
                        .map(|images| images.width.max(images.height))
                })
                .unwrap_or(1);
            let detected_total_memory_mb = acceleration.usable_gpu_total_memory_mb();
            let resolved_brush = state.brush_training.resolved.unwrap_or_else(|| {
                state.resolution_plan.map_or_else(
                    || {
                        resolve_brush_training_preset(
                            quality,
                            state.planner_enabled,
                            detected_total_memory_mb,
                            source_long_edge,
                            report.points_3d,
                        )
                    },
                    |resolution| {
                        resolve_brush_training_preset_for_plan(
                            quality,
                            detected_total_memory_mb,
                            source_long_edge,
                            report.points_3d,
                            &resolution,
                        )
                    },
                )
            });
            state
                .brush_training
                .initial_resolved
                .get_or_insert(resolved_brush);
            state.brush_training.resolved = Some(resolved_brush);
            if let Some(tracker) = &self.effectiveness {
                if let Some(initial) = state.brush_training.initial_resolved {
                    tracker.record_brush(initial, false);
                }
                tracker.record_brush(resolved_brush, state.brush_training.oom_retry_used);
            }
            project_manager.write_state(&paths.state, &state).await?;
            let runtime_samples = catalog::runtime_samples().await;
            let estimated_brush_duration_ms = match (&prepared.video, &prepared.image_sequence) {
                (Some(video), _) => estimate_calibrated_brush_stage_ms(
                    video,
                    &prepared.plan,
                    quality,
                    &runtime_samples,
                    Some(&resolved_brush),
                    state.resolution_policy_version,
                    prepared.working_width.max(prepared.working_height),
                ),
                (_, Some(images)) => estimate_calibrated_brush_stage_ms_for_images(
                    images.image_count,
                    images.width.max(images.height),
                    &prepared.plan,
                    quality,
                    &runtime_samples,
                    Some(&resolved_brush),
                    state.resolution_policy_version,
                    prepared.working_width.max(prepared.working_height),
                ),
                _ => return Err(SplatError::Process("项目输入信息不完整".into())),
            };
            self.events.send(
                PipelineStage::TrainingSplats,
                Some(PipelineEngine::Brush),
                EventKind::Stage,
                EventLevel::Info,
                None,
                true,
                format!(
                    "Brush 训练开始（使用可用图形后端）· {} iterations · 最大分辨率 {} · 预计约 {}",
                    resolved_brush.preset.total_steps,
                    resolved_brush.preset.max_resolution,
                    format_duration(estimated_brush_duration_ms)
                ),
                Some(0),
                Some(resolved_brush.preset.total_steps as u64),
                Some("iterations"),
            );
            let candidate = self
                .train_brush_with_oom_retry(
                    project_manager,
                    paths,
                    &mut state,
                    quality,
                    acceleration,
                    &dataset,
                    report.points_3d,
                    resolved_brush,
                    estimated_brush_duration_ms,
                )
                .await?;
            state.stage = PipelineStage::TrainingSplats;
            state.brush_complete = true;
            project_manager.write_state(&paths.state, &state).await?;
            self.events
                .stage(PipelineStage::TrainingSplats, 1.0, "Brush 训练完成");
            candidate
        };

        self.events
            .stage(PipelineStage::Exporting, 0.0, "正在校验并发布 final.ply");
        let ply = inspect_gaussian_ply(&candidate)?;
        let final_ply = paths.project.join("final.ply");
        atomic_replace_file(&candidate, &final_ply).await?;
        state.stage = PipelineStage::Completed;
        project_manager.write_state(&paths.state, &state).await?;

        let completed_at = Utc::now();
        let duration_ms = metadata.duration_ms.unwrap_or(0).saturating_add(
            metadata
                .started_at
                .map(|started| (completed_at - started).num_milliseconds().max(0) as u64)
                .unwrap_or(0),
        );
        metadata.status = ProjectStatus::Completed;
        metadata.completed_at = Some(completed_at);
        metadata.duration_ms = Some(duration_ms);
        metadata.output = Some(ProjectOutput {
            final_ply: final_ply.clone(),
            file_size: ply.file_size,
            splat_count: ply.splat_count,
            input_images: report.input_images,
            registered_images: report.registered_images,
            registered_ratio: report.registered_ratio,
            points_3d: report.points_3d,
        });
        project_manager
            .write_metadata(&paths.metadata, metadata)
            .await?;

        if let Some(tracker) = &self.effectiveness {
            tracker.record_final_result(PlannerFinalResultMetrics {
                input_images: report.input_images,
                registered_images: report.registered_images,
                points_3d: report.points_3d,
                splat_count: ply.splat_count,
                ply_size_bytes: ply.file_size,
            });
        }

        self.events.stage(
            PipelineStage::Exporting,
            1.0,
            format!("已发布 {} 个 Splat", ply.splat_count),
        );
        self.events
            .stage(PipelineStage::Completed, 1.0, "全部处理完成");
        Ok(PipelineResult {
            project_id: paths.id.to_string(),
            project_path: paths.project.clone(),
            final_ply,
            file_size: ply.file_size,
            splat_count: ply.splat_count,
            input_images: report.input_images,
            registered_images: report.registered_images,
            registered_ratio: report.registered_ratio,
            points_3d: report.points_3d,
            duration_ms,
            completed_at,
            warning,
            logs_directory: paths.logs.clone(),
            source_duration_seconds,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn train_brush_with_oom_retry(
        &self,
        project_manager: &ProjectManager,
        paths: &ProjectPaths,
        state: &mut PipelineStateFile,
        quality: Quality,
        acceleration: &crate::engines::ColmapAccelerationStatus,
        dataset: &Path,
        initial_sfm_points: u64,
        resolved: ResolvedBrushTrainingPreset,
        _estimated_duration_ms: u64,
    ) -> Result<PathBuf> {
        self.log_brush_profile("BrushProfile", &resolved, EventLevel::Info);
        let gpu_launch_policy = brush::BrushGpuLaunchPolicy::from_acceleration(acceleration);
        self.events.send(
            PipelineStage::TrainingSplats,
            Some(PipelineEngine::Brush),
            EventKind::Log,
            EventLevel::Info,
            None,
            true,
            format!("[BrushGpu] {}", gpu_launch_policy.log_summary()),
            None,
            None,
            None,
        );
        let first_log = if state.brush_training.oom_retry_used {
            paths.logs.join("brush-retry.log")
        } else {
            paths.logs.join("brush.log")
        };
        let first = brush::train(
            &self.engines.brush,
            dataset,
            &paths.brush,
            brush::BrushTrainingOptions {
                preset: resolved.preset,
                log_path: first_log,
                gpu_launch_policy: &gpu_launch_policy,
            },
            &self.process_manager,
            Some(self.process_observer(
                PipelineStage::TrainingSplats,
                PipelineEngine::Brush,
                Some(resolved.preset.total_steps as u64),
                ObserverMode::Brush,
            )),
        )
        .await;

        match first {
            Ok(candidate) => Ok(candidate),
            Err(SplatError::BrushOutOfMemory(detail))
                if quality == Quality::High
                    && state.planner_enabled
                    && !state.brush_training.oom_retry_used =>
            {
                let Some(downgraded) = brush_oom_fallback(
                    quality,
                    state.planner_enabled,
                    state.brush_training.oom_retry_used,
                    resolved,
                    initial_sfm_points,
                ) else {
                    return Err(SplatError::BrushOutOfMemory(detail));
                };
                state.brush_training.oom_retry_used = true;
                state.brush_training.resolved = Some(downgraded);
                if let Some(tracker) = &self.effectiveness {
                    tracker.record_brush(downgraded, true);
                }
                project_manager.write_state(&paths.state, state).await?;
                self.events.send(
                    PipelineStage::TrainingSplats,
                    Some(PipelineEngine::Brush),
                    EventKind::Log,
                    EventLevel::Warning,
                    None,
                    true,
                    format!(
                        "[BrushRetry] reason=OutOfMemory from={} to={}，复用现有 SfM 和训练数据重试一次",
                        resolved.profile.label(),
                        downgraded.profile.label()
                    ),
                    None,
                    None,
                    None,
                );
                self.log_brush_profile("BrushRetryProfile", &downgraded, EventLevel::Warning);
                brush::train(
                    &self.engines.brush,
                    dataset,
                    &paths.brush,
                    brush::BrushTrainingOptions {
                        preset: downgraded.preset,
                        log_path: paths.logs.join("brush-retry.log"),
                        gpu_launch_policy: &gpu_launch_policy,
                    },
                    &self.process_manager,
                    Some(self.process_observer(
                        PipelineStage::TrainingSplats,
                        PipelineEngine::Brush,
                        Some(downgraded.preset.total_steps as u64),
                        ObserverMode::Brush,
                    )),
                )
                .await
            }
            Err(error) => Err(error),
        }
    }

    fn log_brush_profile(
        &self,
        label: &str,
        resolved: &ResolvedBrushTrainingPreset,
        level: EventLevel,
    ) {
        let densification = resolved.preset.densification;
        self.events.send(
            PipelineStage::TrainingSplats,
            Some(PipelineEngine::Brush),
            EventKind::Log,
            level,
            None,
            true,
            format!(
                "[{label}] profile={} vramMiB={} resolution={} steps={} threshold={} fraction={} growthStop={} configuredMaxSplats={} effectiveMaxSplats={}",
                resolved.profile.label(),
                resolved
                    .detected_total_memory_mb
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "unknown".into()),
                resolved.preset.max_resolution,
                resolved.preset.total_steps,
                densification
                    .map(|value| value.growth_grad_threshold.to_string())
                    .unwrap_or_else(|| "default".into()),
                densification
                    .map(|value| value.growth_select_fraction.to_string())
                    .unwrap_or_else(|| "default".into()),
                densification
                    .map(|value| value.growth_stop_iter.to_string())
                    .unwrap_or_else(|| "default".into()),
                resolved
                    .configured_max_splats
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "default".into()),
                resolved
                    .preset
                    .max_splats
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "default".into()),
            ),
            None,
            None,
            None,
        );
    }

    #[allow(clippy::too_many_arguments)]
    async fn maybe_run_bridge_backfill(
        &self,
        project_manager: &ProjectManager,
        paths: &ProjectPaths,
        metadata: &ProjectMetadata,
        state: &mut PipelineStateFile,
        prepared: &mut PreparedFrames,
        database: &Path,
        colmap_images: &Path,
        colmap_masks: Option<&Path>,
        colmap_log: &Path,
        gpu_index: Option<u32>,
        initial_model: PathBuf,
        initial_report: ReconstructionReport,
    ) -> Result<(PathBuf, ReconstructionReport)> {
        if !state.planner_enabled || prepared.input_type != ProjectInputType::Video {
            return Ok((initial_model, initial_report));
        }

        match state.bridge_backfill.status {
            BridgeBackfillStatus::NotNeeded
            | BridgeBackfillStatus::NoBudget
            | BridgeBackfillStatus::FailedRolledBack => {
                return Ok((initial_model, initial_report));
            }
            BridgeBackfillStatus::Completed => {
                if let Some(relative) = state.bridge_backfill.selected_model.as_deref() {
                    let model = paths.colmap.join(relative);
                    if let Ok(report) = ReconstructionValidator::validate_with_input_images(
                        prepared.extracted_frames,
                        &model,
                    ) {
                        return Ok((model, report));
                    }
                }
                state.bridge_backfill.status = BridgeBackfillStatus::FailedRolledBack;
                project_manager.write_state(&paths.state, state).await?;
                return Ok((initial_model, initial_report));
            }
            BridgeBackfillStatus::NotEvaluated | BridgeBackfillStatus::Running => {}
        }

        if initial_report.registered_ratio >= bridge_trigger_ratio() {
            state.bridge_backfill.status = BridgeBackfillStatus::NotNeeded;
            project_manager.write_state(&paths.state, state).await?;
            self.events.stage(
                PipelineStage::ValidatingReconstruction,
                1.0,
                format!(
                    "初始注册 {}/{} 张（{:.1}%）达到 {:.1}% 阈值，无需 Bridge Backfill",
                    initial_report.registered_images,
                    initial_report.input_images,
                    initial_report.registered_ratio * 100.0,
                    bridge_trigger_ratio() * 100.0
                ),
            );
            return Ok((initial_model, initial_report));
        }

        let bridge_plan = if state.bridge_backfill.status == BridgeBackfillStatus::Running {
            state.bridge_backfill.plan.clone().unwrap_or_default()
        } else {
            let registered = match read_registered_source_indices(&initial_model) {
                Ok(indices) => indices,
                Err(error) => {
                    state.bridge_backfill.status = BridgeBackfillStatus::FailedRolledBack;
                    project_manager.write_state(&paths.state, state).await?;
                    self.events.send(
                        PipelineStage::ValidatingReconstruction,
                        Some(PipelineEngine::System),
                        EventKind::Log,
                        EventLevel::Warning,
                        Some(1.0),
                        false,
                        format!(
                            "Bridge Backfill 无法读取初始注册时间线，继续使用初始模型：{error}"
                        ),
                        None,
                        None,
                        None,
                    );
                    return Ok((initial_model, initial_report));
                }
            };
            let plan = plan_bridge_backfill(
                &prepared.plan,
                &registered,
                initial_report.registered_images,
            );
            state.bridge_backfill.plan = Some(plan.clone());
            state.bridge_backfill.initial_model = Some(relative_model_path(paths, &initial_model));
            if plan.selected_frame_indices.is_empty() {
                state.bridge_backfill.status = BridgeBackfillStatus::NoBudget;
                project_manager.write_state(&paths.state, state).await?;
                self.events.send(
                    PipelineStage::ValidatingReconstruction,
                    Some(PipelineEngine::System),
                    EventKind::Log,
                    EventLevel::Warning,
                    Some(1.0),
                    false,
                    format!(
                        "初始注册 {}/{} 张（{:.1}%）低于 {:.1}%，但 Bridge Backfill 没有可用补帧预算",
                        initial_report.registered_images,
                        initial_report.input_images,
                        initial_report.registered_ratio * 100.0,
                        bridge_trigger_ratio() * 100.0
                    ),
                    None,
                    None,
                    None,
                );
                return Ok((initial_model, initial_report));
            }
            state.bridge_backfill.status = BridgeBackfillStatus::Running;
            project_manager.write_state(&paths.state, state).await?;
            plan
        };

        let additional = bridge_plan
            .selected_frame_indices
            .iter()
            .filter_map(|index| {
                prepared
                    .plan
                    .candidate_frames
                    .iter()
                    .find(|frame| frame.source_frame_index == *index)
                    .cloned()
            })
            .collect::<Vec<_>>();
        if additional.len() != bridge_plan.selected_frame_indices.len() {
            state.bridge_backfill.status = BridgeBackfillStatus::FailedRolledBack;
            project_manager.write_state(&paths.state, state).await?;
            self.events.send(
                PipelineStage::ValidatingReconstruction,
                Some(PipelineEngine::System),
                EventKind::Log,
                EventLevel::Warning,
                Some(1.0),
                false,
                "Bridge Backfill 检查点与候选池不一致，继续使用初始模型",
                None,
                None,
                None,
            );
            return Ok((initial_model, initial_report));
        }

        if let Some(first) = bridge_plan.selection_trace.first() {
            self.events.send(
                PipelineStage::ValidatingReconstruction,
                Some(PipelineEngine::System),
                EventKind::Log,
                EventLevel::Info,
                Some(0.0),
                false,
                format!(
                    "Bridge Backfill 最长未注册区 {}..{}，选择中点候选帧 {}",
                    first.gap_start_frame_index,
                    first.gap_end_frame_index,
                    first.selected_frame_index
                ),
                Some(1),
                Some(additional.len() as u64),
                Some("frames"),
            );
        }
        self.events.send(
            PipelineStage::ValidatingReconstruction,
            Some(PipelineEngine::System),
            EventKind::Log,
            EventLevel::Warning,
            Some(0.0),
            false,
            format!(
                "Bridge Backfill 已触发：初始注册={}/{}（{:.1}%），剩余预算={}，补帧={}，内部桥接={}，边缘延伸={}",
                bridge_plan.initial_registered_images,
                bridge_plan.initial_input_images,
                bridge_plan.initial_registration_ratio * 100.0,
                bridge_plan.available_budget,
                additional.len(),
                bridge_plan.internal_bridge_count,
                bridge_plan.edge_extension_count
            ),
            Some(additional.len() as u64),
            Some(bridge_plan.available_budget),
            Some("frames"),
        );

        let bridge_started = Instant::now();
        let attempt = self
            .execute_bridge_attempt(
                project_manager,
                paths,
                metadata,
                state,
                prepared,
                database,
                colmap_images,
                colmap_masks,
                colmap_log,
                gpu_index,
                &initial_model,
                &initial_report,
                &additional,
            )
            .await;
        match attempt {
            Ok((model, report)) => {
                let bridge_duration_ms = bridge_started.elapsed().as_millis() as u64;
                state.bridge_backfill.status = BridgeBackfillStatus::Completed;
                state.bridge_backfill.selected_model = Some(relative_model_path(paths, &model));
                state.bridge_backfill.final_registered_images = Some(report.registered_images);
                state.bridge_backfill.final_points_3d = Some(report.points_3d);
                state.bridge_backfill.duration_ms = Some(bridge_duration_ms);
                project_manager.write_state(&paths.state, state).await?;
                self.events.send(
                    PipelineStage::ValidatingReconstruction,
                    Some(PipelineEngine::System),
                    EventKind::Log,
                    EventLevel::Info,
                    Some(1.0),
                    false,
                    format!(
                        "Bridge Backfill 完成：注册 {}（{:.1}%）→ {}（{:.1}%），三维点 {} → {}，耗时 {}；采用增量模型",
                        initial_report.registered_images,
                        initial_report.registered_ratio * 100.0,
                        report.registered_images,
                        report.registered_ratio * 100.0,
                        initial_report.points_3d,
                        report.points_3d,
                        format_duration(bridge_duration_ms)
                    ),
                    Some(report.registered_images),
                    Some(report.input_images),
                    Some("images"),
                );
                Ok((model, report))
            }
            Err(error) => {
                let bridge_duration_ms = bridge_started.elapsed().as_millis() as u64;
                remove_bridge_outputs(&paths.frames, &paths.masks, &additional, prepared.has_alpha)
                    .await;
                let added = additional
                    .iter()
                    .map(|frame| frame.source_frame_index)
                    .collect::<HashSet<_>>();
                prepared
                    .plan
                    .selected_frames
                    .retain(|frame| !added.contains(&frame.source_frame_index));
                prepared.extracted_frames = bridge_plan.initial_input_images;
                prepared.mask_count = if prepared.has_alpha {
                    bridge_plan.initial_input_images
                } else {
                    0
                };
                prepared.plan.estimated_frames = bridge_plan.initial_input_images;
                if let Some(video) = prepared.video.as_ref() {
                    prepared.plan.retention_ratio =
                        bridge_plan.initial_input_images as f64 / video.total_frames.max(1) as f64;
                    prepared.plan.sampling_fps =
                        bridge_plan.initial_input_images as f64 / video.duration.max(0.001);
                }
                update_frame_checkpoint(state, prepared);
                state.bridge_backfill.status = BridgeBackfillStatus::FailedRolledBack;
                state.bridge_backfill.selected_model = state.bridge_backfill.initial_model.clone();
                state.bridge_backfill.duration_ms = Some(bridge_duration_ms);
                project_manager.write_state(&paths.state, state).await?;
                self.events.send(
                    PipelineStage::ValidatingReconstruction,
                    Some(PipelineEngine::System),
                    EventKind::Log,
                    EventLevel::Warning,
                    Some(1.0),
                    false,
                    format!(
                        "Bridge Backfill 失败并回退可用初始模型（耗时 {}）：{error}",
                        format_duration(bridge_duration_ms)
                    ),
                    Some(initial_report.registered_images),
                    Some(initial_report.input_images),
                    Some("images"),
                );
                Ok((initial_model, initial_report))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_bridge_attempt(
        &self,
        project_manager: &ProjectManager,
        paths: &ProjectPaths,
        metadata: &ProjectMetadata,
        state: &mut PipelineStateFile,
        prepared: &mut PreparedFrames,
        database: &Path,
        colmap_images: &Path,
        colmap_masks: Option<&Path>,
        colmap_log: &Path,
        gpu_index: Option<u32>,
        initial_model: &Path,
        initial_report: &ReconstructionReport,
        additional: &[PlannedFrame],
    ) -> Result<(PathBuf, ReconstructionReport)> {
        self.events.stage(
            PipelineStage::ValidatingReconstruction,
            0.0,
            format!("Bridge Backfill 正在提取 {} 张新增帧", additional.len()),
        );
        let extraction = extract_additional_frames(
            &self.engines.ffmpeg,
            &metadata.source_path,
            &paths.frames,
            &paths.masks,
            additional,
            prepared.has_alpha,
            Some((prepared.working_width, prepared.working_height)),
            Some(paths.logs.join("ffmpeg-bridge.log")),
            &self.process_manager,
            Some(self.process_observer(
                PipelineStage::ValidatingReconstruction,
                PipelineEngine::Ffmpeg,
                Some(additional.len() as u64),
                ObserverMode::Ffmpeg,
            )),
        )
        .await?;
        for frame in additional {
            if !prepared
                .plan
                .selected_frames
                .iter()
                .any(|selected| selected.source_frame_index == frame.source_frame_index)
            {
                prepared.plan.selected_frames.push(frame.clone());
            }
        }
        prepared
            .plan
            .selected_frames
            .sort_by_key(|frame| frame.source_frame_index);
        prepared.extracted_frames = extraction.frame_count;
        prepared.mask_count = extraction.mask_count;
        prepared.plan.estimated_frames = extraction.frame_count;
        prepared.plan.retention_ratio = prepared
            .video
            .as_ref()
            .map(|video| extraction.frame_count as f64 / video.total_frames.max(1) as f64)
            .unwrap_or(prepared.plan.retention_ratio);
        if let Some(video) = prepared.video.as_ref() {
            prepared.plan.sampling_fps = extraction.frame_count as f64 / video.duration.max(0.001);
        }
        update_frame_checkpoint(state, prepared);
        project_manager.write_state(&paths.state, state).await?;

        let extension = if prepared.has_alpha { "png" } else { "jpg" };
        let image_list = paths.colmap.join("bridge-images.txt");
        let image_list_text = additional
            .iter()
            .map(|frame| format!("frame_{:010}.{extension}", frame.source_frame_index))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        tokio::fs::write(&image_list, image_list_text).await?;
        let preset = metadata.quality.preset();
        let sfm_max_image_size = state
            .resolution_plan
            .map(|plan| plan.sfm_max_image_size)
            .unwrap_or(preset.sfm_max_image_size);
        self.events.stage(
            PipelineStage::ValidatingReconstruction,
            0.0,
            format!(
                "Bridge Backfill 正在为 {} 张新增帧提取特征",
                additional.len()
            ),
        );
        colmap::extract_features_quality_v2(
            &self.engines.colmap,
            database,
            colmap_images,
            colmap_masks,
            Some(&image_list),
            sfm_max_image_size,
            preset.sfm_max_features,
            paths.logs.join("colmap-bridge-features.log"),
            &self.process_manager,
            Some(self.process_observer(
                PipelineStage::ValidatingReconstruction,
                PipelineEngine::Colmap,
                Some(additional.len() as u64),
                ObserverMode::BracketProgress,
            )),
            gpu_index,
        )
        .await?;

        let all_indices = prepared
            .plan
            .selected_frames
            .iter()
            .map(|frame| frame.source_frame_index)
            .collect::<Vec<_>>();
        let added_indices = additional
            .iter()
            .map(|frame| frame.source_frame_index)
            .collect::<Vec<_>>();
        let pair_list = paths.colmap.join("bridge-pairs.txt");
        let pair_count =
            write_bridge_pair_list(&pair_list, &all_indices, &added_indices, prepared.has_alpha)?;
        if pair_count == 0 {
            return Err(SplatError::Process(
                "Bridge Backfill did not produce any local matching pairs".into(),
            ));
        }
        self.events.stage(
            PipelineStage::ValidatingReconstruction,
            0.0,
            format!("Bridge Backfill 正在匹配 {pair_count} 组局部帧对"),
        );
        colmap::match_pairs(
            &self.engines.colmap,
            database,
            &pair_list,
            paths.logs.join("colmap-bridge-matching.log"),
            &self.process_manager,
            Some(self.process_observer(
                PipelineStage::ValidatingReconstruction,
                PipelineEngine::Colmap,
                Some(pair_count as u64),
                ObserverMode::BracketProgress,
            )),
            gpu_index,
        )
        .await?;

        let bridge_sparse = paths.colmap.join("sparse-bridge");
        reset_directory(&bridge_sparse).await?;
        self.events.stage(
            PipelineStage::ValidatingReconstruction,
            0.0,
            "Bridge Backfill 正在复用初始模型继续增量重建",
        );
        let input_model = initial_model
            .strip_prefix(&paths.colmap)
            .unwrap_or(initial_model);
        colmap::map_from_existing(
            &self.engines.colmap,
            database,
            colmap_images,
            input_model,
            &bridge_sparse,
            state.planner_enabled,
            colmap_log.with_file_name("colmap-bridge-mapper.log"),
            &self.process_manager,
            Some(self.process_observer(
                PipelineStage::ValidatingReconstruction,
                PipelineEngine::Colmap,
                Some(prepared.extracted_frames),
                ObserverMode::Mapper,
            )),
            gpu_index,
        )
        .await?;
        let (model, report) =
            best_sparse_model_with_input_count(&bridge_sparse, prepared.extracted_frames).await?;
        if report.registered_images < initial_report.registered_images {
            return Err(SplatError::Process(format!(
                "Bridge model registered fewer images than the initial model ({} < {})",
                report.registered_images, initial_report.registered_images
            )));
        }
        Ok((model, report))
    }

    fn process_observer(
        &self,
        stage: PipelineStage,
        engine: PipelineEngine,
        expected_total: Option<u64>,
        mode: ObserverMode,
    ) -> ProcessObserver {
        let events = self.events.clone();
        let mapper_count = Arc::new(AtomicU64::new(0));
        let runtime = Arc::new(std::sync::Mutex::new(RuntimeTracker::new()));
        let matching_heartbeat_bucket = Arc::new(AtomicU64::new(0));
        Arc::new(move |update| match update {
            ProcessUpdate::Started { process_id } => {
                let mut tracker = runtime
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                tracker.snapshot.process_id = process_id;
                if mode != ObserverMode::Brush {
                    tracker.snapshot.phase = "running".into();
                }
                events.runtime(stage, engine, tracker.refresh(Instant::now()));
                events.send(
                    stage,
                    Some(engine),
                    EventKind::Log,
                    EventLevel::Info,
                    None,
                    true,
                    format!("进程已启动 · PID {process_id}"),
                    None,
                    expected_total,
                    None,
                );
            }
            ProcessUpdate::Resources(sample) => {
                let mut tracker = runtime
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if tracker.snapshot.process_id != sample.process_id {
                    return;
                }
                tracker.snapshot.resources = Some(sample);
                events.runtime(stage, engine, tracker.refresh(Instant::now()));
            }
            ProcessUpdate::Heartbeat { elapsed_ms } => {
                let mut tracker = runtime
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                events.runtime(stage, engine, tracker.refresh(Instant::now()));
                if mode == ObserverMode::Matching {
                    let bucket = elapsed_ms / 10_000;
                    if bucket > 0
                        && matching_heartbeat_bucket.fetch_max(bucket, Ordering::Relaxed) < bucket
                    {
                        events.send(
                            stage,
                            Some(engine),
                            EventKind::Heartbeat,
                            EventLevel::Info,
                            None,
                            true,
                            format!("补拍图像匹配中 · 已用时 {}", format_duration(elapsed_ms)),
                            None,
                            expected_total,
                            None,
                        );
                    }
                }
            }
            ProcessUpdate::Line { stream: _, line } => {
                if line.is_empty() {
                    return;
                }
                let brush = mode == ObserverMode::Brush;
                let actual = {
                    let mut tracker = runtime
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let actual = tracker.line(&line, Instant::now(), brush);
                    events.runtime(stage, engine, tracker.refresh(Instant::now()));
                    actual
                };
                let parsed = match mode {
                    ObserverMode::Ffmpeg => parse_ffmpeg_frame(&line).map(|current| {
                        (
                            current,
                            expected_total,
                            format!("FFmpeg 已输出 {current} 帧"),
                        )
                    }),
                    ObserverMode::BracketProgress | ObserverMode::Matching => {
                        parse_bracket_progress(&line).map(|(current, total)| {
                            (current, Some(total), friendly_engine_line(&line))
                        })
                    }
                    ObserverMode::Mapper => {
                        parse_mapper_progress(&line, &mapper_count, expected_total)
                    }
                    ObserverMode::Brush => actual.map(|(current, total)| {
                        (
                            current,
                            Some(total),
                            format!("Brush 训练 · {current}/{total} 步"),
                        )
                    }),
                };
                if let Some((current, total, message)) = parsed {
                    let progress = total
                        .filter(|value| *value > 0)
                        .map(|value| current as f32 / value as f32);
                    events.send(
                        stage,
                        Some(engine),
                        EventKind::Progress,
                        EventLevel::Info,
                        progress.map(|value| if brush { value.min(0.99) } else { value }),
                        progress.is_none(),
                        message,
                        Some(current),
                        total,
                        Some(if brush { "steps" } else { "张" }),
                    );
                } else if mode == ObserverMode::Brush {
                    events.send(
                        stage,
                        Some(engine),
                        EventKind::Log,
                        if line.contains(" WARN ") {
                            EventLevel::Warning
                        } else if line.contains(" ERROR ")
                            || line.contains("panicked at")
                            || line.starts_with("Error:")
                        {
                            EventLevel::Error
                        } else {
                            EventLevel::Info
                        },
                        None,
                        true,
                        friendly_engine_line(&line),
                        None,
                        expected_total,
                        None,
                    );
                } else if is_useful_line(&line) {
                    events.send(
                        stage,
                        Some(engine),
                        EventKind::Log,
                        EventLevel::Info,
                        None,
                        true,
                        friendly_engine_line(&line),
                        None,
                        expected_total,
                        None,
                    );
                }
            }
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ObserverMode {
    Ffmpeg,
    BracketProgress,
    Matching,
    Mapper,
    Brush,
}

fn parse_ffmpeg_frame(line: &str) -> Option<u64> {
    line.strip_prefix("frame=")?.trim().parse().ok()
}

fn parse_bracket_progress(line: &str) -> Option<(u64, u64)> {
    let open = line.find('[')?;
    let close = line[open + 1..].find(']')? + open + 1;
    let value = &line[open + 1..close];
    let (current, total) = value.split_once('/')?;
    Some((current.trim().parse().ok()?, total.trim().parse().ok()?))
}

fn parse_mapper_progress(
    line: &str,
    counter: &AtomicU64,
    expected_total: Option<u64>,
) -> Option<(u64, Option<u64>, String)> {
    let reported_count = value_after(line, "num_reg_frames=")
        .or_else(|| value_after(line, "num_reg_frames ="))
        .and_then(|value| value.parse::<u64>().ok());
    let lower = line.to_ascii_lowercase();
    if lower.contains("retriangulation") || lower.contains("global bundle adjustment") {
        if let Some(current) = reported_count {
            counter.fetch_max(current, Ordering::Relaxed);
        }
        let current = counter.load(Ordering::Relaxed);
        if current > 0 {
            return Some((current, expected_total, friendly_engine_line(line)));
        }
        return None;
    }
    if let Some(current) = reported_count {
        counter.fetch_max(current, Ordering::Relaxed);
        let current = counter.load(Ordering::Relaxed);
        return Some((current, expected_total, format!("已注册 {current} 张图像")));
    }
    if line.contains("Registering image #") {
        let current = counter.fetch_add(1, Ordering::Relaxed) + 1;
        return Some((
            current,
            expected_total,
            format!("正在注册第 {current} 张图像"),
        ));
    }
    None
}

fn value_after<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    let value = line.split_once(marker)?.1;
    Some(
        value
            .split_whitespace()
            .next()?
            .trim_matches(|ch: char| !ch.is_ascii_digit()),
    )
}

fn is_useful_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    [
        "bundle", "register", "triang", "elapsed", "warning", "error", "writing", "loading",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn friendly_engine_line(line: &str) -> String {
    const MAX: usize = 360;
    let mut value = line.trim().to_string();
    if value.chars().count() > MAX {
        value = value.chars().take(MAX).collect::<String>() + "…";
    }
    value
}

fn format_duration(milliseconds: u64) -> String {
    let seconds = milliseconds / 1_000;
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3_600,
        (seconds % 3_600) / 60,
        seconds % 60
    )
}

fn brush_oom_fallback(
    quality: Quality,
    planner_enabled: bool,
    retry_used: bool,
    resolved: ResolvedBrushTrainingPreset,
    initial_sfm_points: u64,
) -> Option<ResolvedBrushTrainingPreset> {
    (quality == Quality::High && planner_enabled && !retry_used)
        .then(|| resolved.downgrade_after_oom(initial_sfm_points))
        .flatten()
}

fn checkpoint_stage(state: &PipelineStateFile) -> PipelineStage {
    if state.brush_complete {
        PipelineStage::TrainingSplats
    } else if state.reconstruction_complete {
        PipelineStage::Reconstructing
    } else if state.matching_complete {
        PipelineStage::Matching
    } else if state.features_complete {
        PipelineStage::ExtractingFeatures
    } else if state
        .frames
        .as_ref()
        .and_then(|frames| frames.extracted_frames)
        .is_some_and(|count| count > 0)
    {
        PipelineStage::ExtractingFrames
    } else {
        PipelineStage::Created
    }
}

async fn count_image_files(directory: &Path) -> Result<u64> {
    let directory = directory.to_path_buf();
    tokio::task::spawn_blocking(move || {
        Ok::<u64, SplatError>(crate::video::list_images(&directory)?.len() as u64)
    })
    .await
    .map_err(|error| SplatError::Process(format!("无法统计输入画面：{error}")))?
}

async fn ensure_reshoot_input_is_new(
    source_project: &Path,
    stored_source: &Path,
    input: &Path,
    input_type: ReshootInputType,
) -> Result<()> {
    let canonical_input = tokio::fs::canonicalize(input).await.ok();
    let canonical_project = tokio::fs::canonicalize(source_project).await.ok();
    if canonical_input
        .as_ref()
        .zip(canonical_project.as_ref())
        .is_some_and(|(input, project)| input.starts_with(project))
    {
        return Err(original_reshoot_input_error());
    }

    let is_original = match input_type {
        ReshootInputType::Video => {
            stored_source.is_file() && files_have_same_content(stored_source, input).await?
        }
        ReshootInputType::Images => {
            let same_as_source = stored_source.is_dir()
                && image_sequences_share_content(stored_source, input).await?;
            if same_as_source {
                true
            } else {
                let prepared_source_frames = source_project.join("work/frames");
                prepared_source_frames.is_dir()
                    && image_sequences_share_content(&prepared_source_frames, input).await?
            }
        }
    };
    if is_original {
        Err(original_reshoot_input_error())
    } else {
        Ok(())
    }
}

fn original_reshoot_input_error() -> SplatError {
    SplatError::Process("补拍时不能再次使用原素材，请选择新拍摄的视频或图片序列。".into())
}

async fn image_sequences_share_content(left: &Path, right: &Path) -> Result<bool> {
    if !left.is_dir() || !right.is_dir() {
        return Ok(false);
    }
    let left = left.to_path_buf();
    let right = right.to_path_buf();
    let (left, right) = tokio::task::spawn_blocking(move || {
        Ok::<_, SplatError>((
            crate::video::list_images(&left)?,
            crate::video::list_images(&right)?,
        ))
    })
    .await
    .map_err(|error| SplatError::Process(format!("比较补拍素材失败：{error}")))??;
    let mut source_fingerprints: HashMap<(u64, u64), Vec<PathBuf>> = HashMap::new();
    for path in left {
        source_fingerprints
            .entry(file_fingerprint(&path).await?)
            .or_default()
            .push(path);
    }
    for path in right {
        if let Some(candidates) = source_fingerprints.get(&file_fingerprint(&path).await?) {
            for source in candidates {
                if files_have_same_content(source, &path).await? {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

async fn file_fingerprint(path: &Path) -> Result<(u64, u64)> {
    let mut file = tokio::fs::File::open(path).await?;
    let length = file.metadata().await?.len();
    let mut hash = 0xcbf29ce484222325_u64;
    let mut buffer = vec![0_u8; 256 * 1024];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            return Ok((length, hash));
        }
        for byte in &buffer[..read] {
            hash = (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
        }
    }
}

async fn files_have_same_content(left: &Path, right: &Path) -> Result<bool> {
    if !left.is_file() || !right.is_file() {
        return Ok(false);
    }
    let left_metadata = tokio::fs::metadata(left).await?;
    let right_metadata = tokio::fs::metadata(right).await?;
    if left_metadata.len() != right_metadata.len() {
        return Ok(false);
    }

    let mut left_file = tokio::fs::File::open(left).await?;
    let mut right_file = tokio::fs::File::open(right).await?;
    let mut left_buffer = vec![0_u8; 256 * 1024];
    let mut right_buffer = vec![0_u8; 256 * 1024];
    loop {
        let left_read = left_file.read(&mut left_buffer).await?;
        let right_read = right_file.read(&mut right_buffer).await?;
        if left_read != right_read || left_buffer[..left_read] != right_buffer[..right_read] {
            return Ok(false);
        }
        if left_read == 0 {
            return Ok(true);
        }
    }
}

fn mark_state_terminal(mut state: PipelineStateFile, cancelled: bool) -> PipelineStateFile {
    state.stage = if cancelled {
        PipelineStage::Cancelled
    } else {
        PipelineStage::Failed
    };
    state
}

async fn copy_image_files(source: &Path, destination: &Path) -> Result<()> {
    tokio::fs::create_dir_all(destination).await?;
    for image in crate::video::list_images(source)? {
        let name = image
            .file_name()
            .ok_or_else(|| SplatError::InvalidPath(image.clone()))?;
        tokio::fs::copy(&image, destination.join(name)).await?;
    }
    Ok(())
}

async fn remove_reshoot_images(directory: &Path) -> Result<()> {
    if !directory.is_dir() {
        return Ok(());
    }
    let mut entries = tokio::fs::read_dir(directory).await?;
    while let Some(entry) = entries.next_entry().await? {
        if entry.file_name().to_string_lossy().starts_with("reshoot_") && entry.path().is_file() {
            tokio::fs::remove_file(entry.path()).await?;
        }
    }
    Ok(())
}

async fn move_extracted_reshoot(
    extracted_frames: &Path,
    extracted_masks: &Path,
    frames: &Path,
    masks: &Path,
    has_alpha: bool,
) -> Result<()> {
    tokio::fs::create_dir_all(frames).await?;
    if has_alpha {
        tokio::fs::create_dir_all(masks).await?;
    }
    for (index, source) in crate::video::list_images(extracted_frames)?
        .iter()
        .enumerate()
    {
        let extension = source
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("jpg");
        let old_name = source
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| SplatError::InvalidPath(source.clone()))?;
        let new_name = format!("reshoot_{:06}.{extension}", index + 1);
        tokio::fs::rename(source, frames.join(&new_name)).await?;
        if has_alpha {
            tokio::fs::rename(
                extracted_masks.join(format!("{old_name}.png")),
                masks.join(format!("{new_name}.png")),
            )
            .await?;
        }
    }
    Ok(())
}

async fn image_names_with_prefix(directory: &Path, prefix: &str) -> Result<Vec<String>> {
    Ok(crate::video::list_images(directory)?
        .into_iter()
        .filter_map(|path| {
            path.file_name()
                .and_then(|value| value.to_str())
                .map(str::to_owned)
        })
        .filter(|name| name.starts_with(prefix))
        .collect())
}

async fn write_image_list(path: &Path, names: &[String]) -> Result<()> {
    let mut value = names.join("\n");
    value.push('\n');
    tokio::fs::write(path, value).await?;
    Ok(())
}

async fn validate_image_dimensions(
    directory: &Path,
    prefix: &str,
    expected_width: u64,
    expected_height: u64,
) -> Result<()> {
    let directory = directory.to_path_buf();
    let prefix = prefix.to_owned();
    tokio::task::spawn_blocking(move || {
        use image::GenericImageView;
        for path in crate::video::list_images(&directory)? {
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            if !name.starts_with(&prefix) {
                continue;
            }
            let dimensions = image::ImageReader::open(&path)
                .map_err(|error| {
                    SplatError::Process(format!("无法读取补拍画面 {}：{error}", path.display()))
                })?
                .with_guessed_format()
                .map_err(|error| {
                    SplatError::Process(format!("无法识别补拍画面 {}：{error}", path.display()))
                })?
                .decode()
                .map_err(|error| {
                    SplatError::Process(format!("补拍画面损坏 {}：{error}", path.display()))
                })?
                .dimensions();
            if u64::from(dimensions.0) != expected_width
                || u64::from(dimensions.1) != expected_height
            {
                return Err(SplatError::Process(format!(
                    "补拍画面 {} 的分辨率为 {}×{}，需要与原项目保持 {}×{}",
                    name, dimensions.0, dimensions.1, expected_width, expected_height
                )));
            }
        }
        Ok(())
    })
    .await
    .map_err(|error| SplatError::Process(format!("补拍画面校验失败：{error}")))?
}

async fn normalize_checkpoints(paths: &ProjectPaths, state: &mut PipelineStateFile) -> Result<()> {
    let frames_complete = prepared_frames_from_checkpoint(paths, state)
        .await?
        .is_some();
    if !frames_complete {
        state.video = None;
        state.image_sequence = None;
        state.frames = None;
        state.bridge_backfill = Default::default();
    }

    let database_complete = tokio::fs::metadata(paths.colmap.join("database.db"))
        .await
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0);
    state.features_complete = frames_complete && state.features_complete && database_complete;
    state.matching_complete = state.features_complete && state.matching_complete;
    state.reconstruction_complete = state.matching_complete
        && state.reconstruction_complete
        && best_sparse_model(&paths.frames, &paths.colmap.join("sparse"))
            .await
            .is_ok();
    if !state.reconstruction_complete {
        state.brush_training = Default::default();
    }
    state.brush_complete = state.reconstruction_complete
        && state.brush_complete
        && brush_candidate(&paths.brush)
            .and_then(|path| inspect_gaussian_ply(&path).ok())
            .is_some();
    state.stage = checkpoint_stage(state);
    Ok(())
}

async fn prepared_frames_from_checkpoint(
    paths: &ProjectPaths,
    state: &PipelineStateFile,
) -> Result<Option<PreparedFrames>> {
    let Some(frames) = state.frames.as_ref() else {
        return Ok(None);
    };
    let Some(extracted_frames) = frames.extracted_frames.filter(|count| *count > 0) else {
        return Ok(None);
    };
    let plan = FramePlan {
        retention_ratio: frames.retention_ratio,
        sampling_fps: frames.sampling_fps,
        estimated_frames: frames.estimated_frames,
        selected_frames: frames.selected_frames.clone(),
        candidate_frames: frames.candidate_frames.clone(),
        rescue_max_frames: frames.rescue_max_frames,
        minimum_frame_override_applied: frames.minimum_frame_override_applied,
    };
    match state.input_type {
        ProjectInputType::Video => {
            let Some(video) = state.video.clone() else {
                return Ok(None);
            };
            let has_alpha = frames.has_alpha || video.has_alpha;
            let Ok(extraction) = validate_extraction(&paths.frames, &paths.masks, has_alpha).await
            else {
                return Ok(None);
            };
            if extraction.frame_count != extracted_frames
                || frames
                    .image_format
                    .as_deref()
                    .is_some_and(|format| format != extraction.image_format.as_str())
                || frames
                    .mask_count
                    .is_some_and(|count| count != extraction.mask_count)
            {
                return Ok(None);
            }
            if state.resolution_plan.is_some_and(|resolution| {
                (extraction.width, extraction.height)
                    != (resolution.working_width, resolution.working_height)
            }) {
                return Ok(None);
            }
            Ok(Some(PreparedFrames {
                input_type: ProjectInputType::Video,
                video: Some(video),
                image_sequence: None,
                plan,
                extracted_frames,
                image_format: extraction.image_format.as_str().into(),
                mask_count: extraction.mask_count,
                has_alpha: extraction.has_alpha,
                working_width: extraction.width,
                working_height: extraction.height,
            }))
        }
        ProjectInputType::Images => {
            let Some(image_sequence) = state.image_sequence.clone() else {
                return Ok(None);
            };
            let frames_dir = paths.frames.clone();
            let masks_dir = paths.masks.clone();
            let has_alpha = frames.has_alpha;
            let Ok(prepared) = tokio::task::spawn_blocking(move || {
                validate_prepared_image_sequence(
                    &frames_dir,
                    &masks_dir,
                    extracted_frames,
                    has_alpha,
                )
                .map(|prepared| (prepared, image_sequence))
            })
            .await
            .map_err(|error| SplatError::Process(format!("图片检查点校验失败：{error}")))?
            else {
                return Ok(None);
            };
            let (prepared, image_sequence) = prepared;
            if frames
                .mask_count
                .is_some_and(|count| count != prepared.mask_count)
            {
                return Ok(None);
            }
            let working_width = image_sequence.width;
            let working_height = image_sequence.height;
            Ok(Some(PreparedFrames {
                input_type: ProjectInputType::Images,
                video: None,
                image_sequence: Some(image_sequence),
                plan,
                extracted_frames,
                image_format: "images".into(),
                mask_count: prepared.mask_count,
                has_alpha: prepared.has_alpha,
                working_width,
                working_height,
            }))
        }
    }
}

fn brush_candidate(root: &Path) -> Option<PathBuf> {
    [root.join("final.ply.tmp"), root.join("final.ply.tmp.ply")]
        .into_iter()
        .find(|path| path.is_file())
}

async fn recover_interrupted_publish(
    paths: &ProjectPaths,
    state: &PipelineStateFile,
) -> Result<()> {
    if state.stage == PipelineStage::Completed
        || !state.brush_complete
        || brush_candidate(&paths.brush).is_some()
    {
        return Ok(());
    }
    let orphan = paths.project.join("final.ply");
    if !orphan.is_file() {
        return Ok(());
    }
    let inspect_path = orphan.clone();
    if tokio::task::spawn_blocking(move || inspect_gaussian_ply(&inspect_path))
        .await
        .map_err(|error| SplatError::Process(format!("PLY 恢复校验任务失败：{error}")))?
        .is_err()
    {
        return Ok(());
    }
    tokio::fs::create_dir_all(&paths.brush).await?;
    atomic_replace_file(&orphan, &paths.brush.join("final.ply.tmp")).await
}

async fn reset_directory(path: &Path) -> Result<()> {
    if path.exists() {
        tokio::fs::remove_dir_all(path).await?;
    }
    tokio::fs::create_dir_all(path).await?;
    Ok(())
}

#[derive(Debug)]
struct ReusableReconstruction {
    model: PathBuf,
    report: ReconstructionReport,
}

#[derive(Debug)]
struct ReusableModelCandidate {
    model: PathBuf,
    report: ReconstructionReport,
    candidate_id: Option<String>,
}

async fn reusable_reconstruction(
    project: &Path,
    frames: &Path,
    output: Option<&ProjectOutput>,
) -> Result<ReusableReconstruction> {
    let project = project.to_path_buf();
    let frames = frames.to_path_buf();
    let output = output.cloned();
    tokio::task::spawn_blocking(move || {
        reusable_reconstruction_blocking(&project, &frames, output.as_ref())
    })
    .await
    .map_err(|error| SplatError::Process(format!("读取可复用重建模型失败：{error}")))?
}

fn reusable_reconstruction_blocking(
    project: &Path,
    frames: &Path,
    output: Option<&ProjectOutput>,
) -> Result<ReusableReconstruction> {
    let colmap = project.join("work/colmap");
    let selected_candidate = selected_reconstruction_candidate(project)?;
    let mut models = Vec::new();

    collect_reusable_models(frames, &colmap.join("sparse"), None, &mut models)?;
    let candidates = colmap.join("candidates");
    if candidates.is_dir()
        && !std::fs::symlink_metadata(&candidates)?
            .file_type()
            .is_symlink()
    {
        for entry in std::fs::read_dir(&candidates)? {
            let entry = entry?;
            if entry.file_type()?.is_symlink() || !entry.file_type()?.is_dir() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().into_owned();
            if !safe_candidate_id(&id) {
                continue;
            }
            collect_reusable_models(frames, &entry.path(), Some(id), &mut models)?;
        }
    }

    if models.is_empty() {
        return Err(SplatError::Process(
            "原项目缺少可复用的 COLMAP 稀疏重建模型（已检查 sparse 与 candidates）".into(),
        ));
    }
    models.sort_by(|left, right| left.model.cmp(&right.model));

    let selected_models = selected_candidate.as_deref().map(|selected| {
        models
            .iter()
            .enumerate()
            .filter(|(_, candidate)| candidate.candidate_id.as_deref() == Some(selected))
            .map(|(index, _)| index)
            .collect::<Vec<_>>()
    });
    let pool = selected_models
        .as_ref()
        .filter(|indices| !indices.is_empty())
        .cloned()
        .unwrap_or_else(|| (0..models.len()).collect());
    let exact = output.map(|output| {
        pool.iter()
            .copied()
            .filter(|index| {
                let report = &models[*index].report;
                report.registered_images == output.registered_images
                    && report.points_3d == output.points_3d
            })
            .collect::<Vec<_>>()
    });
    let pool = exact
        .as_ref()
        .filter(|indices| !indices.is_empty())
        .unwrap_or(&pool);
    let best = pool
        .iter()
        .copied()
        .max_by(|left, right| compare_reusable_models(&models[*left], &models[*right]))
        .expect("a non-empty model pool");
    let selected = models.swap_remove(best);
    Ok(ReusableReconstruction {
        model: selected.model,
        report: selected.report,
    })
}

fn compare_reusable_models(
    left: &ReusableModelCandidate,
    right: &ReusableModelCandidate,
) -> std::cmp::Ordering {
    left.report
        .registered_images
        .cmp(&right.report.registered_images)
        .then_with(|| left.report.points_3d.cmp(&right.report.points_3d))
        .then_with(|| right.model.cmp(&left.model))
}

fn collect_reusable_models(
    frames: &Path,
    root: &Path,
    candidate_id: Option<String>,
    output: &mut Vec<ReusableModelCandidate>,
) -> Result<()> {
    if !root.is_dir() {
        return Ok(());
    }
    if std::fs::symlink_metadata(root)?.file_type().is_symlink() {
        return Ok(());
    }
    if let Some(report) = validate_reusable_model(frames, root) {
        output.push(ReusableModelCandidate {
            model: root.to_path_buf(),
            report,
            candidate_id: candidate_id.clone(),
        });
    }
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() || !file_type.is_dir() {
            continue;
        }
        let path = entry.path();
        if let Some(report) = validate_reusable_model(frames, &path) {
            output.push(ReusableModelCandidate {
                model: path,
                report,
                candidate_id: candidate_id.clone(),
            });
        }
    }
    Ok(())
}

fn validate_reusable_model(frames: &Path, model: &Path) -> Option<ReconstructionReport> {
    reusable_model_files(model).ok()?;
    ReconstructionValidator::validate(frames, model).ok()
}

fn selected_reconstruction_candidate(project: &Path) -> Result<Option<String>> {
    for path in [
        project.join("state.json"),
        project.join("logs/planner.json"),
    ] {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        let selected = value
            .pointer("/planner/bestReconstructionId")
            .or_else(|| value.pointer("/bestReconstructionId"))
            .and_then(serde_json::Value::as_str);
        let Some(selected) = selected else {
            continue;
        };
        if !safe_candidate_id(selected) {
            return Err(SplatError::Process(
                "项目记录的 COLMAP 候选模型标识不安全，已拒绝访问".into(),
            ));
        }
        return Ok(Some(selected.to_owned()));
    }
    Ok(None)
}

fn safe_candidate_id(value: &str) -> bool {
    let mut components = Path::new(value).components();
    matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none()
        && value != "."
        && value != ".."
        && !value.contains(['/', '\\', ':'])
}

fn validate_database_camera(database: &Path, camera: &ColmapCamera) -> Result<()> {
    use rusqlite::OptionalExtension;

    let connection = rusqlite::Connection::open_with_flags(
        database,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| SplatError::Process(format!("无法读取原项目 COLMAP 数据库：{error}")))?;
    let stored = connection
        .query_row(
            "SELECT model, width, height FROM cameras WHERE camera_id = ?1",
            [camera.id],
            |row| {
                Ok((
                    row.get::<_, i32>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|error| SplatError::Process(format!("校验原项目相机数据库失败：{error}")))?;
    let Some((model, width, height)) = stored else {
        return Err(SplatError::Process(format!(
            "原项目 COLMAP 数据库缺少相机 ID {}",
            camera.id
        )));
    };
    if model != camera.model_id || width != camera.width as i64 || height != camera.height as i64 {
        return Err(SplatError::Process(
            "原项目 COLMAP 数据库与最终稀疏模型的相机信息不一致".into(),
        ));
    }
    Ok(())
}

fn model_snapshot_matches(source_files: &[PathBuf], snapshot: &Path) -> bool {
    source_files.iter().all(|source| {
        let Some(name) = source.file_name() else {
            return false;
        };
        let destination = snapshot.join(name);
        match (std::fs::metadata(source), std::fs::metadata(destination)) {
            (Ok(source), Ok(destination)) => {
                source.is_file() && destination.is_file() && source.len() == destination.len()
            }
            _ => false,
        }
    })
}

async fn best_sparse_model(
    frames: &Path,
    sparse: &Path,
) -> Result<(PathBuf, ReconstructionReport)> {
    let frames = frames.to_path_buf();
    let sparse = sparse.to_path_buf();
    tokio::task::spawn_blocking(move || best_sparse_model_blocking(&frames, &sparse))
        .await
        .map_err(|error| SplatError::Process(format!("稀疏模型校验任务失败：{error}")))?
}

async fn best_sparse_model_with_input_count(
    sparse: &Path,
    input_images: u64,
) -> Result<(PathBuf, ReconstructionReport)> {
    let sparse = sparse.to_path_buf();
    tokio::task::spawn_blocking(move || {
        best_sparse_model_with_input_count_blocking(&sparse, input_images)
    })
    .await
    .map_err(|error| SplatError::Process(format!("Sparse model validation task failed: {error}")))?
}

fn best_sparse_model_blocking(
    frames: &Path,
    sparse: &Path,
) -> Result<(PathBuf, ReconstructionReport)> {
    // COLMAP normally writes numbered children such as `sparse/0`, while
    // continuation from `--input_path` may write directly to the requested
    // directory. Accept both layouts and keep the model with most registered
    // images.
    let mut best = ReconstructionValidator::validate(frames, sparse)
        .ok()
        .map(|report| (sparse.to_path_buf(), report));
    for entry in std::fs::read_dir(sparse)? {
        let path = entry?.path();
        if !path.is_dir() {
            continue;
        }
        if let Ok(report) = ReconstructionValidator::validate(frames, &path) {
            if best
                .as_ref()
                .is_none_or(|(_, current)| report.registered_images > current.registered_images)
            {
                best = Some((path, report));
            }
        }
    }
    best.ok_or_else(|| SplatError::Process("COLMAP 未生成完整的稀疏模型".into()))
}

fn best_sparse_model_with_input_count_blocking(
    sparse: &Path,
    input_images: u64,
) -> Result<(PathBuf, ReconstructionReport)> {
    let mut best = ReconstructionValidator::validate_with_input_images(input_images, sparse)
        .ok()
        .map(|report| (sparse.to_path_buf(), report));
    for entry in std::fs::read_dir(sparse)? {
        let path = entry?.path();
        if !path.is_dir() {
            continue;
        }
        if let Ok(report) = ReconstructionValidator::validate_with_input_images(input_images, &path)
        {
            if best
                .as_ref()
                .is_none_or(|(_, current)| report.registered_images > current.registered_images)
            {
                best = Some((path, report));
            }
        }
    }
    best.ok_or_else(|| SplatError::Process("COLMAP did not produce a usable sparse model".into()))
}

fn relative_model_path(paths: &ProjectPaths, model: &Path) -> String {
    model
        .strip_prefix(&paths.colmap)
        .unwrap_or(model)
        .to_string_lossy()
        .replace('\\', "/")
}

fn update_frame_checkpoint(state: &mut PipelineStateFile, prepared: &PreparedFrames) {
    if let Some(frames) = state.frames.as_mut() {
        frames.extracted_frames = Some(prepared.extracted_frames);
        frames.estimated_frames = prepared.extracted_frames;
        frames.retention_ratio = prepared.plan.retention_ratio;
        frames.sampling_fps = prepared.plan.sampling_fps;
        frames.selected_frames = prepared.plan.selected_frames.clone();
        frames.candidate_frames = prepared.plan.candidate_frames.clone();
        frames.rescue_max_frames = prepared.plan.rescue_max_frames;
        frames.minimum_frame_override_applied = prepared.plan.minimum_frame_override_applied;
        frames.mask_count = Some(prepared.mask_count);
    }
}

fn oriented_video_dimensions(video: &VideoInfo) -> (u32, u32) {
    if video.rotation.rem_euclid(180) == 90 {
        (video.height, video.width)
    } else {
        (video.width, video.height)
    }
}

async fn remove_bridge_outputs(
    frames: &Path,
    masks: &Path,
    additional: &[PlannedFrame],
    has_alpha: bool,
) {
    let extension = if has_alpha { "png" } else { "jpg" };
    for frame in additional {
        let name = format!("frame_{:010}.{extension}", frame.source_frame_index);
        let _ = tokio::fs::remove_file(frames.join(&name)).await;
        if has_alpha {
            let _ = tokio::fs::remove_file(masks.join(format!("{name}.png"))).await;
        }
    }
}

async fn prepare_brush_dataset(root: &Path, frames: &Path, model: &Path) -> Result<PathBuf> {
    let dataset = root.join("dataset");
    let images = dataset.join("images");
    let sparse = dataset.join("sparse").join("0");
    tokio::fs::create_dir_all(&images).await?;
    tokio::fs::create_dir_all(&sparse).await?;
    let mut entries = tokio::fs::read_dir(frames).await?;
    while let Some(entry) = entries.next_entry().await? {
        let source = entry.path();
        if !source.is_file() {
            continue;
        }
        let destination = images.join(entry.file_name());
        if tokio::fs::hard_link(&source, &destination).await.is_err() {
            tokio::fs::copy(&source, &destination).await?;
        }
    }
    for name in ["cameras.bin", "images.bin", "points3D.bin"] {
        tokio::fs::copy(model.join(name), sparse.join(name)).await?;
    }
    Ok(dataset)
}

pub fn default_engine_paths(engine_root: Option<PathBuf>) -> EnginePaths {
    engine_root
        .map(EnginePaths::from_root)
        .unwrap_or_else(|| EnginePaths::discover(None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reshoot_project_name_uses_the_short_suffix() {
        assert_eq!(reshoot_project_name("示例项目"), "示例项目_补拍");
    }

    #[test]
    fn reshoot_inherits_source_planner_settings() {
        for planner_enabled in [false, true] {
            let mut source = PipelineStateFile::created(Quality::Balanced);
            source.planner_enabled = planner_enabled;
            source.resolution_policy_version = planner_enabled.then_some(7);
            let mut target = PipelineStateFile::created(Quality::Balanced);
            target.planner_enabled = !planner_enabled;
            target.resolution_policy_version = (!planner_enabled).then_some(3);

            inherit_reshoot_planner_settings(&source, &mut target);

            assert_eq!(target.planner_enabled, planner_enabled);
            assert_eq!(
                target.resolution_policy_version,
                source.resolution_policy_version
            );
        }
    }

    #[tokio::test]
    async fn image_preparation_emits_monotonic_phase_progress() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source");
        let frames = temporary.path().join("frames");
        let masks = temporary.path().join("masks");
        std::fs::create_dir_all(&source).unwrap();
        image::RgbImage::new(2, 2)
            .save(source.join("1.jpg"))
            .unwrap();
        image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 0]))
            .save(source.join("2.png"))
            .unwrap();
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = events.clone();
        let runner = PipelineRunner::new(EnginePaths::from_root(temporary.path()), move |event| {
            captured.lock().unwrap().push(event);
        });

        let prepared = runner
            .prepare_images(&source, Quality::Balanced, &frames, &masks, false)
            .await
            .unwrap();

        assert!(prepared.has_alpha);
        let events = events.lock().unwrap();
        assert!(events
            .iter()
            .any(|event| event.message.starts_with("正在建立画面链接或复制")));
        assert!(events
            .iter()
            .any(|event| event.message.starts_with("正在检测 Alpha 并生成 Mask")));
        assert!(events
            .iter()
            .any(|event| event.message == "图片与 Mask 完整性校验完成"));
        let progress = events
            .iter()
            .filter(|event| event.stage == PipelineStage::ExtractingFrames)
            .map(|event| event.stage_progress.unwrap_or_default())
            .collect::<Vec<_>>();
        assert!(
            progress.windows(2).all(|pair| pair[0] <= pair[1]),
            "stage progress regressed: {progress:?}"
        );
        assert!(events.iter().any(|event| {
            event.unit.as_deref() == Some("images")
                && event.current.is_some()
                && event.total.is_some()
        }));
    }

    fn write_validator_model(root: &Path, registered: u64, points: u64) {
        std::fs::create_dir_all(root).unwrap();
        for (name, count) in [
            ("cameras.bin", 1_u64),
            ("images.bin", registered),
            ("points3D.bin", points),
        ] {
            let mut bytes = count.to_le_bytes().to_vec();
            bytes.extend_from_slice(&[0_u8; 8]);
            std::fs::write(root.join(name), bytes).unwrap();
        }
    }

    #[test]
    fn incremental_sparse_model_is_found_at_the_output_root() {
        let temporary = tempfile::tempdir().unwrap();
        let frames = temporary.path().join("frames");
        let sparse = temporary.path().join("sparse");
        std::fs::create_dir_all(&frames).unwrap();
        std::fs::write(frames.join("frame_000001.jpg"), b"one").unwrap();
        std::fs::write(frames.join("reshoot_000001.png"), b"two").unwrap();
        write_validator_model(&sparse, 2, 50);

        let (model, report) = best_sparse_model_blocking(&frames, &sparse).unwrap();

        assert_eq!(model, sparse);
        assert_eq!(report.registered_images, 2);
        assert_eq!(report.points_3d, 50);
    }

    #[test]
    fn fresh_sparse_model_is_still_found_in_a_numbered_child() {
        let temporary = tempfile::tempdir().unwrap();
        let frames = temporary.path().join("frames");
        let sparse = temporary.path().join("sparse");
        std::fs::create_dir_all(&frames).unwrap();
        std::fs::write(frames.join("frame_000001.jpg"), b"one").unwrap();
        write_validator_model(&sparse.join("0"), 1, 25);

        let (model, report) = best_sparse_model_blocking(&frames, &sparse).unwrap();

        assert_eq!(model, sparse.join("0"));
        assert_eq!(report.registered_images, 1);
    }

    #[test]
    fn reusable_reconstruction_prefers_the_planner_selected_candidate() {
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path();
        let frames = project.join("work/frames");
        std::fs::create_dir_all(&frames).unwrap();
        for index in 0..3 {
            std::fs::write(frames.join(format!("frame_{index}.jpg")), b"frame").unwrap();
        }
        let selected = project.join("work/colmap/candidates/selected/1");
        let other = project.join("work/colmap/candidates/other/0");
        write_validator_model(&selected, 2, 20);
        write_validator_model(&other, 3, 30);
        std::fs::write(
            project.join("state.json"),
            br#"{"planner":{"bestReconstructionId":"selected"}}"#,
        )
        .unwrap();
        let output = ProjectOutput {
            registered_images: 3,
            points_3d: 30,
            ..Default::default()
        };

        let resolved = reusable_reconstruction_blocking(project, &frames, Some(&output)).unwrap();

        assert_eq!(resolved.model, selected);
        assert_eq!(resolved.report.registered_images, 2);
    }

    #[test]
    fn reusable_reconstruction_uses_output_metrics_without_a_planner_record() {
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path();
        let frames = project.join("work/frames");
        std::fs::create_dir_all(&frames).unwrap();
        for index in 0..3 {
            std::fs::write(frames.join(format!("frame_{index}.jpg")), b"frame").unwrap();
        }
        let expected = project.join("work/colmap/candidates/expected/0");
        let larger = project.join("work/colmap/candidates/larger/0");
        write_validator_model(&expected, 2, 25);
        write_validator_model(&larger, 3, 40);
        let output = ProjectOutput {
            registered_images: 2,
            points_3d: 25,
            ..Default::default()
        };

        let resolved = reusable_reconstruction_blocking(project, &frames, Some(&output)).unwrap();

        assert_eq!(resolved.model, expected);
    }

    #[test]
    fn reusable_reconstruction_rejects_an_unsafe_candidate_id() {
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path();
        let frames = project.join("work/frames");
        let sparse = project.join("work/colmap/sparse/0");
        std::fs::create_dir_all(&frames).unwrap();
        std::fs::write(frames.join("frame.jpg"), b"frame").unwrap();
        write_validator_model(&sparse, 1, 10);
        std::fs::write(
            project.join("state.json"),
            br#"{"planner":{"bestReconstructionId":"../outside"}}"#,
        )
        .unwrap();

        let error = reusable_reconstruction_blocking(project, &frames, None).unwrap_err();

        assert!(error.to_string().contains("不安全"));
    }

    #[test]
    fn reusable_reconstruction_reports_missing_models_without_an_io_error() {
        let temporary = tempfile::tempdir().unwrap();
        let frames = temporary.path().join("work/frames");
        std::fs::create_dir_all(&frames).unwrap();

        let error = reusable_reconstruction_blocking(temporary.path(), &frames, None).unwrap_err();

        let message = error.to_string();
        assert!(message.contains("缺少可复用"));
        assert!(!message.contains("os error"));
    }

    #[test]
    fn reshoot_database_camera_must_match_the_selected_model() {
        let temporary = tempfile::tempdir().unwrap();
        let database = temporary.path().join("database.db");
        let connection = rusqlite::Connection::open(&database).unwrap();
        connection
            .execute(
                "CREATE TABLE cameras (camera_id INTEGER PRIMARY KEY, model INTEGER NOT NULL, width INTEGER NOT NULL, height INTEGER NOT NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO cameras(camera_id, model, width, height) VALUES (1, 2, 1920, 1080)",
                [],
            )
            .unwrap();
        drop(connection);
        let camera = ColmapCamera {
            id: 1,
            model_id: 2,
            model: "SIMPLE_RADIAL".into(),
            width: 1920,
            height: 1080,
        };

        validate_database_camera(&database, &camera).unwrap();
        assert!(validate_database_camera(
            &database,
            &ColmapCamera {
                width: 1080,
                ..camera
            },
        )
        .is_err());
    }

    #[test]
    fn model_snapshot_requires_every_source_model_file_at_the_same_size() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source");
        let snapshot = temporary.path().join("snapshot");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&snapshot).unwrap();
        let source_files = [
            "cameras.bin",
            "images.bin",
            "points3D.bin",
            "rigs.bin",
            "frames.bin",
        ]
        .map(|name| {
            let path = source.join(name);
            std::fs::write(&path, name.as_bytes()).unwrap();
            std::fs::write(snapshot.join(name), name.as_bytes()).unwrap();
            path
        });
        assert!(model_snapshot_matches(&source_files, &snapshot));

        std::fs::write(snapshot.join("frames.bin"), b"truncated").unwrap();
        assert!(!model_snapshot_matches(&source_files, &snapshot));
    }

    #[tokio::test]
    async fn reshoot_rejects_the_original_video_by_content() {
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path().join("project");
        let stored = project.join("source/input.mov");
        let original = temporary.path().join("original.mov");
        let fresh = temporary.path().join("fresh.mov");
        tokio::fs::create_dir_all(stored.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&stored, b"same-video").await.unwrap();
        tokio::fs::write(&original, b"same-video").await.unwrap();
        tokio::fs::write(&fresh, b"new-video!").await.unwrap();

        assert!(
            ensure_reshoot_input_is_new(&project, &stored, &original, ReshootInputType::Video,)
                .await
                .is_err()
        );
        assert!(
            ensure_reshoot_input_is_new(&project, &stored, &fresh, ReshootInputType::Video,)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn reshoot_rejects_the_original_image_sequence_by_content() {
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path().join("project");
        let stored = project.join("source/images");
        let original = temporary.path().join("original-images");
        let fresh = temporary.path().join("fresh-images");
        let mixed = temporary.path().join("mixed-images");
        for directory in [&stored, &original, &fresh, &mixed] {
            tokio::fs::create_dir_all(directory).await.unwrap();
        }
        for (name, bytes) in [("1.jpg", b"one".as_slice()), ("2.png", b"two".as_slice())] {
            tokio::fs::write(stored.join(name), bytes).await.unwrap();
            tokio::fs::write(original.join(name), bytes).await.unwrap();
            tokio::fs::write(fresh.join(name), bytes).await.unwrap();
        }
        tokio::fs::write(fresh.join("1.jpg"), b"fresh-one")
            .await
            .unwrap();
        tokio::fs::write(fresh.join("2.png"), b"fresh-two")
            .await
            .unwrap();
        tokio::fs::write(mixed.join("1.jpg"), b"one").await.unwrap();
        tokio::fs::write(mixed.join("2.png"), b"fresh-two")
            .await
            .unwrap();

        assert!(ensure_reshoot_input_is_new(
            &project,
            &stored,
            &original,
            ReshootInputType::Images,
        )
        .await
        .is_err());
        assert!(
            ensure_reshoot_input_is_new(&project, &stored, &mixed, ReshootInputType::Images,)
                .await
                .is_err()
        );
        assert!(
            ensure_reshoot_input_is_new(&project, &stored, &fresh, ReshootInputType::Images,)
                .await
                .is_ok()
        );
    }

    #[test]
    fn parses_ffmpeg_progress() {
        assert_eq!(parse_ffmpeg_frame("frame=127"), Some(127));
        assert_eq!(parse_ffmpeg_frame("progress=continue"), None);
    }

    #[test]
    fn reports_the_latest_durable_checkpoint_instead_of_terminal_status() {
        let mut state = PipelineStateFile::created(Quality::Balanced);
        state.stage = PipelineStage::Cancelled;
        assert_eq!(checkpoint_stage(&state), PipelineStage::Created);

        state.frames = Some(FrameState {
            retention_ratio: 0.5,
            sampling_fps: 15.0,
            estimated_frames: 100,
            extracted_frames: Some(100),
            image_format: Some("jpeg".into()),
            mask_count: Some(0),
            has_alpha: false,
            ..FrameState::default()
        });
        state.features_complete = true;
        state.matching_complete = true;
        assert_eq!(checkpoint_stage(&state), PipelineStage::Matching);

        state.reconstruction_complete = true;
        state.brush_complete = true;
        assert_eq!(checkpoint_stage(&state), PipelineStage::TrainingSplats);
    }

    #[test]
    fn terminal_state_preserves_every_checkpoint() {
        let mut state = PipelineStateFile::created(Quality::Balanced);
        state.frames = Some(FrameState {
            retention_ratio: 0.5,
            sampling_fps: 15.0,
            estimated_frames: 100,
            extracted_frames: Some(100),
            image_format: Some("jpeg".into()),
            mask_count: Some(0),
            has_alpha: false,
            ..FrameState::default()
        });
        state.features_complete = true;
        state.matching_complete = true;
        state.reconstruction_complete = true;
        state.brush_complete = true;

        let failed = mark_state_terminal(state.clone(), false);
        let cancelled = mark_state_terminal(state, true);
        for terminal in [failed, cancelled] {
            assert_eq!(
                terminal.frames.as_ref().unwrap().extracted_frames,
                Some(100)
            );
            assert!(terminal.features_complete);
            assert!(terminal.matching_complete);
            assert!(terminal.reconstruction_complete);
            assert!(terminal.brush_complete);
        }
    }

    #[tokio::test]
    async fn frame_checkpoint_requires_every_recorded_frame() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::nil(), temporary.path().to_path_buf());
        tokio::fs::create_dir_all(&paths.frames).await.unwrap();
        image::RgbImage::new(2, 2)
            .save(paths.frames.join("frame_000001.jpg"))
            .unwrap();
        image::RgbImage::new(2, 2)
            .save(paths.frames.join("frame_000002.jpg"))
            .unwrap();
        let mut state = PipelineStateFile::created(Quality::Balanced);
        state.video = Some(VideoInfo {
            duration: 1.0,
            width: 1920,
            height: 1080,
            fps: 30.0,
            total_frames: 30,
            codec: "h264".into(),
            rotation: 0,
            pixel_format: "yuv420p".into(),
            has_alpha: false,
        });
        state.frames = Some(FrameState {
            retention_ratio: 0.5,
            sampling_fps: 15.0,
            estimated_frames: 2,
            extracted_frames: Some(2),
            image_format: Some("jpeg".into()),
            mask_count: Some(0),
            has_alpha: false,
            ..FrameState::default()
        });

        assert!(prepared_frames_from_checkpoint(&paths, &state)
            .await
            .unwrap()
            .is_some());
        tokio::fs::remove_file(paths.frames.join("frame_000002.jpg"))
            .await
            .unwrap();
        assert!(prepared_frames_from_checkpoint(&paths, &state)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn interrupted_publish_restores_a_valid_orphan_as_a_brush_checkpoint() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::nil(), temporary.path().to_path_buf());
        let valid = b"ply\nformat binary_little_endian 1.0\nelement vertex 1\nproperty float x\nproperty float y\nproperty float z\nproperty float f_dc_0\nproperty float opacity\nproperty float scale_0\nproperty float rot_0\nend_header\n";
        tokio::fs::write(paths.project.join("final.ply"), valid)
            .await
            .unwrap();
        let mut state = PipelineStateFile::created(Quality::Balanced);
        state.brush_complete = true;

        recover_interrupted_publish(&paths, &state).await.unwrap();

        assert!(!paths.project.join("final.ply").exists());
        assert!(paths.brush.join("final.ply.tmp").is_file());
    }

    #[tokio::test]
    async fn image_sequence_checkpoint_requires_every_image_and_mask() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::nil(), temporary.path().to_path_buf());
        tokio::fs::create_dir_all(&paths.frames).await.unwrap();
        tokio::fs::create_dir_all(&paths.masks).await.unwrap();
        for name in ["frame_000001.png", "frame_000002.jpg"] {
            tokio::fs::write(paths.frames.join(name), b"image")
                .await
                .unwrap();
            tokio::fs::write(paths.masks.join(format!("{name}.png")), b"mask")
                .await
                .unwrap();
        }
        let mut state = PipelineStateFile::created_for(Quality::Balanced, ProjectInputType::Images);
        state.image_sequence = Some(ImageSequenceInfo {
            image_count: 2,
            width: 1920,
            height: 1080,
            has_alpha: true,
            requires_large_sequence_confirmation: false,
        });
        state.frames = Some(FrameState {
            retention_ratio: 1.0,
            sampling_fps: 0.0,
            estimated_frames: 2,
            extracted_frames: Some(2),
            image_format: Some("images".into()),
            mask_count: Some(2),
            has_alpha: true,
            ..FrameState::default()
        });

        assert!(prepared_frames_from_checkpoint(&paths, &state)
            .await
            .unwrap()
            .is_some());
        tokio::fs::remove_file(paths.masks.join("frame_000002.jpg.png"))
            .await
            .unwrap();
        assert!(prepared_frames_from_checkpoint(&paths, &state)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn opaque_alpha_channel_sequence_reuses_a_maskless_checkpoint() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::nil(), temporary.path().to_path_buf());
        tokio::fs::create_dir_all(&paths.frames).await.unwrap();
        for name in ["frame_000001.png", "frame_000002.png"] {
            tokio::fs::write(paths.frames.join(name), b"image")
                .await
                .unwrap();
        }
        let mut state = PipelineStateFile::created_for(Quality::Balanced, ProjectInputType::Images);
        state.image_sequence = Some(ImageSequenceInfo {
            image_count: 2,
            width: 1920,
            height: 1080,
            has_alpha: true,
            requires_large_sequence_confirmation: false,
        });
        state.frames = Some(FrameState {
            retention_ratio: 1.0,
            sampling_fps: 0.0,
            estimated_frames: 2,
            extracted_frames: Some(2),
            image_format: Some("images".into()),
            mask_count: Some(0),
            has_alpha: false,
            ..FrameState::default()
        });

        let prepared = prepared_frames_from_checkpoint(&paths, &state)
            .await
            .unwrap()
            .expect("opaque alpha-channel images should reuse the maskless checkpoint");

        assert!(!prepared.has_alpha);
        assert_eq!(prepared.mask_count, 0);
    }

    #[tokio::test]
    async fn transparent_frame_checkpoint_requires_matching_masks() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::nil(), temporary.path().to_path_buf());
        tokio::fs::create_dir_all(&paths.frames).await.unwrap();
        tokio::fs::create_dir_all(&paths.masks).await.unwrap();
        image::RgbaImage::new(2, 2)
            .save(paths.frames.join("frame_000001.png"))
            .unwrap();
        image::GrayImage::new(2, 2)
            .save(paths.masks.join("frame_000001.png.png"))
            .unwrap();
        let mut state = PipelineStateFile::created(Quality::Balanced);
        state.video = Some(VideoInfo {
            duration: 1.0,
            width: 1920,
            height: 1080,
            fps: 30.0,
            total_frames: 30,
            codec: "prores".into(),
            rotation: 0,
            pixel_format: "yuva444p10le".into(),
            has_alpha: true,
        });
        state.frames = Some(FrameState {
            retention_ratio: 0.5,
            sampling_fps: 15.0,
            estimated_frames: 1,
            extracted_frames: Some(1),
            image_format: Some("png".into()),
            mask_count: Some(1),
            has_alpha: true,
            ..FrameState::default()
        });

        let prepared = prepared_frames_from_checkpoint(&paths, &state)
            .await
            .unwrap()
            .unwrap();
        assert!(prepared.has_alpha);
        assert_eq!(prepared.mask_count, 1);

        tokio::fs::remove_file(paths.masks.join("frame_000001.png.png"))
            .await
            .unwrap();
        assert!(prepared_frames_from_checkpoint(&paths, &state)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn empty_colmap_database_downgrades_the_feature_checkpoint() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::nil(), temporary.path().to_path_buf());
        tokio::fs::create_dir_all(&paths.frames).await.unwrap();
        tokio::fs::create_dir_all(&paths.colmap).await.unwrap();
        image::RgbImage::new(2, 2)
            .save(paths.frames.join("frame_000001.jpg"))
            .unwrap();
        tokio::fs::write(paths.colmap.join("database.db"), b"")
            .await
            .unwrap();
        let mut state = PipelineStateFile::created(Quality::Balanced);
        state.video = Some(VideoInfo {
            duration: 1.0,
            width: 1920,
            height: 1080,
            fps: 30.0,
            total_frames: 30,
            codec: "h264".into(),
            rotation: 0,
            pixel_format: "yuv420p".into(),
            has_alpha: false,
        });
        state.frames = Some(FrameState {
            retention_ratio: 0.5,
            sampling_fps: 15.0,
            estimated_frames: 1,
            extracted_frames: Some(1),
            image_format: Some("jpeg".into()),
            mask_count: Some(0),
            has_alpha: false,
            ..FrameState::default()
        });
        state.features_complete = true;
        state.matching_complete = true;

        normalize_checkpoints(&paths, &mut state).await.unwrap();

        assert!(!state.features_complete);
        assert!(!state.matching_complete);
        assert_eq!(state.stage, PipelineStage::ExtractingFrames);
    }

    #[test]
    fn parses_colmap_file_progress() {
        assert_eq!(
            parse_bracket_progress("Processed file [23/533]"),
            Some((23, 533))
        );
        assert_eq!(
            parse_bracket_progress("Processing image [4/10]"),
            Some((4, 10))
        );
    }

    #[test]
    fn parses_mapper_registration() {
        let counter = AtomicU64::new(0);
        let value = parse_mapper_progress(
            "Registering image #90 (num_reg_frames=86)",
            &counter,
            Some(100),
        )
        .unwrap();
        assert_eq!(value.0, 86);
        assert_eq!(counter.load(Ordering::Relaxed), 86);
    }

    #[test]
    fn mapper_refinement_keeps_the_latest_registered_count() {
        let counter = AtomicU64::new(0);
        parse_mapper_progress(
            "Registering image #90 (num_reg_frames=86)",
            &counter,
            Some(100),
        )
        .unwrap();

        let retriangulation = parse_mapper_progress(
            "Retriangulation and Global bundle adjustment",
            &counter,
            Some(100),
        )
        .unwrap();
        assert_eq!(retriangulation.0, 86);
        assert_eq!(retriangulation.1, Some(100));
        assert_eq!(
            retriangulation.2,
            "Retriangulation and Global bundle adjustment"
        );

        let bundle_adjustment =
            parse_mapper_progress("Global bundle adjustment", &counter, Some(100)).unwrap();
        assert_eq!(bundle_adjustment.0, 86);
        assert_eq!(bundle_adjustment.1, Some(100));
    }

    #[test]
    fn mapper_refinement_without_a_registration_count_stays_indeterminate() {
        let counter = AtomicU64::new(0);
        assert!(parse_mapper_progress(
            "Retriangulation and Global bundle adjustment",
            &counter,
            Some(100),
        )
        .is_none());
    }

    #[test]
    fn mapper_registration_count_only_moves_forward() {
        let counter = AtomicU64::new(0);
        parse_mapper_progress("num_reg_frames=86", &counter, Some(100)).unwrap();
        parse_mapper_progress("num_reg_frames=91", &counter, Some(100)).unwrap();
        parse_mapper_progress("num_reg_frames=89", &counter, Some(100)).unwrap();

        let value = parse_mapper_progress(
            "Retriangulation and Global bundle adjustment",
            &counter,
            Some(100),
        )
        .unwrap();
        assert_eq!(value.0, 91);
    }

    #[test]
    fn brush_heartbeat_never_invents_completed_steps() {
        let received = Arc::new(std::sync::Mutex::new(Vec::new()));
        let output = received.clone();
        let runner = PipelineRunner::new(
            EnginePaths::from_root(PathBuf::from("engines")),
            move |event| output.lock().unwrap().push(event),
        );
        let observer = runner.process_observer(
            PipelineStage::TrainingSplats,
            PipelineEngine::Brush,
            Some(100),
            ObserverMode::Brush,
        );
        observer(ProcessUpdate::Started { process_id: 42 });
        observer(ProcessUpdate::Heartbeat {
            elapsed_ms: 500_000,
        });
        let events = received.lock().unwrap();
        assert!(events.iter().all(|event| event.current.is_none()));
        assert!(events
            .iter()
            .filter_map(|event| event.runtime.as_ref())
            .all(|runtime| runtime.training.is_none()));
    }

    #[test]
    fn brush_progress_and_retry_use_actual_output_and_fresh_state() {
        let received = Arc::new(std::sync::Mutex::new(Vec::new()));
        let output = received.clone();
        let runner = PipelineRunner::new(
            EnginePaths::from_root(PathBuf::from("engines")),
            move |event| output.lock().unwrap().push(event),
        );
        let observer = runner.process_observer(
            PipelineStage::TrainingSplats,
            PipelineEngine::Brush,
            Some(100),
            ObserverMode::Brush,
        );
        observer(ProcessUpdate::Started { process_id: 42 });
        for line in [
            "Training config: total_iters=120 start_iter=20",
            "Training progress: iteration=60 total=120 elapsed_secs=1 lod=0",
            "[INFO brush_process] Export started: iteration 60, final.ply",
        ] {
            observer(ProcessUpdate::Line {
                stream: crate::process::ProcessStream::Stdout,
                line: line.into(),
            });
        }
        {
            let events = received.lock().unwrap();
            let progress = events
                .iter()
                .find(|event| event.kind == EventKind::Progress)
                .unwrap();
            assert_eq!(progress.current, Some(60));
            assert_eq!(progress.total, Some(120));
            assert!(progress.progress < 100.0);
            assert_eq!(
                events
                    .iter()
                    .filter_map(|event| event.runtime.as_ref())
                    .next_back()
                    .unwrap()
                    .phase,
                "exporting"
            );
        }

        let retry = runner.process_observer(
            PipelineStage::TrainingSplats,
            PipelineEngine::Brush,
            Some(100),
            ObserverMode::Brush,
        );
        retry(ProcessUpdate::Started { process_id: 43 });
        let events = received.lock().unwrap();
        let snapshot = events
            .iter()
            .filter_map(|event| event.runtime.as_ref())
            .next_back()
            .unwrap();
        assert_eq!(snapshot.process_id, 43);
        assert!(snapshot.training.is_none());
        assert!(snapshot.resources.is_none());
    }

    #[test]
    fn brush_oom_fallback_is_high_only_and_can_be_used_once() {
        let resolved =
            resolve_brush_training_preset(Quality::High, true, Some(12_288), 7_680, 60_000);
        let fallback = brush_oom_fallback(Quality::High, true, false, resolved, 60_000).unwrap();
        assert_eq!(
            fallback.profile,
            crate::presets::BrushTrainingProfile::HighStandard
        );
        assert!(brush_oom_fallback(Quality::High, true, true, fallback, 60_000).is_none());
        assert!(brush_oom_fallback(Quality::Balanced, true, false, resolved, 60_000).is_none());
        assert!(brush_oom_fallback(Quality::High, false, false, resolved, 60_000).is_none());
    }

    #[test]
    fn event_sequence_is_strictly_increasing() {
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = events.clone();
        let sink = EventSink {
            emit: Arc::new(move |event| captured.lock().unwrap().push(event.sequence)),
            sequence: Arc::new(AtomicU64::new(0)),
            last_progress_milli_percent: Arc::new(AtomicU64::new(0)),
            last_stage: Arc::new(std::sync::Mutex::new(None)),
            dispatch: Arc::new(std::sync::Mutex::new(())),
            started: Instant::now(),
        };
        sink.stage(PipelineStage::Created, 0.0, "created");
        sink.stage(PipelineStage::ProbingVideo, 0.0, "probing");
        assert_eq!(*events.lock().unwrap(), vec![1, 2]);
    }

    #[test]
    fn terminal_event_keeps_the_last_real_progress_and_clears_stage_progress() {
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = events.clone();
        let sink = EventSink {
            emit: Arc::new(move |event| captured.lock().unwrap().push(event)),
            sequence: Arc::new(AtomicU64::new(0)),
            last_progress_milli_percent: Arc::new(AtomicU64::new(0)),
            last_stage: Arc::new(std::sync::Mutex::new(None)),
            dispatch: Arc::new(std::sync::Mutex::new(())),
            started: Instant::now(),
        };
        sink.stage(PipelineStage::TrainingSplats, 0.5, "training");
        sink.terminal(&SplatError::Process("boom".into()));

        let events = events.lock().unwrap();
        assert_eq!(events[0].progress, 79.0);
        assert_eq!(events[1].progress, 79.0);
        assert_eq!(events[1].stage_progress, None);
        assert_eq!(events[1].stage, PipelineStage::Failed);
        assert_eq!(
            *sink.last_stage.lock().unwrap(),
            Some(PipelineStage::TrainingSplats)
        );
    }
}
