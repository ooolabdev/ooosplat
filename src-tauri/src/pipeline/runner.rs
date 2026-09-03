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
        brush, colmap, ffmpeg::extract_uniform_frames, ffprobe::probe_video, EngineKind,
        EnginePaths,
    },
    error::{Result, SplatError},
    pipeline::{
        progress::stage_progress_range, EventKind, EventLevel, PipelineEngine, PipelineEvent,
        PipelineStage,
    },
    presets::Quality,
    process::{ProcessManager, ProcessObserver, ProcessUpdate},
    project::{
        catalog, FrameState, PipelineStateFile, ProjectManager, ProjectMetadata, ProjectOutput,
        ProjectPaths, ProjectStatus,
    },
    reconstruction::{
        ply::inspect_gaussian_ply,
        validator::{ReconstructionQuality, ReconstructionReport, ReconstructionValidator},
    },
    video::{FramePlan, FrameSelectionStrategy, UniformRatioFrameSelection, VideoInfo},
};

pub struct PreparedFrames {
    pub video: VideoInfo,
    pub plan: FramePlan,
    pub extracted_frames: u64,
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
    pub(crate) source_duration_seconds: f64,
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
        self.events.stage(
            PipelineStage::ProbingVideo,
            1.0,
            format!(
                "视频 {:.1} 秒 · {:.2} FPS · {}×{}",
                video.duration, video.fps, video.width, video.height
            ),
        );

        self.events
            .stage(PipelineStage::PlanningFrames, 0.0, "正在规划均匀抽帧");
        let plan = UniformRatioFrameSelection.create_plan(&video, &quality.preset());
        self.events.stage(
            PipelineStage::PlanningFrames,
            1.0,
            format!("预计提取 {} 帧", plan.estimated_frames),
        );

        self.events
            .stage(PipelineStage::ExtractingFrames, 0.0, "FFmpeg 开始提取画面");
        let observer = self.process_observer(
            PipelineStage::ExtractingFrames,
            PipelineEngine::Ffmpeg,
            Some(plan.estimated_frames),
            ObserverMode::Ffmpeg,
        );
        let extracted_frames = extract_uniform_frames(
            &self.engines.ffmpeg,
            input,
            output,
            &plan,
            logs.map(|path| path.join("ffmpeg.log")),
            &self.process_manager,
            Some(observer),
        )
        .await?;
        self.events.stage(
            PipelineStage::ExtractingFrames,
            1.0,
            format!("已提取 {extracted_frames} 帧"),
        );
        Ok(PreparedFrames {
            video,
            plan,
            extracted_frames,
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
        let state = PipelineStateFile::created(quality);
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
        if !metadata.source_path.is_file() {
            return Err(SplatError::Process("项目源视频缺失，无法继续".into()));
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
                .unwrap_or_else(|_| PipelineStateFile::created(metadata.quality));
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
                reset_directory(&paths.frames).await?;
                reset_directory(&paths.colmap).await?;
                reset_directory(&paths.brush).await?;
                let prepared = self
                    .prepare_frames(
                        &metadata.source_path,
                        quality,
                        &paths.frames,
                        Some(&paths.logs),
                    )
                    .await?;
                state.video = Some(prepared.video.clone());
                let mut frames = FrameState::from(&prepared.plan);
                frames.extracted_frames = Some(prepared.extracted_frames);
                state.frames = Some(frames);
                state.features_complete = false;
                state.matching_complete = false;
                state.reconstruction_complete = false;
                state.brush_complete = false;
                state.stage = PipelineStage::ExtractingFrames;
                project_manager.write_state(&paths.state, &state).await?;
                prepared
            };
        let source_duration_seconds = prepared.video.duration;

        let database = paths.colmap.join("database.db");
        let sparse = paths.colmap.join("sparse");
        let colmap_log = paths.logs.join("colmap.log");
        // COLMAP's bundled bitmap loader cannot reliably open non-ASCII absolute
        // paths on Windows. The process working directory is work/colmap, so this
        // ASCII-only relative path preserves Unicode/UNC project roots without
        // moving any project data outside the project directory.
        let colmap_images = Path::new("../frames");

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
            self.events
                .stage(PipelineStage::Matching, 1.0, "已复用跨视角匹配检查点");
        } else {
            self.events.stage(
                PipelineStage::Matching,
                0.0,
                format!("COLMAP 正在进行 {backend_label} 跨视角匹配"),
            );
            colmap::match_exhaustive(
                &self.engines.colmap,
                &database,
                colmap_log.clone(),
                &self.process_manager,
                Some(self.process_observer(
                    PipelineStage::Matching,
                    PipelineEngine::Colmap,
                    Some(prepared.extracted_frames),
                    ObserverMode::BracketProgress,
                )),
                gpu_index,
            )
            .await?;
            state.stage = PipelineStage::Matching;
            state.matching_complete = true;
            state.matching_strategy_version = 1;
            project_manager.write_state(&paths.state, &state).await?;
            self.events
                .stage(PipelineStage::Matching, 1.0, "跨视角匹配完成");
        }

        if state.reconstruction_complete {
            self.events
                .stage(PipelineStage::Reconstructing, 1.0, "已复用相机重建检查点");
        } else {
            reset_directory(&sparse).await?;
            let global_sparse = sparse.join("global");
            self.events.stage(
                PipelineStage::Reconstructing,
                0.0,
                "正在使用 Global Mapper 快速重建相机轨迹",
            );
            let global_result = colmap::map_global(
                &self.engines.colmap,
                &database,
                colmap_images,
                &global_sparse,
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
            .await;

            let global_report = match global_result {
                Ok(()) => best_sparse_model(&paths.frames, &global_sparse).ok(),
                Err(SplatError::Cancelled) => return Err(SplatError::Cancelled),
                Err(error) => {
                    self.events.send(
                        PipelineStage::Reconstructing,
                        Some(PipelineEngine::Colmap),
                        EventKind::Log,
                        EventLevel::Warning,
                        None,
                        false,
                        format!("Global Mapper 不可用，将回退增量重建：{error}"),
                        None,
                        None,
                        None,
                    );
                    None
                }
            };
            let global_is_good = global_report
                .as_ref()
                .is_some_and(|(_, report)| report.quality == ReconstructionQuality::Good);
            if !global_is_good {
                if let Some((_, report)) = &global_report {
                    self.events.send(
                        PipelineStage::Reconstructing,
                        Some(PipelineEngine::Colmap),
                        EventKind::Log,
                        EventLevel::Warning,
                        None,
                        false,
                        format!(
                            "Global Mapper 注册率 {:.1}%，正在回退增量重建",
                            report.registered_ratio * 100.0
                        ),
                        None,
                        None,
                        None,
                    );
                }
                let incremental_sparse = sparse.join("incremental");
                self.events.stage(
                    PipelineStage::Reconstructing,
                    0.05,
                    "正在使用增量 Mapper 进行稳健回退",
                );
                let incremental_result = colmap::map_incremental(
                    &self.engines.colmap,
                    &database,
                    colmap_images,
                    &incremental_sparse,
                    colmap_log,
                    &self.process_manager,
                    Some(self.process_observer(
                        PipelineStage::Reconstructing,
                        PipelineEngine::Colmap,
                        Some(prepared.extracted_frames),
                        ObserverMode::Mapper,
                    )),
                )
                .await;
                if let Err(error) = incremental_result {
                    if matches!(error, SplatError::Cancelled) || global_report.is_none() {
                        return Err(error);
                    }
                    self.events.send(
                        PipelineStage::Reconstructing,
                        Some(PipelineEngine::Colmap),
                        EventKind::Log,
                        EventLevel::Warning,
                        None,
                        false,
                        "增量重建失败，继续使用可用的 Global Mapper 结果",
                        None,
                        None,
                        None,
                    );
                }
            }
            state.stage = PipelineStage::Reconstructing;
            state.reconstruction_complete = true;
            project_manager.write_state(&paths.state, &state).await?;
            self.events.stage(
                PipelineStage::Reconstructing,
                1.0,
                if global_is_good {
                    "Global Mapper 快速重建完成"
                } else {
                    "相机轨迹重建完成"
                },
            );
        }

        self.events.stage(
            PipelineStage::ValidatingReconstruction,
            0.0,
            "正在核验注册率和三维点",
        );
        let (model, report) = best_sparse_model(&paths.frames, &sparse)?;
        if report.quality == ReconstructionQuality::Failed {
            return Err(SplatError::Process(format!(
                "素材重建失败：注册 {}/{} 张（{:.1}%），低于 50% 阈值",
                report.registered_images,
                report.input_images,
                report.registered_ratio * 100.0
            )));
        }
        let mut warning = (report.quality == ReconstructionQuality::Warning).then(|| {
            format!(
                "注册率 {:.1}%：低于 80%，结果质量可能受影响",
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

        let total_vram_mb = acceleration
            .device
            .as_ref()
            .and_then(|device| device.total_memory_mb);
        let preset = quality.preset().for_vram_mb(total_vram_mb);
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
            self.events.send(
                PipelineStage::TrainingSplats,
                Some(PipelineEngine::Brush),
                EventKind::Stage,
                EventLevel::Info,
                None,
                true,
                format!(
                    "Brush 训练开始 · {} iterations · 最大分辨率 {} · 最多 {} Splats · SH {}",
                    preset.brush_iterations,
                    preset.brush_max_resolution,
                    preset.brush_max_splats,
                    preset.brush_sh_degree
                ),
                Some(0),
                Some(preset.brush_iterations as u64),
                Some("iterations"),
            );
            let observer = self.process_observer(
                PipelineStage::TrainingSplats,
                PipelineEngine::Brush,
                Some(preset.brush_iterations as u64),
                ObserverMode::Brush,
            );
            let first_attempt = brush::train(
                &self.engines.brush,
                &dataset,
                &paths.brush,
                preset,
                paths.logs.join("brush.log"),
                &self.process_manager,
                Some(observer.clone()),
            )
            .await;
            let candidate = match first_attempt {
                Ok(candidate) => candidate,
                Err(error) if brush::is_out_of_memory(&error) => {
                    let retry = preset.degraded_for_oom();
                    warning = Some(match warning {
                        Some(existing) => {
                            format!("{existing}；训练曾遇到显存不足，已自动降低分辨率和 Splat 上限")
                        }
                        None => "训练曾遇到显存不足，已自动降低分辨率和 Splat 上限".into(),
                    });
                    self.events.send(
                        PipelineStage::TrainingSplats,
                        Some(PipelineEngine::Brush),
                        EventKind::Stage,
                        EventLevel::Warning,
                        None,
                        true,
                        format!(
                            "显存不足，自动重试 · 最大分辨率 {} · 最多 {} Splats",
                            retry.brush_max_resolution, retry.brush_max_splats
                        ),
                        Some(0),
                        Some(retry.brush_iterations as u64),
                        Some("iterations"),
                    );
                    brush::train(
                        &self.engines.brush,
                        &dataset,
                        &paths.brush,
                        retry,
                        paths.logs.join("brush.log"),
                        &self.process_manager,
                        Some(observer),
                    )
                    .await?
                }
                Err(error) => return Err(error),
            };
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
            ProcessUpdate::Heartbeat { elapsed_ms } if mode == ObserverMode::Brush => events.send(
                stage,
                Some(engine),
                EventKind::Heartbeat,
                EventLevel::Info,
                None,
                true,
                format!("Brush 正在运行 · 已用时 {}", format_duration(elapsed_ms)),
                None,
                expected_total,
                Some("iterations"),
            ),
            ProcessUpdate::Heartbeat { .. } => {}
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
                    ObserverMode::Brush => None,
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
                } else if mode == ObserverMode::Brush || is_useful_line(&line) {
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

async fn normalize_checkpoints(paths: &ProjectPaths, state: &mut PipelineStateFile) -> Result<()> {
    let frames_complete = prepared_frames_from_checkpoint(paths, state)
        .await?
        .is_some();
    if !frames_complete {
        state.video = None;
        state.frames = None;
    }

    state.features_complete =
        frames_complete && state.features_complete && paths.colmap.join("database.db").is_file();
    state.matching_complete =
        state.features_complete && state.matching_complete && state.matching_strategy_version == 1;
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
    let Some(video) = state.video.clone() else {
        return Ok(None);
    };
    let Some(frames) = state.frames.as_ref() else {
        return Ok(None);
    };
    let Some(extracted_frames) = frames.extracted_frames.filter(|count| *count > 0) else {
        return Ok(None);
    };
    let mut actual_frames = 0;
    if paths.frames.is_dir() {
        let mut entries = tokio::fs::read_dir(&paths.frames).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry
                .path()
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("jpg"))
            {
                actual_frames += 1;
            }
        }
    }
    if actual_frames != extracted_frames {
        return Ok(None);
    }
    Ok(Some(PreparedFrames {
        video,
        plan: FramePlan {
            retention_ratio: frames.retention_ratio,
            sampling_fps: frames.sampling_fps,
            estimated_frames: frames.estimated_frames,
        },
        extracted_frames,
    }))
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
    let mut candidates = vec![sparse.to_path_buf()];
    for entry in std::fs::read_dir(sparse)? {
        let path = entry?.path();
        if path.is_dir() {
            candidates.push(path.clone());
            for nested in std::fs::read_dir(path)? {
                let nested = nested?.path();
                if nested.is_dir() {
                    candidates.push(nested);
                }
            }
        }
    }
    for path in candidates {
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
    use super::*;

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
        });
        state.frames = Some(FrameState {
            retention_ratio: 0.5,
            sampling_fps: 15.0,
            estimated_frames: 2,
            extracted_frames: Some(2),
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
