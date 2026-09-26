use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Instant,
};

use chrono::Utc;
use serde::Serialize;

use crate::{
    engines::{
        brush, colmap,
        ffmpeg::{
            extract_additional_frames, extract_selected_frames, extract_uniform_frames,
            scan_frame_candidates, validate_extraction,
        },
        ffprobe::probe_video,
        EngineKind, EnginePaths,
    },
    error::{Result, SplatError},
    pipeline::{
        estimate::{
            estimate_calibrated_brush_stage_ms, estimate_calibrated_brush_stage_ms_for_images,
        },
        progress::stage_progress_range,
        EventKind, EventLevel, PipelineEngine, PipelineEvent, PipelineStage,
    },
    planner::{
        analyze_sparse_geometry, can_start_new_probe, geometry_probe_budget,
        geometry_probe_reasons, parse_model_analyzer, plan_geometry_probe_backfill,
        probe_candidate_acceptable, screening_applicable, BudgetFramePlanner, CaptureAnalysis,
        CaptureAnalyzer, CapturePrior, EstimatedMatchingCost, FailureAnalyzer, FrameCandidate,
        FramePlanner, GeometryProbeMetrics, GeometryProbeStatus, GeometryScreeningDecision,
        GeometryScreeningThresholds, GraphDecision, GraphQualityGate, MapperBackend, MapperPlan,
        MinimumCapturePolicy, PairingPlan, PairingPlanner, PairingStrategy, PairingThresholds,
        PlannerCheckpoint, PlannerReconstructionValidator, PlannerRecoveryMode,
        ReconstructionCandidate, ReconstructionComparator, ReconstructionDecision,
        ReconstructionViability, RescueAction, RescueRecord, SuccessRecoveryPolicy,
        ViewGraphAnalyzer,
    },
    presets::{BrushResolutionContext, MatchingBudget, Quality, ResolvedBrushBudget, SfmBudget},
    process::{ProcessManager, ProcessObserver, ProcessUpdate},
    project::{
        catalog, manager::atomic_replace_file, FrameState, PipelineStateFile,
        ProjectImportObserver, ProjectInputType, ProjectManager, ProjectMetadata, ProjectOutput,
        ProjectPaths, ProjectStatus, QualityRunMetrics,
    },
    reconstruction::{
        ply::inspect_gaussian_ply,
        validator::{
            validate_sparse_geometry, ReconstructionQuality, ReconstructionReport,
            ReconstructionValidator,
        },
    },
    video::{
        prepare_scanned_image_sequence, scan_image_sequence, validate_prepared_image_sequence,
        FramePlan, FramePlanningMode, FrameSelectionStrategy, ImagePreparationObserver,
        ImagePreparationPhase, ImageSequenceInfo, UniformRatioFrameSelection, VideoInfo,
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
    pub capture_prior: Option<CapturePrior>,
}

#[derive(Debug, Clone, Copy, Default)]
struct FramePreparationMode<'a> {
    planner_enabled: bool,
    saved_plan: Option<&'a FramePlan>,
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
    pub quality_metrics: QualityRunMetrics,
    pub duration_ms: u64,
    pub completed_at: chrono::DateTime<Utc>,
    pub warning: Option<String>,
    pub logs_directory: PathBuf,
    #[serde(skip)]
    pub(crate) source_duration_seconds: Option<f64>,
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
    project_path: PathBuf,
    logs_directory: PathBuf,
}

pub struct PipelineRunner {
    engines: EnginePaths,
    process_manager: ProcessManager,
    events: EventSink,
    active_project: Arc<std::sync::Mutex<Option<ActiveProjectContext>>>,
    planner_enabled: bool,
}

impl PipelineRunner {
    pub fn new(engines: EnginePaths, emit: impl Fn(PipelineEvent) + Send + Sync + 'static) -> Self {
        Self::new_with_planner(engines, false, emit)
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
            planner_enabled,
        }
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

    pub async fn verify_pipeline_engines(
        &self,
    ) -> Result<crate::engines::ColmapAccelerationStatus> {
        let statuses = self.engines.check_all().await;
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
        colmap::require_vocabulary_tree(&self.engines.colmap_vocab_tree)?;
        brush::require_verified_cli(&self.engines.brush)?;
        statuses
            .into_iter()
            .find(|status| status.kind == EngineKind::Colmap)
            .and_then(|status| status.acceleration)
            .ok_or_else(|| SplatError::UnsupportedEngine("无法确定 COLMAP 自动加速状态".into()))
    }

    pub async fn prepare_frames(
        &self,
        input: &Path,
        quality: Quality,
        output: &Path,
        masks: &Path,
        logs: Option<&Path>,
    ) -> Result<PreparedFrames> {
        self.prepare_frames_with_mode(
            input,
            quality,
            output,
            masks,
            logs,
            FramePreparationMode::default(),
        )
        .await
    }

    async fn prepare_frames_with_mode(
        &self,
        input: &Path,
        quality: Quality,
        output: &Path,
        masks: &Path,
        logs: Option<&Path>,
        mode: FramePreparationMode<'_>,
    ) -> Result<PreparedFrames> {
        self.events
            .stage(PipelineStage::ProbingVideo, 0.0, "正在读取视频信息");
        let video = probe_video(
            &self.engines.ffprobe,
            input,
            logs.map(|path| path.join("ffprobe.log")),
            &self.process_manager,
        )
        .await?;
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

        let planning_message = if mode.saved_plan.is_some() {
            "正在恢复已保存的画面计划"
        } else if mode.planner_enabled {
            "Planner 正在根据质量预算选择画面"
        } else {
            "正在规划均匀抽帧"
        };
        self.events
            .stage(PipelineStage::PlanningFrames, 0.0, planning_message);
        let mut capture_prior = None;
        let plan = if let Some(saved_plan) = mode.saved_plan {
            saved_plan.clone()
        } else if mode.planner_enabled {
            self.events
                .stage(PipelineStage::PlanningFrames, 0.08, "正在分析拍摄素材");
            let candidates = scan_frame_candidates(
                &self.engines.ffmpeg,
                input,
                &video,
                MinimumCapturePolicy::default()
                    .analysis_fps(&video, quality.budget().frame.analysis_fps),
                &self.process_manager,
                Some({
                    let events = self.events.clone();
                    Arc::new(move |current, total| {
                        let ratio = current as f32 / total.max(1) as f32;
                        events.send(
                            PipelineStage::PlanningFrames,
                            Some(PipelineEngine::Ffmpeg),
                            EventKind::Progress,
                            EventLevel::Info,
                            Some(0.08 + ratio.clamp(0.0, 1.0) * 0.52),
                            false,
                            format!("正在分析拍摄素材 {current}/{total} 帧"),
                            Some(current),
                            Some(total),
                            Some("frames"),
                        );
                    })
                }),
            )
            .await?;
            let prior = CaptureAnalyzer.analyze_video(&candidates);
            let activity = candidates
                .iter()
                .map(|candidate| {
                    (candidate.motion_score + candidate.view_change_score) as f64 * 0.5
                })
                .sum::<f64>()
                / candidates.len().max(1) as f64;
            if let Some(parent) = output.parent() {
                let planner_dir = parent.join("planner");
                tokio::fs::create_dir_all(&planner_dir).await?;
                let bytes = serde_json::to_vec(&candidates).map_err(|error| {
                    SplatError::Process(format!("Unable to cache frame candidates: {error}"))
                })?;
                tokio::fs::write(planner_dir.join("frame-candidates.json"), bytes).await?;
            }
            capture_prior = Some(prior.clone());
            self.events
                .stage(PipelineStage::PlanningFrames, 0.62, "正在选择合适的画面");
            BudgetFramePlanner
                .plan(
                    &video,
                    quality,
                    CaptureAnalysis {
                        activity,
                        prior,
                        candidates,
                    },
                )
                .map_err(|error| SplatError::Process(format!("画面规划失败：{error}")))?
        } else {
            UniformRatioFrameSelection.create_plan(&video, quality)
        };
        let minimum = &plan.minimum_frame_protection;
        let planning_complete_message = if minimum.minimum_frame_target_unreachable {
            format!(
                "最低 {} 帧目标不可达，将尽量保留可用画面（计划 {} 帧）",
                minimum.minimum_frame_target, plan.estimated_frames
            )
        } else if minimum.minimum_frame_override_applied {
            format!(
                "短视频最低帧数保护已启用：目标 {} 帧，有效采样 {:.2} fps",
                minimum.minimum_frame_target, plan.target_fps
            )
        } else {
            format!("预计提取 {} 帧", plan.estimated_frames)
        };
        self.events.stage(
            PipelineStage::PlanningFrames,
            1.0,
            planning_complete_message,
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
        let extraction = if plan.planning_mode == FramePlanningMode::Budgeted {
            extract_selected_frames(
                &self.engines.ffmpeg,
                input,
                output,
                masks,
                &plan,
                video.has_alpha,
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
                logs.map(|path| path.join("ffmpeg.log")),
                &self.process_manager,
                Some(observer),
            )
            .await?
        };
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
            capture_prior,
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
        Ok(PreparedFrames {
            input_type: ProjectInputType::Images,
            video: None,
            image_sequence: Some(image_sequence),
            plan,
            extracted_frames: prepared.image_count,
            image_format: "images".into(),
            mask_count: prepared.mask_count,
            has_alpha: prepared.has_alpha,
            capture_prior: Some(CaptureAnalyzer.unordered_images(prepared.image_count)),
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
        let mut state = project_manager.read_state(&paths.state).await?;
        state.planner_enabled = self.planner_enabled;
        state.planner = self
            .planner_enabled
            .then(|| PlannerCheckpoint::new(quality.budget(), SuccessRecoveryPolicy::default()));
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
        let result = self
            .run_project(&project_manager, &paths, metadata, state, acceleration)
            .await;

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

    async fn run_project(
        &self,
        project_manager: &ProjectManager,
        paths: &ProjectPaths,
        metadata: &mut ProjectMetadata,
        mut state: PipelineStateFile,
        acceleration: &crate::engines::ColmapAccelerationStatus,
    ) -> Result<PipelineResult> {
        let quality = metadata.quality;
        let budget = quality.budget();
        let colmap_tuning = colmap::ColmapQualityTuning::for_run(
            quality,
            colmap::ColmapQualityTuning::requested_from_environment(),
        );
        if state.input_type != metadata.input_type {
            return Err(SplatError::Process(
                "项目输入类型与检查点不一致，无法安全继续".into(),
            ));
        }
        recover_interrupted_publish(paths, &state).await?;
        normalize_checkpoints(paths, &mut state).await?;
        let has_colmap_checkpoint = state.features_complete
            || state.matching_complete
            || state.reconstruction_complete
            || state.brush_complete;
        match state.colmap_high_quality_experiment {
            Some(saved) if saved != colmap_tuning.enabled && has_colmap_checkpoint => {
                return Err(SplatError::Process(format!(
                    "COLMAP High Quality 实验开关与项目检查点不一致：检查点为 {saved}，当前为 {}。请使用新项目进行 A/B 测试。",
                    colmap_tuning.enabled
                )));
            }
            None if has_colmap_checkpoint && colmap_tuning.enabled => {
                return Err(SplatError::Process(
                    "旧项目检查点未记录 COLMAP High Quality 实验状态，不能在恢复时直接开启实验；请创建新项目。".into(),
                ));
            }
            None => state.colmap_high_quality_experiment = Some(colmap_tuning.enabled),
            _ => {}
        }
        if state.planner_enabled && state.planner.is_none() {
            state.planner = Some(PlannerCheckpoint::new(
                budget.clone(),
                SuccessRecoveryPolicy::default(),
            ));
        }
        project_manager.write_state(&paths.state, &state).await?;
        let mut prepared =
            if let Some(prepared) = prepared_frames_from_checkpoint(paths, &state).await? {
                self.events.stage(
                    PipelineStage::ExtractingFrames,
                    1.0,
                    format!("已复用 {} 帧检查点", prepared.extracted_frames),
                );
                prepared
            } else {
                let saved_frame_plan = state.frames.as_ref().map(frame_plan_from_state);
                reset_directory(&paths.frames).await?;
                reset_directory(&paths.masks).await?;
                reset_directory(&paths.colmap).await?;
                reset_directory(&paths.brush).await?;
                let prepared = match metadata.input_type {
                    ProjectInputType::Video => {
                        self.prepare_frames_with_mode(
                            &metadata.source_path,
                            quality,
                            &paths.frames,
                            &paths.masks,
                            Some(&paths.logs),
                            FramePreparationMode {
                                planner_enabled: state.planner_enabled,
                                saved_plan: saved_frame_plan.as_ref(),
                            },
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
                let mut frames = FrameState::from(&prepared.plan);
                frames.extracted_frames = Some(prepared.extracted_frames);
                frames.image_format = Some(prepared.image_format.clone());
                frames.mask_count = Some(prepared.mask_count);
                frames.has_alpha = prepared.has_alpha;
                state.frames = Some(frames);
                if let Some(planner) = state.planner.as_mut() {
                    planner.capture_prior = prepared.capture_prior.clone();
                    planner.frame_plan = Some(prepared.plan.clone());
                    planner.actual_selected_frames = prepared
                        .plan
                        .selected_frames
                        .iter()
                        .map(|frame| frame.source_frame_index)
                        .collect();
                }
                state.features_complete = false;
                state.matching_complete = false;
                state.reconstruction_complete = false;
                state.brush_complete = false;
                state.stage = PipelineStage::ExtractingFrames;
                project_manager.write_state(&paths.state, &state).await?;
                write_planner_snapshot(paths, &state).await?;
                prepared
            };
        let source_duration_seconds = prepared.video.as_ref().map(|video| video.duration);
        let sfm_source_size = prepared
            .video
            .as_ref()
            .map(|video| (video.width, video.height))
            .or_else(|| {
                prepared
                    .image_sequence
                    .as_ref()
                    .map(|images| (images.width, images.height))
            })
            .ok_or_else(|| SplatError::Process("项目输入尺寸信息不完整".into()))?;

        let database = paths.colmap.join("database.db");
        let sparse = paths.colmap.join("sparse");
        let colmap_log = paths.logs.join("colmap.log");
        // COLMAP's bundled bitmap loader cannot reliably open non-ASCII absolute
        // paths on Windows. The process working directory is work/colmap, so this
        // ASCII-only relative path preserves Unicode/UNC project roots without
        // moving any project data outside the project directory.
        let colmap_images = Path::new("../frames");
        let colmap_masks = prepared.has_alpha.then_some(Path::new("../masks"));
        let sfm_budget = budget
            .baseline
            .sfm
            .capped_for_source(sfm_source_size.0, sfm_source_size.1);

        self.events.send(
            PipelineStage::ExtractingFeatures,
            Some(PipelineEngine::Colmap),
            EventKind::Log,
            EventLevel::Info,
            None,
            false,
            colmap_tuning.log_summary(sfm_budget),
            None,
            None,
            None,
        );

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
                colmap::extract_features(
                    &self.engines.colmap,
                    &database,
                    colmap_images,
                    colmap_masks,
                    colmap_log.clone(),
                    &self.process_manager,
                    observer,
                    gpu_index,
                    sfm_budget,
                    colmap_tuning,
                )
                .await?;
            } else {
                colmap::extract_features_legacy(
                    &self.engines.colmap,
                    &database,
                    colmap_images,
                    colmap_masks,
                    colmap_log.clone(),
                    &self.process_manager,
                    observer,
                    gpu_index,
                    colmap_tuning,
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
        if let Ok(feature_metrics) = colmap::analyze_database(&database) {
            self.events.send(
                PipelineStage::ExtractingFeatures,
                Some(PipelineEngine::Colmap),
                EventKind::Log,
                EventLevel::Info,
                Some(1.0),
                false,
                format!(
                    "特征统计：图片 {}，总特征 {}，平均 {:.2}/图，中位数 {:.1}/图",
                    feature_metrics.image_count,
                    feature_metrics.total_detected_features,
                    feature_metrics.mean_features_per_image,
                    feature_metrics.median_features_per_image,
                ),
                None,
                None,
                None,
            );
        }

        let pairing_plan = if state.planner_enabled {
            if let Some(saved) = state
                .planner
                .as_ref()
                .and_then(|planner| planner.pairing_plan.clone())
            {
                saved
            } else {
                let prior = state
                    .planner
                    .as_ref()
                    .and_then(|planner| planner.capture_prior.clone())
                    .or_else(|| prepared.capture_prior.clone())
                    .unwrap_or_default();
                let plan = PairingPlanner::default().plan(
                    &prior,
                    prepared.extracted_frames,
                    budget.baseline.matching,
                );
                if let Some(planner) = state.planner.as_mut() {
                    planner.capture_prior = Some(prior);
                    planner.pairing_plan = Some(plan.clone());
                }
                project_manager.write_state(&paths.state, &state).await?;
                plan
            }
        } else {
            PairingPlan {
                strategy: if prepared.input_type == ProjectInputType::Images {
                    PairingStrategy::Exhaustive
                } else {
                    PairingStrategy::Sequential
                },
                sequential_overlap: 10,
                prefilter_neighbors: 10,
                estimated_pairs: 0,
                reason_codes: vec!["legacy_main_baseline".into()],
            }
        };
        let pairing_label = match pairing_plan.strategy {
            PairingStrategy::Exhaustive => "穷举匹配",
            PairingStrategy::Sequential => "顺序匹配",
            PairingStrategy::SequentialWithLoopClosure => "带回环的顺序匹配",
            PairingStrategy::Prefilter => "预筛选匹配",
        };

        if state.matching_complete {
            self.events.stage(
                PipelineStage::Matching,
                1.0,
                format!("已复用{pairing_label}检查点"),
            );
        } else {
            self.events.stage(
                PipelineStage::Matching,
                0.0,
                format!("COLMAP 正在进行 {backend_label} {pairing_label}"),
            );
            let observer = Some(self.process_observer(
                PipelineStage::Matching,
                PipelineEngine::Colmap,
                Some(prepared.extracted_frames),
                ObserverMode::BracketProgress,
            ));
            match pairing_plan.strategy {
                PairingStrategy::Exhaustive => {
                    colmap::match_exhaustive(
                        &self.engines.colmap,
                        &database,
                        colmap_log.clone(),
                        &self.process_manager,
                        observer,
                        gpu_index,
                        colmap_tuning,
                    )
                    .await?
                }
                PairingStrategy::Sequential => {
                    colmap::match_sequential(
                        &self.engines.colmap,
                        &database,
                        colmap_log.clone(),
                        &self.process_manager,
                        observer,
                        gpu_index,
                        MatchingBudget {
                            sequential_overlap: pairing_plan.sequential_overlap,
                            prefilter_neighbors: pairing_plan.prefilter_neighbors,
                        },
                        colmap_tuning,
                    )
                    .await?
                }
                PairingStrategy::SequentialWithLoopClosure => {
                    colmap::match_sequential_with_loop(
                        &self.engines.colmap,
                        &database,
                        &self.engines.colmap_vocab_tree,
                        colmap_log.clone(),
                        &self.process_manager,
                        observer,
                        gpu_index,
                        budget.baseline.matching,
                        colmap_tuning,
                    )
                    .await?
                }
                PairingStrategy::Prefilter => {
                    colmap::match_prefilter(
                        &self.engines.colmap,
                        &database,
                        &self.engines.colmap_vocab_tree,
                        colmap_log.clone(),
                        &self.process_manager,
                        observer,
                        gpu_index,
                        pairing_plan.prefilter_neighbors,
                        colmap_tuning,
                    )
                    .await?
                }
            }
            state.stage = PipelineStage::Matching;
            state.matching_complete = true;
            if let Some(planner) = state.planner.as_mut() {
                planner.pairing_actual = Some(pairing_plan.strategy);
            }
            project_manager.write_state(&paths.state, &state).await?;
            self.events
                .stage(PipelineStage::Matching, 1.0, format!("{pairing_label}完成"));
        }
        let database_metrics = colmap::analyze_database(&database).ok();
        if let Some(database_metrics) = &database_metrics {
            self.events.send(
                PipelineStage::Matching,
                Some(PipelineEngine::Colmap),
                EventKind::Log,
                EventLevel::Info,
                Some(1.0),
                false,
                format!(
                    "匹配统计：raw pairs {}，raw matches {}，verified pairs {}，verified correspondences {}",
                    database_metrics.raw_match_pairs,
                    database_metrics.raw_matches,
                    database_metrics.geometrically_verified_pairs,
                    database_metrics.verified_correspondences,
                ),
                None,
                None,
                None,
            );
        }

        let (model, report, planner_candidate, actual_frame_count) = if state.planner_enabled {
            let (model, report, candidate, actual_frame_count) = self
                .reconstruct_with_planner(
                    project_manager,
                    paths,
                    &mut state,
                    &database,
                    colmap_images,
                    &colmap_log,
                    gpu_index,
                    prepared.extracted_frames,
                    &pairing_plan,
                    &metadata.source_path,
                    prepared.has_alpha,
                    sfm_budget,
                    colmap_tuning,
                )
                .await?;
            (model, report, Some(candidate), actual_frame_count)
        } else {
            if state.reconstruction_complete {
                self.events
                    .stage(PipelineStage::Reconstructing, 1.0, "已复用相机重建检查点");
            } else {
                reset_directory(&sparse).await?;
                self.events
                    .stage(PipelineStage::Reconstructing, 0.0, "正在增量重建相机轨迹");
                colmap::map(
                    &self.engines.colmap,
                    &database,
                    colmap_images,
                    &sparse,
                    colmap_log.clone(),
                    &self.process_manager,
                    Some(self.process_observer(
                        PipelineStage::Reconstructing,
                        PipelineEngine::Colmap,
                        Some(prepared.extracted_frames),
                        ObserverMode::Mapper,
                    )),
                    colmap_tuning,
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
            let (model, report) = best_sparse_model(&paths.frames, &sparse).await?;
            (model, report, None, prepared.extracted_frames)
        };
        prepared.extracted_frames = actual_frame_count;
        let warning = if let Some(candidate) = &planner_candidate {
            (candidate.decision != ReconstructionDecision::Pass).then(|| {
                if candidate.viability == ReconstructionViability::DegradedButViable {
                    "已选择降级但可用的重建结果，将继续 Brush".to_owned()
                } else {
                    format!(
                        "注册率 {:.1}%：将使用当前最佳可用重建继续训练",
                        report.registered_ratio * 100.0
                    )
                }
            })
        } else {
            (report.quality == ReconstructionQuality::Warning).then(|| {
                format!(
                    "注册率 {:.1}%：低于 80%，将继续训练，但结果质量可能受影响",
                    report.registered_ratio * 100.0
                )
            })
        };
        self.events.stage(
            PipelineStage::ValidatingReconstruction,
            1.0,
            format!(
                "注册 {}/{} 张 · 三维点 {}",
                report.registered_images, report.input_images, report.points_3d
            ),
        );

        let (source_width, source_height) = prepared
            .video
            .as_ref()
            .map(|video| (video.width, video.height))
            .or_else(|| {
                prepared
                    .image_sequence
                    .as_ref()
                    .map(|images| (images.width, images.height))
            })
            .ok_or_else(|| SplatError::Process("项目输入尺寸信息不完整".into()))?;
        let brush_budget = if state.planner_enabled {
            budget.baseline.brush.resolve(BrushResolutionContext::new(
                source_width,
                source_height,
                prepared.extracted_frames,
                acceleration
                    .device
                    .as_ref()
                    .and_then(|device| device.total_memory_mb),
            ))
        } else {
            ResolvedBrushBudget {
                iterations: budget.baseline.brush.iterations,
                max_resolution: budget.baseline.brush.resolution.estimate_max_resolution(),
            }
        };
        let candidate = if state.brush_complete {
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
            let runtime_samples = catalog::runtime_samples().await;
            let estimated_brush_duration_ms = match (&prepared.video, &prepared.image_sequence) {
                (Some(video), _) => estimate_calibrated_brush_stage_ms(
                    video,
                    &prepared.plan,
                    quality,
                    &runtime_samples,
                ),
                (_, Some(images)) => estimate_calibrated_brush_stage_ms_for_images(
                    images.image_count,
                    &prepared.plan,
                    quality,
                    &runtime_samples,
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
                    brush_budget.iterations,
                    brush_budget.max_resolution,
                    format_duration(estimated_brush_duration_ms)
                ),
                Some(0),
                Some(brush_budget.iterations as u64),
                Some("iterations"),
            );
            let candidate = brush::train(
                &self.engines.brush,
                &dataset,
                &paths.brush,
                brush_budget,
                paths.logs.join("brush.log"),
                &self.process_manager,
                Some(self.process_observer(
                    PipelineStage::TrainingSplats,
                    PipelineEngine::Brush,
                    Some(brush_budget.iterations as u64),
                    ObserverMode::Brush {
                        estimated_duration_ms: estimated_brush_duration_ms,
                    },
                )),
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
        let quality_metrics = QualityRunMetrics {
            actual_frame_count: prepared.extracted_frames,
            actual_sfm_resolution: if state.planner_enabled {
                source_width
                    .max(source_height)
                    .min(budget.baseline.sfm.max_image_size)
            } else {
                source_width.max(source_height).min(1_920)
            },
            actual_feature_count: database_metrics
                .as_ref()
                .map(|metrics| metrics.total_detected_features),
            mean_features_per_image: database_metrics
                .as_ref()
                .map(|metrics| metrics.mean_features_per_image),
            median_features_per_image: database_metrics
                .as_ref()
                .map(|metrics| metrics.median_features_per_image),
            raw_matches: database_metrics.as_ref().map(|metrics| metrics.raw_matches),
            geometrically_verified_pairs: database_metrics
                .as_ref()
                .map(|metrics| metrics.geometrically_verified_pairs),
            verified_correspondences: database_metrics
                .as_ref()
                .map(|metrics| metrics.verified_correspondences),
            colmap_high_quality_experiment: colmap_tuning.enabled,
            actual_brush_resolution: brush_budget.max_resolution,
            actual_brush_iterations: brush_budget.iterations,
            registered_images: report.registered_images,
            reprojection_error: planner_candidate
                .as_ref()
                .and_then(|candidate| candidate.metrics.mean_reprojection_error),
            splat_count: ply.splat_count,
            // Stage durations are emitted by PipelineTelemetrySession.
            stage_durations_ms: Default::default(),
            // Total VRAM is not a peak-memory measurement.
            peak_gpu_memory_mb: None,
            planner_enabled: state.planner_enabled,
            planner_version: state
                .planner
                .as_ref()
                .map(|planner| planner.planner_version),
            capture_type: state
                .planner
                .as_ref()
                .and_then(|planner| planner.capture_prior.as_ref())
                .map(|prior| format!("{:?}", prior.capture_type).to_ascii_lowercase()),
            pairing_planned: state
                .planner
                .as_ref()
                .and_then(|planner| planner.pairing_plan.as_ref())
                .map(|plan| format!("{:?}", plan.strategy).to_ascii_lowercase()),
            pairing_actual: state
                .planner
                .as_ref()
                .and_then(|planner| planner.pairing_actual)
                .map(|value| format!("{value:?}").to_ascii_lowercase()),
            mapper_planned: state
                .planner
                .as_ref()
                .and_then(|planner| planner.mapper_plan.as_ref())
                .map(|plan| format!("{:?}", plan.backend).to_ascii_lowercase()),
            mapper_actual: state
                .planner
                .as_ref()
                .and_then(|planner| planner.mapper_actual)
                .map(|value| format!("{value:?}").to_ascii_lowercase()),
            largest_component_ratio: state
                .planner
                .as_ref()
                .and_then(|planner| planner.view_graph_report.as_ref())
                .map(|graph| graph.largest_component_ratio),
            two_core_ratio: state
                .planner
                .as_ref()
                .and_then(|planner| planner.view_graph_report.as_ref())
                .map(|graph| graph.two_core_ratio),
            bridge_ratio: state
                .planner
                .as_ref()
                .and_then(|planner| planner.view_graph_report.as_ref())
                .map(|graph| graph.bridge_ratio),
            normal_rescue_rounds: state
                .planner
                .as_ref()
                .map(|planner| {
                    planner
                        .rescue_history
                        .iter()
                        .filter(|record| record.mode == PlannerRecoveryMode::Normal)
                        .count() as u32
                })
                .unwrap_or(0),
            success_recovery_rounds: state
                .planner
                .as_ref()
                .map(|planner| {
                    planner
                        .rescue_history
                        .iter()
                        .filter(|record| record.mode == PlannerRecoveryMode::SuccessRecovery)
                        .count() as u32
                })
                .unwrap_or(0),
            normal_budget_exhausted: state
                .planner
                .as_ref()
                .is_some_and(|planner| planner.normal_budget_exhausted),
            success_recovery_entered: state
                .planner
                .as_ref()
                .is_some_and(|planner| planner.success_recovery_entered),
            budget_overridden_for_success: state
                .planner
                .as_ref()
                .is_some_and(|planner| planner.budget_overridden_for_success),
            normal_duration_ms: state
                .planner
                .as_ref()
                .map(|planner| planner.normal_duration_ms)
                .unwrap_or(0),
            recovery_duration_ms: state
                .planner
                .as_ref()
                .map(|planner| planner.recovery_duration_ms)
                .unwrap_or(0),
            reconstruction_quality: planner_candidate.as_ref().map(|candidate| {
                match candidate.viability {
                    ReconstructionViability::Viable => {
                        if candidate.decision == ReconstructionDecision::Pass {
                            "normal"
                        } else {
                            "warning"
                        }
                    }
                    ReconstructionViability::DegradedButViable => "degraded",
                    ReconstructionViability::NotViable => "failed",
                }
                .to_owned()
            }),
            geometry_screening: state
                .planner
                .as_ref()
                .and_then(|planner| planner.geometry_screening_report.clone()),
            geometry_probe: state
                .planner
                .as_ref()
                .and_then(|planner| planner.geometry_probe_metrics.clone()),
        };
        metadata.output = Some(ProjectOutput {
            final_ply: final_ply.clone(),
            file_size: ply.file_size,
            splat_count: ply.splat_count,
            input_images: report.input_images,
            registered_images: report.registered_images,
            registered_ratio: report.registered_ratio,
            points_3d: report.points_3d,
            quality_metrics: quality_metrics.clone(),
        });
        project_manager
            .write_metadata(&paths.metadata, metadata)
            .await?;

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
            quality_metrics,
            duration_ms,
            completed_at,
            warning,
            logs_directory: paths.logs.clone(),
            source_duration_seconds,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn reconstruct_with_planner(
        &self,
        project_manager: &ProjectManager,
        paths: &ProjectPaths,
        state: &mut PipelineStateFile,
        database: &Path,
        images: &Path,
        colmap_log: &Path,
        gpu_index: Option<u32>,
        input_images: u64,
        pairing_plan: &PairingPlan,
        source_video: &Path,
        has_alpha: bool,
        sfm_budget: SfmBudget,
        colmap_tuning: colmap::ColmapQualityTuning,
    ) -> Result<(PathBuf, ReconstructionReport, ReconstructionCandidate, u64)> {
        let mut input_images = input_images;
        let planner_started = Instant::now();
        let mut recovery_started = None;
        if state.reconstruction_complete {
            if let Some(planner) = &state.planner {
                if let Some(best_id) = &planner.best_reconstruction_id {
                    if let Some(candidate) = planner
                        .reconstruction_candidates
                        .iter()
                        .find(|candidate| &candidate.id == best_id)
                    {
                        if let Ok(report) =
                            ReconstructionValidator::validate(&paths.frames, &candidate.model_path)
                        {
                            self.events.stage(
                                PipelineStage::Reconstructing,
                                1.0,
                                "已复用 Planner 最佳重建候选",
                            );
                            return Ok((
                                candidate.model_path.clone(),
                                report,
                                candidate.clone(),
                                input_images,
                            ));
                        }
                    }
                }
            }
        }

        self.events
            .stage(PipelineStage::Matching, 0.92, "正在分析图像关系");
        let mut graph = ViewGraphAnalyzer.analyze_database(database)?;
        let mut graph_decision = GraphQualityGate::default().decide(&graph);
        let normal_budget = state
            .planner
            .as_ref()
            .and_then(|planner| planner.quality_budget_snapshot.as_ref())
            .map(|budget| budget.extension.rescue)
            .unwrap_or_else(|| state.preset.budget().extension.rescue);
        let mut normal_rounds = state
            .planner
            .as_ref()
            .map(|planner| {
                planner
                    .rescue_history
                    .iter()
                    .filter(|record| record.mode == PlannerRecoveryMode::Normal)
                    .count() as u32
            })
            .unwrap_or(0);

        if graph_decision == GraphDecision::NeedRescue
            && normal_budget.allow_frame_backfill
            && normal_rounds < normal_budget.max_rounds
            && source_video.is_file()
        {
            if let Some(mut full_plan) = state.frames.as_ref().map(frame_plan_from_state) {
                let old_selected: std::collections::HashSet<u64> = full_plan
                    .selected_frames
                    .iter()
                    .map(|frame| frame.source_frame_index)
                    .collect();
                let remaining = full_plan
                    .candidate_frames
                    .iter()
                    .filter(|frame| !old_selected.contains(&frame.source_frame_index))
                    .count();
                let additional = ((full_plan.selected_frames.len() as f32 * 0.25).ceil() as usize)
                    .max(1)
                    .min(remaining);
                if additional > 0
                    && BudgetFramePlanner
                        .backfill(&mut full_plan, additional)
                        .is_ok()
                {
                    let additions: Vec<_> = full_plan
                        .selected_frames
                        .iter()
                        .filter(|frame| !old_selected.contains(&frame.source_frame_index))
                        .cloned()
                        .collect();
                    let mut addition_plan = full_plan.clone();
                    addition_plan.selected_frames = additions.clone();
                    addition_plan.estimated_frames = additions.len() as u64;
                    self.events.stage(
                        PipelineStage::ExtractingFrames,
                        0.80,
                        "正在为薄弱区域补充画面",
                    );
                    let extraction = extract_additional_frames(
                        &self.engines.ffmpeg,
                        source_video,
                        &paths.frames,
                        &paths.masks,
                        &addition_plan,
                        has_alpha,
                        Some(paths.logs.join("ffmpeg-backfill.log")),
                        &self.process_manager,
                        Some(self.process_observer(
                            PipelineStage::ExtractingFrames,
                            PipelineEngine::Ffmpeg,
                            Some(additions.len() as u64),
                            ObserverMode::Ffmpeg,
                        )),
                    )
                    .await?;
                    input_images = extraction.frame_count;
                    let extension = if has_alpha { "png" } else { "jpg" };
                    let names: Vec<String> = additions
                        .iter()
                        .map(|frame| format!("frame_{:010}.{extension}", frame.source_frame_index))
                        .collect();
                    let image_list = paths.colmap.join("planner-backfill-images.txt");
                    tokio::fs::write(&image_list, names.join("\n")).await?;
                    colmap::extract_features_for_list(
                        &self.engines.colmap,
                        database,
                        images,
                        has_alpha.then_some(Path::new("../masks")),
                        &image_list,
                        paths.logs.join("colmap-backfill.log"),
                        &self.process_manager,
                        Some(self.process_observer(
                            PipelineStage::ExtractingFeatures,
                            PipelineEngine::Colmap,
                            Some(additions.len() as u64),
                            ObserverMode::BracketProgress,
                        )),
                        gpu_index,
                        sfm_budget,
                        colmap_tuning,
                    )
                    .await?;
                    let pair_list = paths.colmap.join("planner-backfill-pairs.txt");
                    write_backfill_pairs(
                        &paths.frames,
                        &names,
                        pairing_plan.sequential_overlap.max(4) as usize,
                        &pair_list,
                    )
                    .await?;
                    colmap::match_pairs(
                        &self.engines.colmap,
                        database,
                        &pair_list,
                        paths.logs.join("colmap-backfill.log"),
                        &self.process_manager,
                        Some(self.process_observer(
                            PipelineStage::Matching,
                            PipelineEngine::Colmap,
                            Some(additions.len() as u64),
                            ObserverMode::BracketProgress,
                        )),
                        gpu_index,
                        colmap_tuning,
                    )
                    .await?;
                    let previous_lcc = graph.largest_component_ratio;
                    graph = ViewGraphAnalyzer.analyze_database(database)?;
                    graph_decision = GraphQualityGate::default().decide(&graph);
                    normal_rounds += 1;
                    if let Some(frames) = state.frames.as_mut() {
                        *frames = FrameState::from(&full_plan);
                        frames.extracted_frames = Some(input_images);
                        frames.image_format = Some(if has_alpha { "png" } else { "jpeg" }.into());
                        frames.mask_count = Some(if has_alpha { input_images } else { 0 });
                        frames.has_alpha = has_alpha;
                    }
                    if let Some(planner) = state.planner.as_mut() {
                        planner.frame_plan = Some(full_plan.clone());
                        planner.actual_selected_frames = full_plan
                            .selected_frames
                            .iter()
                            .map(|frame| frame.source_frame_index)
                            .collect();
                        planner.rescue_history.push(RescueRecord {
                            round: normal_rounds,
                            mode: PlannerRecoveryMode::Normal,
                            action: RescueAction::FrameBackfill,
                            effective: Some(graph.largest_component_ratio > previous_lcc + 0.005),
                            reason_codes: vec!["weak_temporal_region".into()],
                        });
                    }
                    project_manager.write_state(&paths.state, state).await?;
                }
            }
        }

        if graph_decision == GraphDecision::NeedRescue
            && normal_rounds < normal_budget.max_rounds
            && pairing_plan.strategy != PairingStrategy::Exhaustive
        {
            let reason_codes = FailureAnalyzer
                .graph_failures(&graph, graph_decision)
                .into_iter()
                .map(|value| format!("{value:?}").to_ascii_lowercase())
                .collect();
            self.events
                .stage(PipelineStage::Matching, 0.94, "正在优化重建策略");
            colmap::match_exhaustive(
                &self.engines.colmap,
                database,
                colmap_log.to_path_buf(),
                &self.process_manager,
                Some(self.process_observer(
                    PipelineStage::Matching,
                    PipelineEngine::Colmap,
                    Some(input_images),
                    ObserverMode::BracketProgress,
                )),
                gpu_index,
                colmap_tuning,
            )
            .await?;
            graph = ViewGraphAnalyzer.analyze_database(database)?;
            graph_decision = GraphQualityGate::default().decide(&graph);
            normal_rounds += 1;
            if let Some(planner) = state.planner.as_mut() {
                planner.pairing_actual = Some(PairingStrategy::Exhaustive);
                planner.rescue_history.push(RescueRecord {
                    round: normal_rounds,
                    mode: PlannerRecoveryMode::Normal,
                    action: RescueAction::LocalExhaustive,
                    effective: Some(graph_decision != GraphDecision::NeedRescue),
                    reason_codes,
                });
            }
        }

        let prior = state
            .planner
            .as_ref()
            .and_then(|planner| planner.capture_prior.as_ref())
            .cloned()
            .unwrap_or_default();
        // Production Planner always uses Incremental. Persisted plans from an
        // older Planner version are intentionally normalized here on resume.
        let mapper_plan = MapperPlan::production_incremental();
        if let Some(planner) = state.planner.as_mut() {
            planner.view_graph_report = Some(graph.clone());
            planner.graph_decision = Some(graph_decision);
            planner.mapper_plan = Some(mapper_plan.clone());
        }
        project_manager.write_state(&paths.state, state).await?;
        write_planner_snapshot(paths, state).await?;

        let mut candidates: Vec<_> = state
            .planner
            .as_ref()
            .map(|planner| planner.reconstruction_candidates.clone())
            .unwrap_or_default()
            .into_iter()
            .filter(|candidate| candidate.mapper == MapperBackend::Incremental)
            .collect();
        let primary_id =
            format!("normal-{}-{:?}", normal_rounds, mapper_plan.backend).to_ascii_lowercase();
        if !candidates
            .iter()
            .any(|candidate| candidate.id == primary_id)
        {
            if let Ok(candidate) = self
                .execute_mapper_candidate(
                    paths,
                    database,
                    images,
                    colmap_log,
                    input_images,
                    primary_id.clone(),
                    mapper_plan.backend,
                    normal_rounds,
                    mapper_plan.calibrate_view_graph,
                    colmap_tuning,
                )
                .await
            {
                candidates.push(candidate);
            }
        }
        checkpoint_planner_candidates(project_manager, paths, state, &candidates).await?;

        let baseline_candidate = candidates
            .iter()
            .find(|candidate| candidate.id == primary_id)
            .filter(|candidate| candidate.viability != ReconstructionViability::NotViable)
            .cloned();
        let mut geometry_screening_handled_usable_reconstruction = false;
        if state.preset == Quality::High {
            if let Some(baseline) = baseline_candidate {
                let remaining_candidates = state
                    .frames
                    .as_ref()
                    .map(frame_plan_from_state)
                    .map(|plan| {
                        let selected: std::collections::HashSet<_> = plan
                            .selected_frames
                            .iter()
                            .map(|frame| frame.source_frame_index)
                            .collect();
                        plan.candidate_frames
                            .iter()
                            .filter(|frame| !selected.contains(&frame.source_frame_index))
                            .count()
                    })
                    .unwrap_or(0);
                let applicable = screening_applicable(
                    state.preset,
                    state.planner_enabled,
                    baseline.viability,
                    remaining_candidates,
                );
                let checkpoint_exists = state.planner.as_ref().is_some_and(|planner| {
                    planner.geometry_screening_complete
                        && planner.geometry_probe_status != GeometryProbeStatus::NotEvaluated
                });
                if applicable || checkpoint_exists {
                    geometry_screening_handled_usable_reconstruction = true;
                    self.run_high_geometry_screening_and_probe(
                        project_manager,
                        paths,
                        state,
                        database,
                        images,
                        colmap_log,
                        gpu_index,
                        pairing_plan,
                        source_video,
                        has_alpha,
                        sfm_budget,
                        colmap_tuning,
                        normal_rounds,
                        &baseline,
                        &mut candidates,
                        &mut input_images,
                    )
                    .await?;
                }
            }
        }

        let has_pass = candidates
            .iter()
            .any(|candidate| candidate.decision == ReconstructionDecision::Pass);
        if !has_pass && !geometry_screening_handled_usable_reconstruction {
            recovery_started = Some(Instant::now());
            let policy = state
                .planner
                .as_ref()
                .and_then(|planner| planner.success_recovery_policy_snapshot)
                .unwrap_or_default();
            if let Some(planner) = state.planner.as_mut() {
                // No distinct normal-budget action remains useful. This may occur
                // before max_rounds when every applicable action has already run.
                planner.normal_budget_exhausted = true;
                planner.success_recovery_entered = true;
                planner.recovery_mode = PlannerRecoveryMode::SuccessRecovery;
            }
            self.events.stage(
                PipelineStage::Reconstructing,
                0.70,
                "正在尝试提高重建成功率",
            );
            let mut recovery_round = state
                .planner
                .as_ref()
                .map(|planner| {
                    planner
                        .rescue_history
                        .iter()
                        .filter(|record| record.mode == PlannerRecoveryMode::SuccessRecovery)
                        .count() as u32
                })
                .unwrap_or(0);
            if policy.allow_frame_backfill_beyond_quality
                && source_video.is_file()
                && recovery_round < policy.max_rounds
                && !recovery_action_completed(state, RescueAction::FrameBackfill)
            {
                if let Some(mut full_plan) = state.frames.as_ref().map(frame_plan_from_state) {
                    let old_selected: std::collections::HashSet<u64> = full_plan
                        .selected_frames
                        .iter()
                        .map(|frame| frame.source_frame_index)
                        .collect();
                    let duration = state
                        .video
                        .as_ref()
                        .map(|video| video.duration)
                        .unwrap_or(0.0);
                    let recovery_fps = (full_plan.actual_average_fps * 1.25)
                        .max(full_plan.actual_average_fps + 1.0)
                        .min(policy.max_recovery_fps.unwrap_or(f64::INFINITY));
                    let target_count = (duration * recovery_fps).ceil() as usize;
                    let remaining = full_plan
                        .candidate_frames
                        .iter()
                        .filter(|frame| !old_selected.contains(&frame.source_frame_index))
                        .count();
                    let additional = target_count
                        .saturating_sub(full_plan.selected_frames.len())
                        .max(1)
                        .min(remaining);
                    if additional > 0
                        && BudgetFramePlanner
                            .backfill(&mut full_plan, additional)
                            .is_ok()
                    {
                        full_plan.actual_average_fps =
                            full_plan.selected_frames.len() as f64 / duration.max(0.001);
                        full_plan.sampling_fps = full_plan.actual_average_fps;
                        let additions: Vec<_> = full_plan
                            .selected_frames
                            .iter()
                            .filter(|frame| !old_selected.contains(&frame.source_frame_index))
                            .cloned()
                            .collect();
                        let mut addition_plan = full_plan.clone();
                        addition_plan.selected_frames = additions.clone();
                        addition_plan.estimated_frames = additions.len() as u64;
                        let extraction = extract_additional_frames(
                            &self.engines.ffmpeg,
                            source_video,
                            &paths.frames,
                            &paths.masks,
                            &addition_plan,
                            has_alpha,
                            Some(paths.logs.join("ffmpeg-recovery.log")),
                            &self.process_manager,
                            Some(self.process_observer(
                                PipelineStage::ExtractingFrames,
                                PipelineEngine::Ffmpeg,
                                Some(additions.len() as u64),
                                ObserverMode::Ffmpeg,
                            )),
                        )
                        .await?;
                        input_images = extraction.frame_count;
                        let extension = if has_alpha { "png" } else { "jpg" };
                        let names: Vec<String> = additions
                            .iter()
                            .map(|frame| {
                                format!("frame_{:010}.{extension}", frame.source_frame_index)
                            })
                            .collect();
                        let image_list = paths.colmap.join("planner-recovery-images.txt");
                        tokio::fs::write(&image_list, names.join("\n")).await?;
                        let recovery_sfm = state
                            .planner
                            .as_ref()
                            .and_then(|planner| planner.quality_budget_snapshot.as_ref())
                            .map(|budget| budget.extension.sfm_rescue)
                            .unwrap_or(sfm_budget);
                        colmap::extract_features_for_list(
                            &self.engines.colmap,
                            database,
                            images,
                            has_alpha.then_some(Path::new("../masks")),
                            &image_list,
                            paths.logs.join("colmap-recovery.log"),
                            &self.process_manager,
                            None,
                            gpu_index,
                            recovery_sfm,
                            colmap_tuning,
                        )
                        .await?;
                        let pair_list = paths.colmap.join("planner-recovery-pairs.txt");
                        write_backfill_pairs(
                            &paths.frames,
                            &names,
                            pairing_plan.sequential_overlap.max(4) as usize,
                            &pair_list,
                        )
                        .await?;
                        colmap::match_pairs(
                            &self.engines.colmap,
                            database,
                            &pair_list,
                            paths.logs.join("colmap-recovery.log"),
                            &self.process_manager,
                            None,
                            gpu_index,
                            colmap_tuning,
                        )
                        .await?;
                        recovery_round += 1;
                        let id = format!("recovery-{recovery_round}-{:?}", mapper_plan.backend)
                            .to_ascii_lowercase();
                        if let Ok(candidate) = self
                            .execute_mapper_candidate(
                                paths,
                                database,
                                images,
                                colmap_log,
                                input_images,
                                id,
                                mapper_plan.backend,
                                normal_rounds + recovery_round,
                                false,
                                colmap_tuning,
                            )
                            .await
                        {
                            candidates.push(candidate);
                        }
                        if let Some(frames) = state.frames.as_mut() {
                            *frames = FrameState::from(&full_plan);
                            frames.extracted_frames = Some(input_images);
                            frames.image_format =
                                Some(if has_alpha { "png" } else { "jpeg" }.into());
                            frames.mask_count = Some(if has_alpha { input_images } else { 0 });
                            frames.has_alpha = has_alpha;
                        }
                        if let Some(planner) = state.planner.as_mut() {
                            let normal_max = planner
                                .quality_budget_snapshot
                                .as_ref()
                                .map(|budget| budget.frame.max_fps)
                                .unwrap_or(f64::INFINITY);
                            planner.frame_plan = Some(full_plan.clone());
                            planner.actual_selected_frames = full_plan
                                .selected_frames
                                .iter()
                                .map(|frame| frame.source_frame_index)
                                .collect();
                            planner.budget_overridden_for_success |=
                                full_plan.actual_average_fps > normal_max;
                            planner.rescue_history.push(RescueRecord {
                                round: recovery_round,
                                mode: PlannerRecoveryMode::SuccessRecovery,
                                action: RescueAction::FrameBackfill,
                                effective: Some(candidates.last().is_some_and(|candidate| {
                                    candidate.viability != ReconstructionViability::NotViable
                                })),
                                reason_codes: vec!["frame_budget_overridden_for_success".into()],
                            });
                        }
                        checkpoint_planner_candidates(project_manager, paths, state, &candidates)
                            .await?;
                    }
                }
            }
            if policy.allow_pairing_escalation_beyond_quality
                && state
                    .planner
                    .as_ref()
                    .and_then(|planner| planner.pairing_actual)
                    != Some(PairingStrategy::Exhaustive)
                && recovery_round < policy.max_rounds
                && !recovery_action_completed(state, RescueAction::LocalExhaustive)
                && !recovery_action_completed(state, RescueAction::Prefilter)
            {
                let recovery_matching = MatchingBudget {
                    sequential_overlap: pairing_plan.sequential_overlap.max(20),
                    prefilter_neighbors: pairing_plan.prefilter_neighbors.max(32),
                };
                let matching_cost =
                    EstimatedMatchingCost::new(input_images, recovery_matching.sequential_overlap);
                let recovery_strategy = if matching_cost.exhaustive_pairs
                    <= PairingThresholds::default().exhaustive_pair_limit
                {
                    colmap::match_exhaustive(
                        &self.engines.colmap,
                        database,
                        colmap_log.to_path_buf(),
                        &self.process_manager,
                        Some(self.process_observer(
                            PipelineStage::Matching,
                            PipelineEngine::Colmap,
                            Some(input_images),
                            ObserverMode::BracketProgress,
                        )),
                        gpu_index,
                        colmap_tuning,
                    )
                    .await?;
                    PairingStrategy::Exhaustive
                } else {
                    colmap::match_prefilter(
                        &self.engines.colmap,
                        database,
                        &self.engines.colmap_vocab_tree,
                        colmap_log.to_path_buf(),
                        &self.process_manager,
                        Some(self.process_observer(
                            PipelineStage::Matching,
                            PipelineEngine::Colmap,
                            Some(input_images),
                            ObserverMode::BracketProgress,
                        )),
                        gpu_index,
                        recovery_matching.prefilter_neighbors,
                        colmap_tuning,
                    )
                    .await?;
                    PairingStrategy::Prefilter
                };
                recovery_round += 1;
                let id = format!("recovery-{recovery_round}-{:?}", mapper_plan.backend)
                    .to_ascii_lowercase();
                if let Ok(candidate) = self
                    .execute_mapper_candidate(
                        paths,
                        database,
                        images,
                        colmap_log,
                        input_images,
                        id,
                        mapper_plan.backend,
                        normal_rounds + recovery_round,
                        false,
                        colmap_tuning,
                    )
                    .await
                {
                    candidates.push(candidate);
                }
                if let Some(planner) = state.planner.as_mut() {
                    planner.pairing_actual = Some(recovery_strategy);
                    planner.budget_overridden_for_success = true;
                    planner.rescue_history.push(RescueRecord {
                        round: recovery_round,
                        mode: PlannerRecoveryMode::SuccessRecovery,
                        action: if recovery_strategy == PairingStrategy::Exhaustive {
                            RescueAction::LocalExhaustive
                        } else {
                            RescueAction::Prefilter
                        },
                        effective: Some(candidates.last().is_some_and(|candidate| {
                            candidate.viability != ReconstructionViability::NotViable
                        })),
                        reason_codes: vec!["normal_budget_exhausted".into()],
                    });
                }
                checkpoint_planner_candidates(project_manager, paths, state, &candidates).await?;
            }
            if !candidates
                .iter()
                .any(|candidate| candidate.decision == ReconstructionDecision::Pass)
                && policy.allow_sfm_escalation_beyond_quality
                && recovery_round < policy.max_rounds
                && !recovery_action_completed(state, RescueAction::SfmEscalation)
            {
                let escalated_database = paths.colmap.join("database-recovery-sfm.db");
                if escalated_database.is_file() {
                    tokio::fs::remove_file(&escalated_database).await?;
                }
                let escalated_sfm = SfmBudget {
                    max_image_size: policy.max_sfm_image_size,
                    max_features: policy.max_sfm_features,
                };
                self.events.stage(
                    PipelineStage::ExtractingFeatures,
                    0.0,
                    "正在提高重建特征精度",
                );
                colmap::extract_features(
                    &self.engines.colmap,
                    &escalated_database,
                    images,
                    has_alpha.then_some(Path::new("../masks")),
                    paths.logs.join("colmap-recovery-sfm.log"),
                    &self.process_manager,
                    Some(self.process_observer(
                        PipelineStage::ExtractingFeatures,
                        PipelineEngine::Colmap,
                        Some(input_images),
                        ObserverMode::BracketProgress,
                    )),
                    gpu_index,
                    escalated_sfm,
                    colmap_tuning,
                )
                .await?;
                let recovery_matching = MatchingBudget {
                    sequential_overlap: pairing_plan.sequential_overlap.max(20),
                    prefilter_neighbors: pairing_plan.prefilter_neighbors.max(32),
                };
                let recovery_pairing =
                    PairingPlanner::default().plan(&prior, input_images, recovery_matching);
                match recovery_pairing.strategy {
                    PairingStrategy::Exhaustive => {
                        colmap::match_exhaustive(
                            &self.engines.colmap,
                            &escalated_database,
                            paths.logs.join("colmap-recovery-sfm.log"),
                            &self.process_manager,
                            None,
                            gpu_index,
                            colmap_tuning,
                        )
                        .await?;
                    }
                    PairingStrategy::Sequential => {
                        colmap::match_sequential(
                            &self.engines.colmap,
                            &escalated_database,
                            paths.logs.join("colmap-recovery-sfm.log"),
                            &self.process_manager,
                            None,
                            gpu_index,
                            recovery_matching,
                            colmap_tuning,
                        )
                        .await?;
                    }
                    PairingStrategy::SequentialWithLoopClosure => {
                        colmap::match_sequential_with_loop(
                            &self.engines.colmap,
                            &escalated_database,
                            &self.engines.colmap_vocab_tree,
                            paths.logs.join("colmap-recovery-sfm.log"),
                            &self.process_manager,
                            None,
                            gpu_index,
                            recovery_matching,
                            colmap_tuning,
                        )
                        .await?;
                    }
                    PairingStrategy::Prefilter => {
                        colmap::match_prefilter(
                            &self.engines.colmap,
                            &escalated_database,
                            &self.engines.colmap_vocab_tree,
                            paths.logs.join("colmap-recovery-sfm.log"),
                            &self.process_manager,
                            None,
                            gpu_index,
                            recovery_matching.prefilter_neighbors,
                            colmap_tuning,
                        )
                        .await?;
                    }
                }
                recovery_round += 1;
                let id = format!("recovery-{recovery_round}-sfm-{:?}", mapper_plan.backend)
                    .to_ascii_lowercase();
                if let Ok(candidate) = self
                    .execute_mapper_candidate(
                        paths,
                        &escalated_database,
                        images,
                        colmap_log,
                        input_images,
                        id,
                        mapper_plan.backend,
                        normal_rounds + recovery_round,
                        false,
                        colmap_tuning,
                    )
                    .await
                {
                    candidates.push(candidate);
                }
                if let Some(planner) = state.planner.as_mut() {
                    planner.pairing_actual = Some(recovery_pairing.strategy);
                    planner.budget_overridden_for_success = true;
                    planner.rescue_history.push(RescueRecord {
                        round: recovery_round,
                        mode: PlannerRecoveryMode::SuccessRecovery,
                        action: RescueAction::SfmEscalation,
                        effective: Some(candidates.last().is_some_and(|candidate| {
                            candidate.viability != ReconstructionViability::NotViable
                        })),
                        reason_codes: vec!["sfm_budget_overridden_for_success".into()],
                    });
                }
                checkpoint_planner_candidates(project_manager, paths, state, &candidates).await?;
            }
        }

        let best = ReconstructionComparator
            .best(&candidates)
            .cloned()
            .ok_or_else(|| {
                SplatError::Process(
                    "所有正常与恢复重建路径均已结束，未找到可用于 Brush 的重建结果".into(),
                )
            })?;
        let report = ReconstructionValidator::validate(&paths.frames, &best.model_path)?;
        if let Some(planner) = state.planner.as_mut() {
            planner.mapper_actual = Some(best.mapper);
            planner.reconstruction_candidates = candidates;
            planner.best_reconstruction_id = Some(best.id.clone());
            planner.reconstruction_report = Some(report.clone());
            planner.normal_duration_ms = recovery_started
                .map(|started| started.duration_since(planner_started).as_millis() as u64)
                .unwrap_or_else(|| planner_started.elapsed().as_millis() as u64);
            planner.recovery_duration_ms = recovery_started
                .map(|started| started.elapsed().as_millis() as u64)
                .unwrap_or(0);
        }
        state.stage = PipelineStage::Reconstructing;
        state.reconstruction_complete = true;
        project_manager.write_state(&paths.state, state).await?;
        write_planner_snapshot(paths, state).await?;
        self.events
            .stage(PipelineStage::Reconstructing, 1.0, "已选择最佳可用重建");
        Ok((best.model_path.clone(), report, best, input_images))
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_high_geometry_screening_and_probe(
        &self,
        project_manager: &ProjectManager,
        paths: &ProjectPaths,
        state: &mut PipelineStateFile,
        database: &Path,
        images: &Path,
        colmap_log: &Path,
        gpu_index: Option<u32>,
        pairing_plan: &PairingPlan,
        source_video: &Path,
        has_alpha: bool,
        sfm_budget: SfmBudget,
        colmap_tuning: colmap::ColmapQualityTuning,
        normal_rounds: u32,
        baseline: &ReconstructionCandidate,
        candidates: &mut Vec<ReconstructionCandidate>,
        input_images: &mut u64,
    ) -> Result<()> {
        let thresholds = GeometryScreeningThresholds::default();
        let current_status = state
            .planner
            .as_ref()
            .map(|planner| planner.geometry_probe_status)
            .unwrap_or_default();
        if matches!(
            current_status,
            GeometryProbeStatus::NotNeeded
                | GeometryProbeStatus::RecommendedButNoBudget
                | GeometryProbeStatus::Completed
                | GeometryProbeStatus::FailedRolledBack
        ) {
            return Ok(());
        }

        if !state
            .planner
            .as_ref()
            .is_some_and(|planner| planner.geometry_screening_complete)
        {
            let Some(frame_plan) = state.frames.as_ref().map(frame_plan_from_state) else {
                return Ok(());
            };
            let report = match analyze_sparse_geometry(
                &baseline.model_path,
                database,
                &frame_plan,
                &baseline.metrics,
                thresholds,
            ) {
                Ok(report) => report,
                Err(error) => {
                    self.events.send(
                        PipelineStage::Reconstructing,
                        Some(PipelineEngine::System),
                        EventKind::Log,
                        EventLevel::Warning,
                        None,
                        false,
                        format!("[HighGeometryScreening] 分析失败，继续使用初始重建：{error}"),
                        None,
                        None,
                        None,
                    );
                    if let Some(planner) = state.planner.as_mut() {
                        planner.geometry_screening_complete = true;
                        planner.geometry_probe_status = GeometryProbeStatus::FailedRolledBack;
                        planner.geometry_probe_before_candidate_id = Some(baseline.id.clone());
                    }
                    project_manager.write_state(&paths.state, state).await?;
                    return Ok(());
                }
            };
            self.events.send(
                PipelineStage::Reconstructing,
                Some(PipelineEngine::System),
                EventKind::Log,
                EventLevel::Info,
                None,
                false,
                format!(
                    "[HighGeometryScreening] thresholdProfile={} points3D={} observations={} meanTrackLength={:.3} pointDiversity={:.5} medianTriangulationRatio={:.5} p25TriangulationRatio={:.5} weakIntervals={} triangulationUnderfilled={} trackRedundancyHigh={} continuousWeakRegion={} decision={:?}",
                    report.threshold_profile,
                    report.points_3d,
                    report.observations,
                    report.mean_track_length,
                    report.point_diversity_ratio,
                    report.median_triangulation_ratio,
                    report.p25_triangulation_ratio,
                    report.weak_geometry_intervals.len(),
                    report.triangulation_underfilled,
                    report.track_redundancy_high,
                    report.continuous_weak_region,
                    report.decision,
                ),
                None,
                None,
                None,
            );
            if let Some(planner) = state.planner.as_mut() {
                planner.geometry_screening_complete = true;
                planner.geometry_screening_report = Some(report.clone());
                planner.geometry_probe_before_candidate_id = Some(baseline.id.clone());
                if report.decision == GeometryScreeningDecision::NoProbe {
                    planner.geometry_probe_status = GeometryProbeStatus::NotNeeded;
                }
            }
            project_manager.write_state(&paths.state, state).await?;
            if report.decision == GeometryScreeningDecision::NoProbe {
                return Ok(());
            }
        }

        let status = state
            .planner
            .as_ref()
            .map(|planner| planner.geometry_probe_status)
            .unwrap_or_default();
        if status != GeometryProbeStatus::Running {
            let Some(frame_plan) = state.frames.as_ref().map(frame_plan_from_state) else {
                return Ok(());
            };
            let selected: std::collections::HashSet<_> = frame_plan
                .selected_frames
                .iter()
                .map(|frame| frame.source_frame_index)
                .collect();
            let remaining = frame_plan
                .candidate_frames
                .iter()
                .filter(|frame| !selected.contains(&frame.source_frame_index))
                .count();
            let duration = state
                .video
                .as_ref()
                .map(|video| video.duration)
                .unwrap_or(0.0);
            let max_fps = state
                .planner
                .as_ref()
                .and_then(|planner| planner.quality_budget_snapshot.as_ref())
                .map(|budget| budget.frame.max_fps)
                .unwrap_or(15.0);
            let budget =
                geometry_probe_budget(&frame_plan, duration, max_fps, remaining, thresholds);
            if budget.available == 0 {
                if let Some(planner) = state.planner.as_mut() {
                    planner.geometry_probe_status = GeometryProbeStatus::RecommendedButNoBudget;
                    planner.geometry_probe_metrics = Some(GeometryProbeMetrics {
                        triggered_reasons: planner
                            .geometry_screening_report
                            .as_ref()
                            .map(geometry_probe_reasons)
                            .unwrap_or_default(),
                        requested_additional_frames: budget.requested,
                        baseline_points: baseline.metrics.points_3d,
                        baseline_observations: baseline.metrics.observations,
                        baseline_track_length: baseline.metrics.mean_track_length,
                        baseline_reprojection_error: baseline.metrics.mean_reprojection_error,
                        ..GeometryProbeMetrics::default()
                    });
                }
                project_manager.write_state(&paths.state, state).await?;
                self.events.send(
                    PipelineStage::Reconstructing,
                    Some(PipelineEngine::System),
                    EventKind::Log,
                    EventLevel::Info,
                    None,
                    false,
                    "[HighGeometryProbe] ProbeRecommendedButNoBudget：已达到 High 15fps 上限或候选池耗尽",
                    None,
                    None,
                    None,
                );
                return Ok(());
            }
            let candidate_bytes =
                match tokio::fs::read(paths.work.join("planner").join("frame-candidates.json"))
                    .await
                {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        self.record_geometry_probe_failure(
                            project_manager,
                            paths,
                            state,
                            format!("无法读取候选帧：{error}"),
                        )
                        .await?;
                        return Ok(());
                    }
                };
            let frame_candidates: Vec<FrameCandidate> =
                match serde_json::from_slice(&candidate_bytes) {
                    Ok(candidates) => candidates,
                    Err(error) => {
                        self.record_geometry_probe_failure(
                            project_manager,
                            paths,
                            state,
                            format!("无法恢复候选帧：{error}"),
                        )
                        .await?;
                        return Ok(());
                    }
                };
            let report = state
                .planner
                .as_ref()
                .and_then(|planner| planner.geometry_screening_report.as_ref())
                .cloned();
            let Some(report) = report else {
                self.record_geometry_probe_failure(
                    project_manager,
                    paths,
                    state,
                    "筛查 checkpoint 缺失".into(),
                )
                .await?;
                return Ok(());
            };
            let additions = plan_geometry_probe_backfill(
                &frame_plan,
                &frame_candidates,
                &report,
                budget.available,
                thresholds,
            );
            if additions.is_empty() {
                if let Some(planner) = state.planner.as_mut() {
                    planner.geometry_probe_status = GeometryProbeStatus::RecommendedButNoBudget;
                }
                project_manager.write_state(&paths.state, state).await?;
                return Ok(());
            }
            let reasons = geometry_probe_reasons(&report);
            let strategy = if report.continuous_weak_region {
                "ContinuousWeakRegion"
            } else if report.triangulation_underfilled {
                "LowTriangulationRegions"
            } else {
                "LargestTemporalGap"
            };
            self.events.send(
                PipelineStage::Reconstructing,
                Some(PipelineEngine::System),
                EventKind::Log,
                EventLevel::Info,
                None,
                false,
                format!(
                    "[HighGeometryProbe] reason={reasons:?} selectedBefore={} requestedAdditional={} actualAdditional={} selectionStrategy={strategy}",
                    frame_plan.selected_frames.len(),
                    budget.requested,
                    additions.len(),
                ),
                None,
                None,
                None,
            );
            if let Some(planner) = state.planner.as_mut() {
                if !can_start_new_probe(
                    planner.geometry_probe_attempted,
                    planner.geometry_probe_status,
                ) {
                    return Ok(());
                }
                planner.geometry_probe_attempted = true;
                planner.geometry_probe_status = GeometryProbeStatus::Running;
                planner.geometry_probe_added_frames = additions.len();
                planner.geometry_probe_frame_indices = additions
                    .iter()
                    .map(|frame| frame.source_frame_index)
                    .collect();
                planner.geometry_probe_metrics = Some(GeometryProbeMetrics {
                    triggered_reasons: reasons,
                    requested_additional_frames: budget.requested,
                    actual_additional_frames: additions.len(),
                    baseline_points: baseline.metrics.points_3d,
                    baseline_observations: baseline.metrics.observations,
                    baseline_track_length: baseline.metrics.mean_track_length,
                    baseline_reprojection_error: baseline.metrics.mean_reprojection_error,
                    ..GeometryProbeMetrics::default()
                });
            }
            project_manager.write_state(&paths.state, state).await?;
        }

        let probe_started = Instant::now();
        let original_plan = state.frames.as_ref().map(frame_plan_from_state);
        let Some(original_plan) = original_plan else {
            self.record_geometry_probe_failure(
                project_manager,
                paths,
                state,
                "画面计划 checkpoint 缺失".into(),
            )
            .await?;
            return Ok(());
        };
        let probe_indices: std::collections::HashSet<u64> = state
            .planner
            .as_ref()
            .map(|planner| {
                planner
                    .geometry_probe_frame_indices
                    .iter()
                    .copied()
                    .collect()
            })
            .unwrap_or_default();
        let mut additions: Vec<_> = original_plan
            .candidate_frames
            .iter()
            .filter(|frame| probe_indices.contains(&frame.source_frame_index))
            .cloned()
            .collect();
        additions.sort_by_key(|frame| frame.source_frame_index);
        additions.dedup_by_key(|frame| frame.source_frame_index);
        if additions.len() != probe_indices.len() {
            self.record_geometry_probe_failure(
                project_manager,
                paths,
                state,
                "新增帧列表无法从 checkpoint 精确恢复".into(),
            )
            .await?;
            return Ok(());
        }
        let mut full_plan = original_plan.clone();
        full_plan.selected_frames.extend(additions.clone());
        full_plan
            .selected_frames
            .sort_by_key(|frame| frame.source_frame_index);
        full_plan
            .selected_frames
            .dedup_by_key(|frame| frame.source_frame_index);
        full_plan.estimated_frames = full_plan.selected_frames.len() as u64;
        full_plan.actual_average_fps = full_plan.selected_frames.len() as f64
            / state
                .video
                .as_ref()
                .map(|video| video.duration)
                .unwrap_or(0.0)
                .max(0.001);
        full_plan.sampling_fps = full_plan.actual_average_fps;

        let extension = if has_alpha { "png" } else { "jpg" };
        let names: Vec<String> = additions
            .iter()
            .map(|frame| format!("frame_{:010}.{extension}", frame.source_frame_index))
            .collect();
        let missing: Vec<_> = additions
            .iter()
            .filter(|frame| {
                !paths
                    .frames
                    .join(format!(
                        "frame_{:010}.{extension}",
                        frame.source_frame_index
                    ))
                    .is_file()
            })
            .cloned()
            .collect();
        let probe_result: Result<ReconstructionCandidate> = async {
            if !missing.is_empty() {
                let mut addition_plan = full_plan.clone();
                addition_plan.selected_frames = missing;
                addition_plan.estimated_frames = addition_plan.selected_frames.len() as u64;
                let extraction = extract_additional_frames(
                    &self.engines.ffmpeg,
                    source_video,
                    &paths.frames,
                    &paths.masks,
                    &addition_plan,
                    has_alpha,
                    Some(paths.logs.join("ffmpeg-geometry-probe.log")),
                    &self.process_manager,
                    Some(self.process_observer(
                        PipelineStage::ExtractingFrames,
                        PipelineEngine::Ffmpeg,
                        Some(addition_plan.estimated_frames),
                        ObserverMode::Ffmpeg,
                    )),
                )
                .await?;
                *input_images = extraction.frame_count;
            } else {
                *input_images = full_plan.selected_frames.len() as u64;
            }
            let image_list = paths.colmap.join("geometry-probe-images.txt");
            tokio::fs::write(&image_list, names.join("\n")).await?;
            colmap::extract_features_for_list(
                &self.engines.colmap,
                database,
                images,
                has_alpha.then_some(Path::new("../masks")),
                &image_list,
                paths.logs.join("colmap-geometry-probe.log"),
                &self.process_manager,
                Some(self.process_observer(
                    PipelineStage::ExtractingFeatures,
                    PipelineEngine::Colmap,
                    Some(additions.len() as u64),
                    ObserverMode::BracketProgress,
                )),
                gpu_index,
                sfm_budget,
                colmap_tuning,
            )
            .await?;
            let pair_list = paths.colmap.join("geometry-probe-pairs.txt");
            write_backfill_pairs(
                &paths.frames,
                &names,
                pairing_plan.sequential_overlap.max(4) as usize,
                &pair_list,
            )
            .await?;
            colmap::match_pairs(
                &self.engines.colmap,
                database,
                &pair_list,
                paths.logs.join("colmap-geometry-probe.log"),
                &self.process_manager,
                Some(self.process_observer(
                    PipelineStage::Matching,
                    PipelineEngine::Colmap,
                    Some(additions.len() as u64),
                    ObserverMode::BracketProgress,
                )),
                gpu_index,
                colmap_tuning,
            )
            .await?;
            self.execute_mapper_candidate(
                paths,
                database,
                images,
                colmap_log,
                *input_images,
                "geometry-probe-incremental".into(),
                MapperBackend::Incremental,
                normal_rounds,
                false,
                colmap_tuning,
            )
            .await
        }
        .await;

        match probe_result {
            Ok(probe) => {
                let accepted = probe_candidate_acceptable(baseline, &probe, thresholds);
                if let Some(planner) = state.planner.as_mut() {
                    planner.geometry_probe_after_candidate_id = Some(probe.id.clone());
                    planner.geometry_probe_status = if accepted {
                        GeometryProbeStatus::Completed
                    } else {
                        GeometryProbeStatus::FailedRolledBack
                    };
                    if let Some(metrics) = planner.geometry_probe_metrics.as_mut() {
                        metrics.probe_points = Some(probe.metrics.points_3d);
                        metrics.point_gain_ratio =
                            signed_gain_ratio(probe.metrics.points_3d, metrics.baseline_points);
                        metrics.probe_observations = Some(probe.metrics.observations);
                        metrics.observation_gain_ratio = signed_gain_ratio(
                            probe.metrics.observations,
                            metrics.baseline_observations,
                        );
                        metrics.probe_track_length = probe.metrics.mean_track_length;
                        metrics.probe_reprojection_error = probe.metrics.mean_reprojection_error;
                        metrics.duration_ms = probe_started.elapsed().as_millis() as u64;
                    }
                }
                if accepted {
                    if let Some(existing) = candidates
                        .iter_mut()
                        .find(|candidate| candidate.id == probe.id)
                    {
                        *existing = probe.clone();
                    } else {
                        candidates.push(probe.clone());
                    }
                    if let Some(frames) = state.frames.as_mut() {
                        *frames = FrameState::from(&full_plan);
                        frames.extracted_frames = Some(*input_images);
                        frames.image_format = Some(if has_alpha { "png" } else { "jpeg" }.into());
                        frames.mask_count = Some(if has_alpha { *input_images } else { 0 });
                        frames.has_alpha = has_alpha;
                    }
                    if let Some(planner) = state.planner.as_mut() {
                        planner.frame_plan = Some(full_plan.clone());
                        planner.actual_selected_frames = full_plan
                            .selected_frames
                            .iter()
                            .map(|frame| frame.source_frame_index)
                            .collect();
                    }
                } else {
                    rollback_geometry_probe_frames(paths, &additions, has_alpha).await;
                    *input_images = baseline.metrics.input_images;
                }
                let probe_metrics = state
                    .planner
                    .as_ref()
                    .and_then(|planner| planner.geometry_probe_metrics.as_ref());
                self.events.send(
                    PipelineStage::Reconstructing,
                    Some(PipelineEngine::System),
                    EventKind::Log,
                    if accepted {
                        EventLevel::Info
                    } else {
                        EventLevel::Warning
                    },
                    None,
                    false,
                    format!(
                        "[HighGeometryProbeResult] accepted={} pointsBefore={} pointsAfter={} pointGain={:?} observationsBefore={} observationsAfter={} observationGain={:?} trackBefore={:?} trackAfter={:?} reprojectionBefore={:?} reprojectionAfter={:?}",
                        accepted,
                        baseline.metrics.points_3d,
                        probe.metrics.points_3d,
                        probe_metrics.and_then(|metrics| metrics.point_gain_ratio),
                        baseline.metrics.observations,
                        probe.metrics.observations,
                        probe_metrics.and_then(|metrics| metrics.observation_gain_ratio),
                        baseline.metrics.mean_track_length,
                        probe.metrics.mean_track_length,
                        baseline.metrics.mean_reprojection_error,
                        probe.metrics.mean_reprojection_error,
                    ),
                    None,
                    None,
                    None,
                );
            }
            Err(error) => {
                rollback_geometry_probe_frames(paths, &additions, has_alpha).await;
                *input_images = baseline.metrics.input_images;
                if let Some(planner) = state.planner.as_mut() {
                    planner.geometry_probe_status = GeometryProbeStatus::FailedRolledBack;
                    if let Some(metrics) = planner.geometry_probe_metrics.as_mut() {
                        metrics.duration_ms = probe_started.elapsed().as_millis() as u64;
                    }
                }
                self.events.send(
                    PipelineStage::Reconstructing,
                    Some(PipelineEngine::System),
                    EventKind::Log,
                    EventLevel::Warning,
                    None,
                    false,
                    format!("[HighGeometryProbeResult] Probe 失败，已回退初始重建：{error}"),
                    None,
                    None,
                    None,
                );
            }
        }
        checkpoint_planner_candidates(project_manager, paths, state, candidates).await?;
        write_planner_snapshot(paths, state).await?;
        Ok(())
    }

    async fn record_geometry_probe_failure(
        &self,
        project_manager: &ProjectManager,
        paths: &ProjectPaths,
        state: &mut PipelineStateFile,
        detail: String,
    ) -> Result<()> {
        if let Some(planner) = state.planner.as_mut() {
            planner.geometry_screening_complete = true;
            planner.geometry_probe_status = GeometryProbeStatus::FailedRolledBack;
        }
        project_manager.write_state(&paths.state, state).await?;
        self.events.send(
            PipelineStage::Reconstructing,
            Some(PipelineEngine::System),
            EventKind::Log,
            EventLevel::Warning,
            None,
            false,
            format!("[HighGeometryProbeResult] {detail}，已保留初始重建"),
            None,
            None,
            None,
        );
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_mapper_candidate(
        &self,
        paths: &ProjectPaths,
        baseline_database: &Path,
        images: &Path,
        colmap_log: &Path,
        input_images: u64,
        id: String,
        backend: MapperBackend,
        rescue_round: u32,
        calibrate: bool,
        colmap_tuning: colmap::ColmapQualityTuning,
    ) -> Result<ReconstructionCandidate> {
        let attempt_database = paths.colmap.join(format!("database-{id}.db"));
        tokio::fs::copy(baseline_database, &attempt_database).await?;
        let output = paths.colmap.join("candidates").join(&id);
        reset_directory(&output).await?;
        if calibrate {
            colmap::calibrate_view_graph(
                &self.engines.colmap,
                &attempt_database,
                colmap_log.to_path_buf(),
                &self.process_manager,
                None,
            )
            .await?;
        }
        self.events.stage(
            PipelineStage::Reconstructing,
            0.05,
            match backend {
                MapperBackend::Global => "正在执行全局重建",
                MapperBackend::Incremental => "正在执行增量重建",
            },
        );
        let observer = Some(self.process_observer(
            PipelineStage::Reconstructing,
            PipelineEngine::Colmap,
            Some(input_images),
            ObserverMode::Mapper,
        ));
        match backend {
            MapperBackend::Incremental => {
                colmap::map(
                    &self.engines.colmap,
                    &attempt_database,
                    images,
                    &output,
                    colmap_log.to_path_buf(),
                    &self.process_manager,
                    observer,
                    colmap_tuning,
                )
                .await?
            }
            MapperBackend::Global => {
                colmap::map_global(
                    &self.engines.colmap,
                    &attempt_database,
                    images,
                    &output,
                    colmap_log.to_path_buf(),
                    &self.process_manager,
                    observer,
                )
                .await?
            }
        }
        let (model, _) = best_sparse_model(&paths.frames, &output).await?;
        let analyzer =
            colmap::analyze_model(&self.engines.colmap, &model, &self.process_manager).await?;
        let model_count = count_sparse_models(&output).max(1);
        let mut metrics = parse_model_analyzer(&analyzer, input_images, model_count);
        metrics.finite_geometry &= validate_sparse_geometry(&model).unwrap_or(false);
        let (decision, viability) = PlannerReconstructionValidator.classify(&metrics);
        Ok(ReconstructionCandidate {
            id,
            mapper: backend,
            model_path: model,
            metrics,
            decision,
            viability,
            rescue_round,
        })
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
        let brush_progress_basis_points = Arc::new(AtomicU64::new(0));
        Arc::new(move |update| match update {
            ProcessUpdate::Started { process_id } => events.send(
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
            ),
            ProcessUpdate::Heartbeat { elapsed_ms } => {
                if let ObserverMode::Brush {
                    estimated_duration_ms,
                } = mode
                {
                    let progress = estimated_brush_progress(elapsed_ms, estimated_duration_ms);
                    brush_progress_basis_points
                        .store((progress * 10_000.0).round() as u64, Ordering::Relaxed);
                    events.send(
                        stage,
                        Some(engine),
                        EventKind::Heartbeat,
                        EventLevel::Info,
                        Some(progress),
                        false,
                        format!(
                            "Brush 训练中 · 估算进度 {:.0}% · 已用时 {}",
                            progress * 100.0,
                            format_duration(elapsed_ms)
                        ),
                        None,
                        expected_total,
                        Some("estimated_progress"),
                    );
                }
            }
            ProcessUpdate::Line { stream: _, line } => {
                if line.is_empty() {
                    return;
                }
                let parsed = match mode {
                    ObserverMode::Ffmpeg => parse_ffmpeg_frame(&line).map(|current| {
                        (
                            current,
                            expected_total,
                            format!("FFmpeg 已输出 {current} 帧"),
                        )
                    }),
                    ObserverMode::BracketProgress => {
                        parse_bracket_progress(&line).map(|(current, total)| {
                            (current, Some(total), friendly_engine_line(&line))
                        })
                    }
                    ObserverMode::Mapper => {
                        parse_mapper_progress(&line, &mapper_count, expected_total)
                    }
                    ObserverMode::Brush { .. } => None,
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
                        progress,
                        progress.is_none(),
                        message,
                        Some(current),
                        total,
                        Some("张"),
                    );
                } else if matches!(mode, ObserverMode::Brush { .. }) {
                    let progress =
                        brush_progress_basis_points.load(Ordering::Relaxed) as f32 / 10_000.0;
                    events.send(
                        stage,
                        Some(engine),
                        EventKind::Log,
                        EventLevel::Info,
                        Some(progress),
                        progress == 0.0,
                        friendly_engine_line(&line),
                        None,
                        expected_total,
                        Some("estimated_progress"),
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
    Mapper,
    Brush { estimated_duration_ms: u64 },
}

fn estimated_brush_progress(elapsed_ms: u64, estimated_duration_ms: u64) -> f32 {
    const MAX_PROGRESS_BEFORE_COMPLETION: f64 = 0.95;
    if estimated_duration_ms == 0 {
        return 0.0;
    }
    ((elapsed_ms as f64 / estimated_duration_ms as f64) * MAX_PROGRESS_BEFORE_COMPLETION)
        .clamp(0.0, MAX_PROGRESS_BEFORE_COMPLETION) as f32
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

fn mark_state_terminal(mut state: PipelineStateFile, cancelled: bool) -> PipelineStateFile {
    state.stage = if cancelled {
        PipelineStage::Cancelled
    } else {
        PipelineStage::Failed
    };
    state
}

async fn normalize_checkpoints(paths: &ProjectPaths, state: &mut PipelineStateFile) -> Result<()> {
    let frames_complete = prepared_frames_from_checkpoint(paths, state)
        .await?
        .is_some();
    if !frames_complete {
        state.video = None;
        state.image_sequence = None;
        state.frames = None;
    }

    let database_complete = tokio::fs::metadata(paths.colmap.join("database.db"))
        .await
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0);
    state.features_complete = frames_complete && state.features_complete && database_complete;
    state.matching_complete = state.features_complete && state.matching_complete;
    let reconstruction_artifact_complete = if state.planner_enabled {
        state
            .planner
            .as_ref()
            .and_then(|planner| {
                planner
                    .best_reconstruction_id
                    .as_ref()
                    .map(|id| (planner, id))
            })
            .and_then(|(planner, id)| {
                planner
                    .reconstruction_candidates
                    .iter()
                    .find(|candidate| &candidate.id == id)
            })
            .is_some_and(|candidate| {
                ReconstructionValidator::validate(&paths.frames, &candidate.model_path).is_ok()
            })
    } else {
        best_sparse_model(&paths.frames, &paths.colmap.join("sparse"))
            .await
            .is_ok()
    };
    state.reconstruction_complete = state.matching_complete
        && state.reconstruction_complete
        && reconstruction_artifact_complete;
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
    let plan = frame_plan_from_state(frames);
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
            Ok(Some(PreparedFrames {
                input_type: ProjectInputType::Video,
                video: Some(video),
                image_sequence: None,
                plan,
                extracted_frames,
                image_format: extraction.image_format.as_str().into(),
                mask_count: extraction.mask_count,
                has_alpha: extraction.has_alpha,
                capture_prior: state
                    .planner
                    .as_ref()
                    .and_then(|planner| planner.capture_prior.clone()),
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
            Ok(Some(PreparedFrames {
                input_type: ProjectInputType::Images,
                video: None,
                image_sequence: Some(image_sequence),
                plan,
                extracted_frames,
                image_format: "images".into(),
                mask_count: prepared.mask_count,
                has_alpha: prepared.has_alpha,
                capture_prior: state
                    .planner
                    .as_ref()
                    .and_then(|planner| planner.capture_prior.clone()),
            }))
        }
    }
}

fn frame_plan_from_state(frames: &FrameState) -> FramePlan {
    FramePlan {
        quality: frames.quality,
        retention_ratio: frames.retention_ratio,
        sampling_fps: frames.sampling_fps,
        actual_average_fps: if frames.actual_average_fps > 0.0 {
            frames.actual_average_fps
        } else {
            frames.sampling_fps
        },
        target_fps: if frames.target_fps > 0.0 {
            frames.target_fps
        } else {
            frames.sampling_fps
        },
        candidate_fps: if frames.candidate_fps > 0.0 {
            frames.candidate_fps
        } else {
            frames.sampling_fps
        },
        estimated_frames: frames.estimated_frames,
        planning_mode: frames.planning_mode,
        preferred_fps: frames.preferred_fps,
        selected_frames: frames.selected_frames.clone(),
        candidate_frames: frames.candidate_frames.clone(),
        minimum_frame_protection: frames.minimum_frame_protection.clone(),
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

fn best_sparse_model_blocking(
    frames: &Path,
    sparse: &Path,
) -> Result<(PathBuf, ReconstructionReport)> {
    let mut best: Option<(PathBuf, ReconstructionReport)> = None;
    if let Ok(report) = ReconstructionValidator::validate(frames, sparse) {
        best = Some((sparse.to_path_buf(), report));
    }
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

fn recovery_action_completed(state: &PipelineStateFile, action: RescueAction) -> bool {
    state.planner.as_ref().is_some_and(|planner| {
        planner.rescue_history.iter().any(|record| {
            record.mode == PlannerRecoveryMode::SuccessRecovery && record.action == action
        })
    })
}

async fn checkpoint_planner_candidates(
    project_manager: &ProjectManager,
    paths: &ProjectPaths,
    state: &mut PipelineStateFile,
    candidates: &[ReconstructionCandidate],
) -> Result<()> {
    if let Some(planner) = state.planner.as_mut() {
        planner.reconstruction_candidates = candidates.to_vec();
    }
    project_manager.write_state(&paths.state, state).await?;
    write_planner_snapshot(paths, state).await
}

fn count_sparse_models(root: &Path) -> u32 {
    let root_is_model = ["cameras.bin", "images.bin", "points3D.bin"]
        .iter()
        .all(|name| root.join(name).is_file());
    let children = std::fs::read_dir(root)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
        .filter(|entry| {
            entry.path().is_dir()
                && ["cameras.bin", "images.bin", "points3D.bin"]
                    .iter()
                    .all(|name| entry.path().join(name).is_file())
        })
        .count() as u32;
    children + u32::from(root_is_model)
}

async fn write_planner_snapshot(paths: &ProjectPaths, state: &PipelineStateFile) -> Result<()> {
    let Some(planner) = &state.planner else {
        return Ok(());
    };
    let bytes = serde_json::to_vec_pretty(planner).map_err(|error| {
        SplatError::Process(format!("Unable to serialize Planner log: {error}"))
    })?;
    tokio::fs::write(paths.logs.join("planner.json"), bytes).await?;
    Ok(())
}

fn signed_gain_ratio(after: u64, before: u64) -> Option<f64> {
    (before > 0).then(|| (after as f64 - before as f64) / before as f64)
}

async fn rollback_geometry_probe_frames(
    paths: &ProjectPaths,
    additions: &[crate::video::PlannedFrame],
    has_alpha: bool,
) {
    let extension = if has_alpha { "png" } else { "jpg" };
    for frame in additions {
        let image = paths.frames.join(format!(
            "frame_{:010}.{extension}",
            frame.source_frame_index
        ));
        if image.is_file() {
            let _ = tokio::fs::remove_file(image).await;
        }
        if has_alpha {
            let mask = paths
                .masks
                .join(format!("frame_{:010}.png.png", frame.source_frame_index));
            if mask.is_file() {
                let _ = tokio::fs::remove_file(mask).await;
            }
        }
    }
}

async fn write_backfill_pairs(
    frames: &Path,
    new_names: &[String],
    overlap: usize,
    destination: &Path,
) -> Result<()> {
    let mut names = Vec::new();
    let mut entries = tokio::fs::read_dir(frames).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.is_file()
            && path.extension().is_some_and(|extension| {
                extension.eq_ignore_ascii_case("jpg")
                    || extension.eq_ignore_ascii_case("jpeg")
                    || extension.eq_ignore_ascii_case("png")
            })
        {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    let new_names: std::collections::HashSet<&str> = new_names.iter().map(String::as_str).collect();
    let mut pairs = std::collections::BTreeSet::new();
    for (position, name) in names.iter().enumerate() {
        if !new_names.contains(name.as_str()) {
            continue;
        }
        let start = position.saturating_sub(overlap);
        let end = (position + overlap + 1).min(names.len());
        for neighbor in &names[start..end] {
            if neighbor == name {
                continue;
            }
            let (left, right) = if name < neighbor {
                (name, neighbor)
            } else {
                (neighbor, name)
            };
            pairs.insert(format!("{left} {right}"));
        }
    }
    if pairs.is_empty() {
        return Err(SplatError::Process(
            "Planner backfill produced no related image pairs".into(),
        ));
    }
    tokio::fs::write(
        destination,
        pairs.into_iter().collect::<Vec<_>>().join("\n"),
    )
    .await?;
    Ok(())
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

    #[tokio::test]
    async fn failed_geometry_probe_removes_only_its_added_frames() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), temporary.path().to_path_buf());
        tokio::fs::create_dir_all(&paths.frames).await.unwrap();
        tokio::fs::create_dir_all(&paths.masks).await.unwrap();
        let original = paths.frames.join("frame_0000000001.png");
        let added = paths.frames.join("frame_0000000002.png");
        let mask = paths.masks.join("frame_0000000002.png.png");
        tokio::fs::write(&original, b"original").await.unwrap();
        tokio::fs::write(&added, b"probe").await.unwrap();
        tokio::fs::write(&mask, b"mask").await.unwrap();

        rollback_geometry_probe_frames(
            &paths,
            &[crate::video::PlannedFrame {
                source_frame_index: 2,
                timestamp_seconds: 0.2,
            }],
            true,
        )
        .await;

        assert!(original.is_file());
        assert!(!added.exists());
        assert!(!mask.exists());
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
            quality: None,
            retention_ratio: 0.5,
            sampling_fps: 15.0,
            actual_average_fps: 15.0,
            target_fps: 15.0,
            candidate_fps: 15.0,
            estimated_frames: 100,
            planning_mode: Default::default(),
            preferred_fps: 0.0,
            selected_frames: Vec::new(),
            candidate_frames: Vec::new(),
            minimum_frame_protection: Default::default(),
            extracted_frames: Some(100),
            image_format: Some("jpeg".into()),
            mask_count: Some(0),
            has_alpha: false,
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
            quality: None,
            retention_ratio: 0.5,
            sampling_fps: 15.0,
            actual_average_fps: 15.0,
            target_fps: 15.0,
            candidate_fps: 15.0,
            estimated_frames: 100,
            planning_mode: Default::default(),
            preferred_fps: 0.0,
            selected_frames: Vec::new(),
            candidate_frames: Vec::new(),
            minimum_frame_protection: Default::default(),
            extracted_frames: Some(100),
            image_format: Some("jpeg".into()),
            mask_count: Some(0),
            has_alpha: false,
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

    #[test]
    fn resume_reuses_the_recorded_frame_plan_without_replanning() {
        let mut state = PipelineStateFile::created(Quality::Fast);
        state.planner_enabled = true;
        state.frames = Some(FrameState {
            quality: Some(Quality::Fast),
            retention_ratio: 0.17,
            sampling_fps: 5.1,
            actual_average_fps: 5.1,
            target_fps: 5.1,
            candidate_fps: 12.0,
            planning_mode: FramePlanningMode::Budgeted,
            preferred_fps: 6.0,
            selected_frames: vec![crate::video::PlannedFrame {
                source_frame_index: 42,
                timestamp_seconds: 1.4,
            }],
            candidate_frames: vec![crate::video::PlannedFrame {
                source_frame_index: 42,
                timestamp_seconds: 1.4,
            }],
            minimum_frame_protection: Default::default(),
            estimated_frames: 1,
            extracted_frames: None,
            image_format: None,
            mask_count: None,
            has_alpha: false,
        });
        let restored = frame_plan_from_state(state.frames.as_ref().unwrap());
        assert_eq!(restored.planning_mode, FramePlanningMode::Budgeted);
        assert_eq!(restored.sampling_fps, 5.1);
        assert_eq!(restored.selected_frames[0].source_frame_index, 42);
    }

    #[tokio::test]
    async fn frame_checkpoint_requires_every_recorded_frame() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::nil(), temporary.path().to_path_buf());
        tokio::fs::create_dir_all(&paths.frames).await.unwrap();
        tokio::fs::write(paths.frames.join("frame_000001.jpg"), b"one")
            .await
            .unwrap();
        tokio::fs::write(paths.frames.join("frame_000002.jpg"), b"two")
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
            quality: None,
            retention_ratio: 0.5,
            sampling_fps: 15.0,
            actual_average_fps: 15.0,
            target_fps: 15.0,
            candidate_fps: 15.0,
            estimated_frames: 2,
            planning_mode: Default::default(),
            preferred_fps: 0.0,
            selected_frames: Vec::new(),
            candidate_frames: Vec::new(),
            minimum_frame_protection: Default::default(),
            extracted_frames: Some(2),
            image_format: Some("jpeg".into()),
            mask_count: Some(0),
            has_alpha: false,
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
            quality: None,
            retention_ratio: 1.0,
            sampling_fps: 0.0,
            actual_average_fps: 0.0,
            target_fps: 0.0,
            candidate_fps: 0.0,
            estimated_frames: 2,
            planning_mode: Default::default(),
            preferred_fps: 0.0,
            selected_frames: Vec::new(),
            candidate_frames: Vec::new(),
            minimum_frame_protection: Default::default(),
            extracted_frames: Some(2),
            image_format: Some("images".into()),
            mask_count: Some(2),
            has_alpha: true,
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
    async fn opaque_alpha_channel_checkpoint_does_not_require_masks() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::nil(), temporary.path().to_path_buf());
        tokio::fs::create_dir_all(&paths.frames).await.unwrap();
        tokio::fs::create_dir_all(&paths.masks).await.unwrap();
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
            quality: None,
            retention_ratio: 1.0,
            sampling_fps: 0.0,
            actual_average_fps: 0.0,
            target_fps: 0.0,
            candidate_fps: 0.0,
            estimated_frames: 2,
            planning_mode: Default::default(),
            preferred_fps: 0.0,
            selected_frames: Vec::new(),
            candidate_frames: Vec::new(),
            minimum_frame_protection: Default::default(),
            extracted_frames: Some(2),
            image_format: Some("images".into()),
            mask_count: Some(0),
            has_alpha: false,
        });

        let prepared = prepared_frames_from_checkpoint(&paths, &state)
            .await
            .unwrap()
            .unwrap();

        assert!(!prepared.has_alpha);
        assert_eq!(prepared.mask_count, 0);
    }

    #[tokio::test]
    async fn transparent_frame_checkpoint_requires_matching_masks() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::nil(), temporary.path().to_path_buf());
        tokio::fs::create_dir_all(&paths.frames).await.unwrap();
        tokio::fs::create_dir_all(&paths.masks).await.unwrap();
        tokio::fs::write(paths.frames.join("frame_000001.png"), b"rgba")
            .await
            .unwrap();
        tokio::fs::write(paths.masks.join("frame_000001.png.png"), b"mask")
            .await
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
            quality: None,
            retention_ratio: 0.5,
            sampling_fps: 15.0,
            actual_average_fps: 15.0,
            target_fps: 15.0,
            candidate_fps: 15.0,
            estimated_frames: 1,
            planning_mode: Default::default(),
            preferred_fps: 0.0,
            selected_frames: Vec::new(),
            candidate_frames: Vec::new(),
            minimum_frame_protection: Default::default(),
            extracted_frames: Some(1),
            image_format: Some("png".into()),
            mask_count: Some(1),
            has_alpha: true,
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
        tokio::fs::write(paths.frames.join("frame_000001.jpg"), b"jpeg")
            .await
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
            quality: None,
            retention_ratio: 0.5,
            sampling_fps: 15.0,
            actual_average_fps: 15.0,
            target_fps: 15.0,
            candidate_fps: 15.0,
            estimated_frames: 1,
            planning_mode: Default::default(),
            preferred_fps: 0.0,
            selected_frames: Vec::new(),
            candidate_frames: Vec::new(),
            minimum_frame_protection: Default::default(),
            extracted_frames: Some(1),
            image_format: Some("jpeg".into()),
            mask_count: Some(0),
            has_alpha: false,
        });
        state.features_complete = true;
        state.matching_complete = true;

        normalize_checkpoints(&paths, &mut state).await.unwrap();

        assert!(!state.features_complete);
        assert!(!state.matching_complete);
        assert_eq!(state.stage, PipelineStage::ExtractingFrames);
    }

    #[tokio::test]
    async fn planner_resume_validates_the_archived_best_candidate() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::nil(), temporary.path().to_path_buf());
        tokio::fs::create_dir_all(&paths.frames).await.unwrap();
        tokio::fs::create_dir_all(&paths.colmap).await.unwrap();
        tokio::fs::write(paths.frames.join("frame_0000000001.jpg"), b"jpeg")
            .await
            .unwrap();
        tokio::fs::write(paths.colmap.join("database.db"), b"sqlite")
            .await
            .unwrap();
        let model = paths.colmap.join("candidates").join("best").join("0");
        tokio::fs::create_dir_all(&model).await.unwrap();
        let mut one = 1_u64.to_le_bytes().to_vec();
        one.push(0);
        for name in ["cameras.bin", "images.bin", "points3D.bin"] {
            tokio::fs::write(model.join(name), &one).await.unwrap();
        }
        let mut state = PipelineStateFile::created(Quality::Fast);
        state.planner_enabled = true;
        state.video = Some(VideoInfo {
            duration: 1.0,
            width: 640,
            height: 480,
            fps: 30.0,
            total_frames: 30,
            codec: "h264".into(),
            rotation: 0,
            pixel_format: "yuv420p".into(),
            has_alpha: false,
        });
        state.frames = Some(FrameState {
            quality: Some(Quality::Fast),
            retention_ratio: 0.1,
            sampling_fps: 3.0,
            actual_average_fps: 1.0,
            target_fps: 4.0,
            candidate_fps: 12.0,
            estimated_frames: 1,
            planning_mode: FramePlanningMode::Budgeted,
            preferred_fps: 6.0,
            selected_frames: vec![],
            candidate_frames: vec![],
            minimum_frame_protection: Default::default(),
            extracted_frames: Some(1),
            image_format: Some("jpeg".into()),
            mask_count: Some(0),
            has_alpha: false,
        });
        let mut planner =
            PlannerCheckpoint::new(Quality::Fast.budget(), SuccessRecoveryPolicy::default());
        planner.best_reconstruction_id = Some("best".into());
        planner
            .reconstruction_candidates
            .push(ReconstructionCandidate {
                id: "best".into(),
                mapper: MapperBackend::Incremental,
                model_path: model,
                metrics: Default::default(),
                decision: ReconstructionDecision::Warning,
                viability: ReconstructionViability::Viable,
                rescue_round: 0,
            });
        state.planner = Some(planner);
        state.features_complete = true;
        state.matching_complete = true;
        state.reconstruction_complete = true;

        normalize_checkpoints(&paths, &mut state).await.unwrap();
        assert!(state.reconstruction_complete);
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
    fn brush_estimated_progress_advances_and_stops_at_ninety_five_percent() {
        assert_eq!(estimated_brush_progress(0, 100_000), 0.0);
        assert!((estimated_brush_progress(50_000, 100_000) - 0.475).abs() < f32::EPSILON);
        assert!((estimated_brush_progress(100_000, 100_000) - 0.95).abs() < f32::EPSILON);
        assert!((estimated_brush_progress(500_000, 100_000) - 0.95).abs() < f32::EPSILON);
    }

    #[test]
    fn brush_estimated_progress_handles_an_invalid_duration() {
        assert_eq!(estimated_brush_progress(10_000, 0), 0.0);
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
