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
        ffmpeg::{extract_uniform_frames, validate_extraction},
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
    presets::Quality,
    process::{ProcessManager, ProcessObserver, ProcessUpdate},
    project::{
        catalog, FrameState, PipelineStateFile, ProjectInputType, ProjectManager, ProjectMetadata,
        ProjectOutput, ProjectPaths, ProjectStatus, ReshootProvenance,
    },
    reconstruction::{
        ply::inspect_gaussian_ply,
        validator::{ReconstructionQuality, ReconstructionReport, ReconstructionValidator},
    },
    video::{
        prepare_image_sequence, validate_prepared_image_sequence, FramePlan,
        FrameSelectionStrategy, ImageSequenceInfo, UniformRatioFrameSelection, VideoInfo,
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

#[derive(Clone)]
struct EventSink {
    emit: Arc<dyn Fn(PipelineEvent) + Send + Sync>,
    sequence: Arc<AtomicU64>,
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
        let (start, end) = stage_progress_range(stage);
        let progress = stage_progress
            .map(|value| start + (end - start) * value.clamp(0.0, 1.0))
            .unwrap_or(start);
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
}

pub struct PipelineRunner {
    engines: EnginePaths,
    process_manager: ProcessManager,
    events: EventSink,
}

impl PipelineRunner {
    pub fn new(engines: EnginePaths, emit: impl Fn(PipelineEvent) + Send + Sync + 'static) -> Self {
        Self {
            engines,
            process_manager: ProcessManager::new(),
            events: EventSink {
                emit: Arc::new(emit),
                sequence: Arc::new(AtomicU64::new(0)),
                dispatch: Arc::new(std::sync::Mutex::new(())),
                started: Instant::now(),
            },
        }
    }

    pub fn cancel(&self) {
        self.process_manager.cancel();
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
        self.events
            .stage(PipelineStage::ProbingVideo, 0.0, "正在读取视频信息");
        let video = probe_video(
            &self.engines.ffprobe,
            input,
            logs.map(|path| path.join("ffprobe.log")),
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

        self.events
            .stage(PipelineStage::PlanningFrames, 0.0, "正在规划均匀抽帧");
        let plan = UniformRatioFrameSelection.create_plan(&video, &quality.preset());
        self.events.stage(
            PipelineStage::PlanningFrames,
            1.0,
            format!("预计提取 {} 帧", plan.estimated_frames),
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
        let extraction = extract_uniform_frames(
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
        .await?;
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
        })
    }

    pub async fn prepare_images(
        &self,
        input: &Path,
        quality: Quality,
        output: &Path,
        masks: &Path,
    ) -> Result<PreparedFrames> {
        self.events
            .stage(PipelineStage::ProbingVideo, 0.0, "正在分析图片序列");
        let source = input.to_path_buf();
        let image_sequence =
            tokio::task::spawn_blocking(move || crate::video::analyze_image_sequence(&source))
                .await
                .map_err(|error| SplatError::Process(format!("图片序列分析任务失败：{error}")))??;
        self.events.stage(
            PipelineStage::ProbingVideo,
            1.0,
            format!(
                "图片序列 {} 张 · {}×{}{}",
                image_sequence.image_count,
                image_sequence.width,
                image_sequence.height,
                if image_sequence.has_alpha {
                    " · 检测到透明区域"
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
                "正在准备原始图片并生成 COLMAP Alpha Mask"
            } else {
                "正在准备图片序列"
            },
        );
        let source = input.to_path_buf();
        let frames = output.to_path_buf();
        let mask_root = masks.to_path_buf();
        let prepared = tokio::task::spawn_blocking(move || {
            prepare_image_sequence(&source, &frames, &mask_root)
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
        let (paths, mut metadata) = project_manager.create(input, quality).await?;
        let state = PipelineStateFile::created_for(quality, metadata.input_type);
        self.execute_project(project_manager, paths, &mut metadata, state, &acceleration)
            .await
    }

    /// Creates a derived project from an already completed project and fresh reshoot media.
    /// The source project is read-only: its frames and final.ply are never moved or replaced.
    pub async fn generate_reshoot(
        &self,
        source_project_id: uuid::Uuid,
        reshoot_input: &Path,
        quality: Quality,
        projects_root: &Path,
        plan: ReshootPlan,
    ) -> Result<PipelineResult> {
        plan.validate()?;
        let ReshootPlan {
            regions,
            guidance,
            guidance_images,
        } = plan;
        let acceleration = self.verify_pipeline_engines().await?;
        self.events.acceleration(acceleration.clone());
        let (source_root, source_metadata) =
            catalog::load_registered_project(source_project_id).await?;
        if source_metadata.status != ProjectStatus::Completed
            || !source_root.join("final.ply").is_file()
        {
            return Err(SplatError::Process(
                "只能为已完成且包含 final.ply 的项目创建高清补拍".into(),
            ));
        }
        let source_frames = source_root.join("work").join("frames");
        if !source_frames.is_dir() {
            return Err(SplatError::Process(
                "原项目缺少可复用的输入画面，无法融合补拍素材".into(),
            ));
        }
        let source_masks = source_root.join("work").join("masks");
        if source_masks.is_dir()
            && tokio::fs::read_dir(&source_masks)
                .await?
                .next_entry()
                .await?
                .is_some()
        {
            return Err(SplatError::Process(
                "原项目含透明 Mask，当前高清补拍不支持混合透明素材".into(),
            ));
        }

        let project_manager = ProjectManager::with_root(projects_root.to_path_buf());
        let (paths, mut metadata) = project_manager.create(reshoot_input, quality).await?;
        let stored_reshoot_source = metadata.source_path.clone();
        let temporary = paths.work.join("reshoot-frames");
        let temporary_masks = paths.work.join("reshoot-masks");
        let prepared_reshoot = if reshoot_input.is_dir() {
            self.prepare_images(reshoot_input, quality, &temporary, &temporary_masks)
                .await?
        } else {
            self.prepare_frames(
                reshoot_input,
                quality,
                &temporary,
                &temporary_masks,
                Some(&paths.logs),
            )
            .await?
        };
        if prepared_reshoot.has_alpha {
            return Err(SplatError::Process(
                "高清补拍暂不支持透明素材；请导出不含 Alpha 的 JPG/PNG 或 MP4/MOV".into(),
            ));
        }
        reset_directory(&paths.frames).await?;
        copy_merged_frames(&source_frames, &temporary, &paths.frames).await?;
        let original_frame_count = count_image_files(&source_frames).await?;
        let merged_count = original_frame_count + prepared_reshoot.extracted_frames;
        metadata.name = format!("{}_高清补拍", source_metadata.name);
        // Keep the copied reshoot input as the project source. The merged frames are a
        // checkpoint; if it is lost, resume can reconstruct it from provenance.
        metadata.source_path = stored_reshoot_source.clone();
        metadata.input_type = ProjectInputType::Images;
        let guidance_images =
            write_reshoot_guidance_images(&paths.project, &guidance_images).await?;
        metadata.reshoot = Some(ReshootProvenance {
            source_project_id,
            source_project_path: source_root.clone(),
            source_final_ply: source_root.join("final.ply"),
            reshoot_source_path: stored_reshoot_source,
            regions,
            guidance,
            guidance_images,
            original_frame_count,
            reshoot_frame_count: prepared_reshoot.extracted_frames,
        });
        project_manager
            .write_metadata(&paths.metadata, &metadata)
            .await?;
        let mut state = PipelineStateFile::created_for(quality, ProjectInputType::Images);
        state.stage = PipelineStage::ExtractingFrames;
        state.image_sequence = Some(ImageSequenceInfo {
            image_count: merged_count,
            width: 0,
            height: 0,
            has_alpha: false,
            requires_large_sequence_confirmation: merged_count
                > crate::video::LARGE_SEQUENCE_WARNING_COUNT,
        });
        state.frames = Some(FrameState {
            retention_ratio: 1.0,
            sampling_fps: 0.0,
            estimated_frames: merged_count,
            extracted_frames: Some(merged_count),
            image_format: Some("merged".into()),
            mask_count: Some(0),
            has_alpha: false,
        });
        project_manager.write_state(&paths.state, &state).await?;
        self.execute_project(project_manager, paths, &mut metadata, state, &acceleration)
            .await
    }

    pub async fn resume(&self, project_id: uuid::Uuid) -> Result<PipelineResult> {
        let acceleration = self.verify_pipeline_engines().await?;
        self.events.acceleration(acceleration.clone());
        let (project, mut metadata) = catalog::load_registered_project(project_id).await?;
        if metadata.status == ProjectStatus::Completed || project.join("final.ply").is_file() {
            return Err(SplatError::Process("该项目已经完成，无需继续".into()));
        }
        let source_available = if metadata.reshoot.is_some() {
            // A derived reshoot can retain either a video file or an image folder.
            metadata.source_path.exists()
        } else {
            match metadata.input_type {
                ProjectInputType::Video => metadata.source_path.is_file(),
                ProjectInputType::Images => metadata.source_path.is_dir(),
            }
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
            state.stage = if cancelled {
                PipelineStage::Cancelled
            } else {
                PipelineStage::Failed
            };
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
        if state.input_type != metadata.input_type {
            return Err(SplatError::Process(
                "项目输入类型与检查点不一致，无法安全继续".into(),
            ));
        }
        normalize_checkpoints(paths, &mut state).await?;
        project_manager.write_state(&paths.state, &state).await?;
        let prepared =
            if let Some(prepared) = prepared_frames_from_checkpoint(paths, &state).await? {
                self.events.stage(
                    PipelineStage::ExtractingFrames,
                    1.0,
                    format!("已复用 {} 帧检查点", prepared.extracted_frames),
                );
                prepared
            } else {
                reset_directory(&paths.colmap).await?;
                reset_directory(&paths.brush).await?;
                let prepared = if let Some(provenance) = metadata.reshoot.as_ref() {
                    let source_frames = provenance.source_project_path.join("work").join("frames");
                    if !source_frames.is_dir() {
                        return Err(SplatError::Process(
                            "原项目输入画面已缺失，无法继续高清补拍".into(),
                        ));
                    }
                    reset_directory(&paths.frames).await?;
                    reset_directory(&paths.masks).await?;
                    let reshoot_frames = paths.work.join("reshoot-recovery-frames");
                    let reshoot_masks = paths.work.join("reshoot-recovery-masks");
                    reset_directory(&reshoot_frames).await?;
                    reset_directory(&reshoot_masks).await?;
                    let recovered_reshoot = if metadata.source_path.is_dir() {
                        self.prepare_images(
                            &metadata.source_path,
                            quality,
                            &reshoot_frames,
                            &reshoot_masks,
                        )
                        .await?
                    } else {
                        self.prepare_frames(
                            &metadata.source_path,
                            quality,
                            &reshoot_frames,
                            &reshoot_masks,
                            Some(&paths.logs),
                        )
                        .await?
                    };
                    if recovered_reshoot.has_alpha {
                        return Err(SplatError::Process("高清补拍恢复不支持透明素材".into()));
                    }
                    copy_merged_frames(&source_frames, &reshoot_frames, &paths.frames).await?;
                    let extracted_frames = count_image_files(&paths.frames).await?;
                    PreparedFrames {
                        input_type: ProjectInputType::Images,
                        video: None,
                        image_sequence: Some(ImageSequenceInfo {
                            image_count: extracted_frames,
                            width: 0,
                            height: 0,
                            has_alpha: false,
                            requires_large_sequence_confirmation: false,
                        }),
                        plan: FramePlan {
                            retention_ratio: 1.0,
                            sampling_fps: 0.0,
                            estimated_frames: extracted_frames,
                        },
                        extracted_frames,
                        image_format: "merged".into(),
                        mask_count: 0,
                        has_alpha: false,
                    }
                } else {
                    reset_directory(&paths.frames).await?;
                    reset_directory(&paths.masks).await?;
                    match metadata.input_type {
                        ProjectInputType::Video => {
                            self.prepare_frames(
                                &metadata.source_path,
                                quality,
                                &paths.frames,
                                &paths.masks,
                                Some(&paths.logs),
                            )
                            .await?
                        }
                        ProjectInputType::Images => {
                            self.prepare_images(
                                &metadata.source_path,
                                quality,
                                &paths.frames,
                                &paths.masks,
                            )
                            .await?
                        }
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
                state.features_complete = false;
                state.matching_complete = false;
                state.reconstruction_complete = false;
                state.brush_complete = false;
                state.stage = PipelineStage::ExtractingFrames;
                project_manager.write_state(&paths.state, &state).await?;
                prepared
            };
        let source_duration_seconds = prepared.video.as_ref().map(|video| video.duration);

        let database = paths.colmap.join("database.db");
        let sparse = paths.colmap.join("sparse");
        let colmap_log = paths.logs.join("colmap.log");
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
            colmap::extract_features(
                &self.engines.colmap,
                &database,
                colmap_images,
                colmap_masks,
                colmap_log.clone(),
                &self.process_manager,
                Some(self.process_observer(
                    PipelineStage::ExtractingFeatures,
                    PipelineEngine::Colmap,
                    Some(prepared.extracted_frames),
                    ObserverMode::BracketProgress,
                )),
                gpu_index,
            )
            .await?;
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
                colmap_log,
                &self.process_manager,
                Some(self.process_observer(
                    PipelineStage::Reconstructing,
                    PipelineEngine::Colmap,
                    Some(prepared.extracted_frames),
                    ObserverMode::Mapper,
                )),
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
        let (model, report) = best_sparse_model(&paths.frames, &sparse)?;
        let warning = (report.quality == ReconstructionQuality::Warning).then(|| {
            format!(
                "注册率 {:.1}%：低于 80%，将继续训练，但结果质量可能受影响",
                report.registered_ratio * 100.0
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

        let preset = quality.preset();
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
                    preset.brush_iterations,
                    preset.brush_max_resolution,
                    format_duration(estimated_brush_duration_ms)
                ),
                Some(0),
                Some(preset.brush_iterations as u64),
                Some("iterations"),
            );
            let candidate = brush::train(
                &self.engines.brush,
                &dataset,
                &paths.brush,
                preset,
                paths.logs.join("brush.log"),
                &self.process_manager,
                Some(self.process_observer(
                    PipelineStage::TrainingSplats,
                    PipelineEngine::Brush,
                    Some(preset.brush_iterations as u64),
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
        tokio::fs::rename(&candidate, &final_ply).await?;
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
    if let Some(value) =
        value_after(line, "num_reg_frames=").or_else(|| value_after(line, "num_reg_frames ="))
    {
        let current = value.parse().ok()?;
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

/// Everything the user selected in the preview for one reshoot run.
#[derive(Debug, Clone, Default)]
pub struct ReshootPlan {
    pub regions: Vec<crate::project::GaussianCrop>,
    pub guidance: Vec<String>,
    /// PNG data URLs: the circled region plus arrows for the shooting positions.
    pub guidance_images: Vec<String>,
}

impl ReshootPlan {
    fn validate(&self) -> Result<()> {
        if self.regions.is_empty() {
            return Err(SplatError::Process("请至少圈选一个需要补拍的区域".into()));
        }
        ensure_distinct_reshoot_regions(&self.regions)?;
        if self.guidance.len() != self.regions.len()
            || self.guidance_images.len() != self.regions.len()
        {
            return Err(SplatError::Process(
                "补拍区域与补拍指引数量不一致，请重新圈选区域".into(),
            ));
        }
        Ok(())
    }
}

/// Two selections closer than this describe the same spot, not two regions.
const RESHOOT_REGION_PRECISION: f64 = 1e3;

fn reshoot_region_key(region: &crate::project::GaussianCrop) -> String {
    let round = |value: f64| (value * RESHOOT_REGION_PRECISION).round() as i64;
    match region {
        crate::project::GaussianCrop::Sphere { center, radius } => format!(
            "sphere:{}:{}:{}:{}",
            round(center[0]),
            round(center[1]),
            round(center[2]),
            round(*radius)
        ),
        crate::project::GaussianCrop::Box { center, size } => format!(
            "box:{}:{}:{}:{}:{}:{}",
            round(center[0]),
            round(center[1]),
            round(center[2]),
            round(size[0]),
            round(size[1]),
            round(size[2])
        ),
    }
}

/// The reshoot list must describe distinct areas: a repeated selection would ask
/// the shooter for the same footage twice and skew the merged frame set.
fn ensure_distinct_reshoot_regions(regions: &[crate::project::GaussianCrop]) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    for region in regions {
        if !seen.insert(reshoot_region_key(region)) {
            return Err(SplatError::Process(
                "补拍清单中存在重复区域，请移除重复项或重新圈选不同区域".into(),
            ));
        }
    }
    Ok(())
}

/// Persist the annotated guidance images inside the derived project so the
/// shooting plan stays traceable next to the frames it belongs to.
async fn write_reshoot_guidance_images(
    project_root: &Path,
    images: &[String],
) -> Result<Vec<PathBuf>> {
    const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let directory = project_root.join("reshoot-guidance");
    let mut written = Vec::new();
    for (index, image) in images.iter().enumerate() {
        if image.trim().is_empty() {
            continue;
        }
        let payload = image
            .split_once(',')
            .filter(|(header, _)| header.starts_with("data:image/png"))
            .map(|(_, payload)| payload)
            .ok_or_else(|| SplatError::Process("补拍指引图格式无效，请重新生成".into()))?;
        let bytes = decode_base64(payload)
            .ok_or_else(|| SplatError::Process("补拍指引图无法解码，请重新生成".into()))?;
        if !bytes.starts_with(&PNG_MAGIC) {
            return Err(SplatError::Process("补拍指引图不是有效的 PNG".into()));
        }
        tokio::fs::create_dir_all(&directory).await?;
        let path = directory.join(format!("region-{:02}.png", index + 1));
        tokio::fs::write(&path, &bytes).await?;
        written.push(path);
    }
    Ok(written)
}

/// Minimal standard-alphabet base64 decoder; the repository ships no base64 crate.
fn decode_base64(input: &str) -> Option<Vec<u8>> {
    let mut buffer = 0u32;
    let mut bits = 0u32;
    let mut output = Vec::with_capacity(input.len() / 4 * 3);
    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\n' | b'\r' => continue,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(output)
}

async fn count_image_files(directory: &Path) -> Result<u64> {
    let directory = directory.to_path_buf();
    tokio::task::spawn_blocking(move || {
        Ok::<u64, SplatError>(crate::video::list_images(&directory)?.len() as u64)
    })
    .await
    .map_err(|error| SplatError::Process(format!("无法统计输入画面：{error}")))?
}

/// Copy original and reshoot frames into one stable, contiguous image sequence.
/// Source material remains untouched; the derived project exclusively owns `destination`.
async fn copy_merged_frames(original: &Path, reshoot: &Path, destination: &Path) -> Result<()> {
    let original = original.to_path_buf();
    let reshoot = reshoot.to_path_buf();
    let destination = destination.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let mut sources = crate::video::list_images(&original)?;
        sources.extend(crate::video::list_images(&reshoot)?);
        if sources.len() < 2 {
            return Err(SplatError::Process("融合后至少需要 2 张有效画面".into()));
        }
        std::fs::create_dir_all(&destination)?;
        for (index, source) in sources.iter().enumerate() {
            let extension = source
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("jpg")
                .to_ascii_lowercase();
            let target = destination.join(format!("frame_{index:06}.{extension}"));
            std::fs::copy(source, target)?;
        }
        Ok(())
    })
    .await
    .map_err(|error| SplatError::Process(format!("融合输入画面失败：{error}")))?
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
    state.reconstruction_complete = state.matching_complete
        && state.reconstruction_complete
        && best_sparse_model(&paths.frames, &paths.colmap.join("sparse")).is_ok();
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
            Ok(Some(PreparedFrames {
                input_type: ProjectInputType::Video,
                video: Some(video),
                image_sequence: None,
                plan,
                extracted_frames,
                image_format: extraction.image_format.as_str().into(),
                mask_count: extraction.mask_count,
                has_alpha: extraction.has_alpha,
            }))
        }
        ProjectInputType::Images => {
            let Some(image_sequence) = state.image_sequence.clone() else {
                return Ok(None);
            };
            let frames_dir = paths.frames.clone();
            let masks_dir = paths.masks.clone();
            let Ok(prepared) = tokio::task::spawn_blocking(move || {
                validate_prepared_image_sequence(
                    &frames_dir,
                    &masks_dir,
                    extracted_frames,
                    image_sequence.has_alpha,
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
            }))
        }
    }
}

fn brush_candidate(root: &Path) -> Option<PathBuf> {
    [root.join("final.ply.tmp"), root.join("final.ply.tmp.ply")]
        .into_iter()
        .find(|path| path.is_file())
}

async fn reset_directory(path: &Path) -> Result<()> {
    if path.exists() {
        tokio::fs::remove_dir_all(path).await?;
    }
    tokio::fs::create_dir_all(path).await?;
    Ok(())
}

fn best_sparse_model(frames: &Path, sparse: &Path) -> Result<(PathBuf, ReconstructionReport)> {
    let mut best: Option<(PathBuf, ReconstructionReport)> = None;
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
    use super::{copy_merged_frames, count_image_files};
    use std::path::Path;

    #[tokio::test]
    async fn merged_frames_are_contiguous_and_sources_remain_unchanged() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("original");
        let reshoot = root.path().join("reshoot");
        let destination = root.path().join("merged");
        std::fs::create_dir_all(&original).unwrap();
        std::fs::create_dir_all(&reshoot).unwrap();
        std::fs::write(original.join("frame_9.jpg"), b"original").unwrap();
        std::fs::write(reshoot.join("hi_1.png"), b"reshoot").unwrap();
        copy_merged_frames(
            Path::new(&original),
            Path::new(&reshoot),
            Path::new(&destination),
        )
        .await
        .unwrap();
        assert_eq!(count_image_files(Path::new(&original)).await.unwrap(), 1);
        assert_eq!(count_image_files(Path::new(&reshoot)).await.unwrap(), 1);
        assert!(destination.join("frame_000000.jpg").is_file());
        assert!(destination.join("frame_000001.png").is_file());
        assert!(!original.join("frame_000000.jpg").exists());
    }

    use super::*;

    fn sphere_region(center: [f64; 3], radius: f64) -> crate::project::GaussianCrop {
        crate::project::GaussianCrop::Sphere { center, radius }
    }

    #[test]
    fn rejects_a_repeated_reshoot_region() {
        let regions = vec![
            sphere_region([1.0, 2.0, 3.0], 0.5),
            sphere_region([1.0, 2.0, 3.0], 0.5),
        ];
        let error = ensure_distinct_reshoot_regions(&regions).unwrap_err();
        assert!(error.to_string().contains("重复区域"));
    }

    #[test]
    fn accepts_distinct_reshoot_regions() {
        let regions = vec![
            sphere_region([1.0, 2.0, 3.0], 0.5),
            sphere_region([1.0, 2.0, 3.4], 0.5),
            crate::project::GaussianCrop::Box {
                center: [1.0, 2.0, 3.0],
                size: [1.0, 1.0, 1.0],
            },
        ];
        assert!(ensure_distinct_reshoot_regions(&regions).is_ok());
    }

    #[test]
    fn decodes_png_data_urls_and_rejects_other_payloads() {
        // "iVBORw0KGgo=" is the base64 form of the eight PNG signature bytes.
        let signature = [0x89u8, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        assert_eq!(decode_base64("iVBORw0KGgo=").unwrap(), signature);
        assert!(decode_base64("####").is_none());
        assert!(decode_base64("aGVsbG8=").is_some());
    }

    #[tokio::test]
    async fn writes_guidance_images_into_the_derived_project() {
        let root = tempfile::tempdir().unwrap();
        let signature = [0x89u8, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let images = vec![
            "data:image/png;base64,iVBORw0KGgo=".to_string(),
            String::new(),
        ];
        let written = write_reshoot_guidance_images(root.path(), &images)
            .await
            .unwrap();

        assert_eq!(written.len(), 1);
        assert_eq!(
            written[0],
            root.path().join("reshoot-guidance").join("region-01.png")
        );
        assert_eq!(std::fs::read(&written[0]).unwrap(), signature);
        assert!(!root
            .path()
            .join("reshoot-guidance")
            .join("region-02.png")
            .exists());
    }

    #[tokio::test]
    async fn rejects_a_guidance_image_that_is_not_png() {
        let root = tempfile::tempdir().unwrap();
        let images = vec!["data:image/png;base64,aGVsbG8=".to_string()];
        let error = write_reshoot_guidance_images(root.path(), &images)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("PNG"));
        assert!(!root.path().join("reshoot-guidance").exists());
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
        });
        state.features_complete = true;
        state.matching_complete = true;
        assert_eq!(checkpoint_stage(&state), PipelineStage::Matching);

        state.reconstruction_complete = true;
        state.brush_complete = true;
        assert_eq!(checkpoint_stage(&state), PipelineStage::TrainingSplats);
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
            retention_ratio: 0.5,
            sampling_fps: 15.0,
            estimated_frames: 2,
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
            retention_ratio: 0.5,
            sampling_fps: 15.0,
            estimated_frames: 1,
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
            retention_ratio: 0.5,
            sampling_fps: 15.0,
            estimated_frames: 1,
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
            dispatch: Arc::new(std::sync::Mutex::new(())),
            started: Instant::now(),
        };
        sink.stage(PipelineStage::Created, 0.0, "created");
        sink.stage(PipelineStage::ProbingVideo, 0.0, "probing");
        assert_eq!(*events.lock().unwrap(), vec![1, 2]);
    }
}
