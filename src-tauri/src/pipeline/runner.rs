use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Instant,
};

#[cfg(test)]
use crate::planner::plan_bridge_backfill;
#[cfg(test)]
use crate::presets::resolve_panorama_training_preset;
#[cfg(test)]
use crate::presets::{resolve_brush_training_preset, resolve_brush_training_preset_for_plan};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;

#[path = "application_activity.rs"]
mod application_activity;
#[cfg(test)]
#[path = "continuation_reliability_tests.rs"]
mod continuation_reliability_tests;
#[path = "feature_progress.rs"]
mod feature_progress;
#[path = "gps_runner.rs"]
mod gps_runner;
use feature_progress::{FeatureGroupProgress, FeatureProgressScope};
#[cfg(test)]
#[path = "feature_progress_tests.rs"]
mod feature_progress_tests;
#[path = "input_dataset.rs"]
mod input_dataset;
#[path = "inputs_runner.rs"]
mod inputs_runner;
#[path = "legacy_bridge_recovery.rs"]
mod legacy_bridge_recovery;
#[path = "legacy_work_order.rs"]
mod legacy_work_order;
#[cfg(test)]
#[path = "mapper_progress_tests.rs"]
mod mapper_progress_tests;
#[path = "prepared_recovery.rs"]
mod prepared_recovery;
#[cfg(test)]
#[path = "repeated_reshoot_validation.rs"]
mod repeated_reshoot_validation;
#[path = "reshoot_runner.rs"]
mod reshoot_runner;
#[path = "retry_archive.rs"]
mod retry_archive;
#[path = "working_database.rs"]
mod working_database;

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
        estimate::{estimate_runtime, estimate_runtime_for_images, RuntimeEstimate},
        progress::stage_progress_range,
        retry_preparation::{
            PreparationContext, PreparationDiagnosticContext, RetryPreparationOperation,
        },
        runtime::{RuntimeSnapshot, RuntimeTracker},
        EventKind, EventLevel, PipelineEngine, PipelineEvent, PipelineStage,
    },
    planner::{
        bridge_trigger_ratio, plan_bridge_backfill_cancellable, read_registered_source_indices,
        write_bridge_pair_list, BridgeBackfillStatus,
    },
    presets::{
        resolve_planner_resolution_plan, Quality, ResolvedBrushTrainingPreset,
        PLANNER_RESOLUTION_POLICY_VERSION,
    },
    process::{ProcessManager, ProcessObserver, ProcessUpdate},
    project::{
        catalog, FrameState, PipelineStateFile, ProjectImportObserver, ProjectInputType,
        ProjectManager, ProjectMetadata, ProjectOutput, ProjectPaths, ProjectStatus,
        ReshootProvenance, ReshootState, COLMAP_INPUT_VERSION,
    },
    reconstruction::{
        colmap_model::{
            read_registered_images, read_single_camera, reusable_model_files, ColmapCamera,
        },
        ply::inspect_gaussian_ply,
        validator::{ReconstructionQuality, ReconstructionReport, ReconstructionValidator},
    },
    video::{
        image_sequence::{
            prepare_scanned_image_sequence_with_preparation,
            validate_image_sequence_with_preparation,
        },
        scaled_video_dimensions, scan_image_sequence, validate_prepared_image_names,
        validate_prepared_image_sequence, validate_reshoot_image_sequence, video_can_scale_to,
        FramePlan, FrameSelectionStrategy, ImagePreparationObserver, ImagePreparationPhase,
        ImageSequenceInfo, ImageSequenceNaming, PlannedFrame, PreparedImageSequence,
        QualityV2FrameSelection, UniformRatioFrameSelection, VideoInfo,
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

enum LegacyBridgeAttempt {
    Accepted(PathBuf, ReconstructionReport),
    GeometryRejected(SplatError),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Rebuild the public result from the committed publication plus durable local
/// views. Both Runner recovery and TaskService startup use this shape so a
/// crash cannot leave a completed task with only a partial `ProjectOutput`.
pub(crate) fn pipeline_result_from_publication(
    project_root: &Path,
    metadata: &ProjectMetadata,
    state: &PipelineStateFile,
    publication: &crate::pipeline::publication::PublicationOutcome,
) -> PipelineResult {
    let output = &publication.output;
    PipelineResult {
        project_id: metadata.id.to_string(),
        project_path: project_root.to_path_buf(),
        final_ply: output.final_ply.clone(),
        file_size: output.file_size,
        splat_count: output.splat_count,
        input_images: output.input_images,
        registered_images: output.registered_images,
        registered_ratio: output.registered_ratio,
        points_3d: output.points_3d,
        duration_ms: metadata.duration_ms.unwrap_or(0),
        completed_at: publication
            .transaction
            .committed_at
            .or(metadata.completed_at)
            .unwrap_or_else(Utc::now),
        warning: state
            .inputs
            .as_ref()
            .and_then(crate::pipeline::inputs::source_adoption_warning),
        logs_directory: project_root.join("logs"),
        source_duration_seconds: state.video.as_ref().map(|video| video.duration),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReshootSourceInfo {
    pub source_identity: String,
    pub projection: crate::video::panorama::VideoProjection,
    pub warm_available: bool,
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
    pub imported: bool,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video: Option<VideoInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_identity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projection_detection: Option<crate::video::panorama::ProjectionDetection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_sequence: Option<ImageSequenceInfo>,
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
    context: Arc<std::sync::Mutex<OperationContext>>,
    bound_operation: Option<super::event::PipelineOperation>,
    // 0 ordinary; 1 training/panorama; 2/3 legacy ERP preparation/validation;
    // 4 ERP timestamps; 5/6/7 generic preparation/validation/checkpoint; 8 timestamps.
    panorama_phase: Arc<AtomicU64>,
    panorama_images: Arc<AtomicBool>,
    emit: Arc<dyn Fn(PipelineEvent) + Send + Sync>,
    sequence: Arc<AtomicU64>,
    last_progress_milli_percent: Arc<AtomicU64>,
    last_stage: Arc<std::sync::Mutex<Option<PipelineStage>>>,
    dispatch: Arc<std::sync::Mutex<()>>,
    started: Instant,
}

#[derive(Default)]
struct OperationContext {
    source_id: Option<String>,
    serial: u64,
    stage: Option<PipelineStage>,
    substage: Option<String>,
    current: Option<super::event::PipelineOperation>,
    attempts: Vec<(PipelineStage, Option<String>, Option<String>, u32)>,
}

impl EventSink {
    fn begin_application(&self, stage: PipelineStage) -> Self {
        let _dispatch = self.dispatch.lock().unwrap_or_else(|p| p.into_inner());
        let mut context = self.context.lock().unwrap_or_else(|p| p.into_inner());
        context.serial += 1;
        let operation = super::event::PipelineOperation {
            source_id: context.source_id.clone(),
            id: context.serial,
            attempt: 0,
        };
        context.stage = Some(stage);
        context.substage = self.substage(stage);
        context.current = Some(operation.clone());
        let mut scoped = self.clone();
        scoped.bound_operation = Some(operation);
        scoped
    }

    fn training_substage(&self, substage: &str) {
        let phase = match substage {
            "panoramaTrainingPreparation" => 2,
            "panoramaTrainingValidation" => 3,
            "trainingPreparation" => 5,
            "trainingValidation" => 6,
            "trainingCheckpoint" => 7,
            "training" => 1,
            _ => return,
        };
        self.panorama_phase.store(phase, Ordering::Relaxed);
    }

    fn source(&self, id: Option<&str>) {
        self.context
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .source_id = id.map(str::to_owned);
    }
    fn substage(&self, stage: PipelineStage) -> Option<String> {
        match (stage, self.panorama_phase.load(Ordering::Relaxed)) {
            (PipelineStage::ExtractingFrames, 4) => Some("panoramaTimestamps"),
            (PipelineStage::ExtractingFrames, 8) => Some("videoTimestamps"),
            (PipelineStage::ExtractingFrames, 1) => {
                Some(if self.panorama_images.load(Ordering::Relaxed) {
                    "panoramaImagesPreparation"
                } else {
                    "panoramaExtraction"
                })
            }
            (PipelineStage::Reconstructing, 1) => {
                Some(if self.panorama_images.load(Ordering::Relaxed) {
                    "panoramaImagesReconstruction"
                } else {
                    "panoramaReconstruction"
                })
            }
            (PipelineStage::TrainingSplats, 2) => Some("panoramaTrainingPreparation"),
            (PipelineStage::TrainingSplats, 3) => Some("panoramaTrainingValidation"),
            (PipelineStage::TrainingSplats, 5) => Some("trainingPreparation"),
            (PipelineStage::TrainingSplats, 6) => Some("trainingValidation"),
            (PipelineStage::TrainingSplats, 7) => Some("trainingCheckpoint"),
            (PipelineStage::TrainingSplats, _) => Some("training"),
            _ => None,
        }
        .map(str::to_owned)
    }

    fn begin_process(&self, stage: PipelineStage) -> Self {
        self.begin_process_with_key(stage, None)
    }

    fn begin_process_with_key(&self, stage: PipelineStage, logical_group: Option<&str>) -> Self {
        let _dispatch = self.dispatch.lock().unwrap_or_else(|p| p.into_inner());
        let mut context = self.context.lock().unwrap_or_else(|p| p.into_inner());
        let substage = self.substage(stage);
        let attempt = if let Some((_, _, _, count)) =
            context.attempts.iter_mut().find(|(key, sub, group, _)| {
                *key == stage && *sub == substage && group.as_deref() == logical_group
            }) {
            *count += 1;
            *count
        } else {
            context
                .attempts
                .push((stage, substage.clone(), logical_group.map(str::to_owned), 1));
            1
        };
        context.serial += 1;
        let operation = super::event::PipelineOperation {
            source_id: context.source_id.clone(),
            id: context.serial,
            attempt,
        };
        context.stage = Some(stage);
        context.substage = substage;
        context.current = Some(operation.clone());
        let mut scoped = self.clone();
        scoped.bound_operation = Some(operation);
        scoped
    }
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
        let _dispatch = self.dispatch.lock().unwrap_or_else(|p| p.into_inner());
        let mut context = self.context.lock().unwrap_or_else(|p| p.into_inner());
        if self
            .bound_operation
            .as_ref()
            .is_some_and(|op| context.current.as_ref().map(|active| active.id) != Some(op.id))
        {
            return;
        }
        let substage = self.substage(stage);
        if kind == EventKind::Stage
            && (context.stage != Some(stage)
                || context.substage != substage
                || context.current.as_ref().and_then(|o| o.source_id.as_ref())
                    != context.source_id.as_ref())
        {
            context.serial += 1;
            context.current = Some(super::event::PipelineOperation {
                source_id: context.source_id.clone(),
                id: context.serial,
                attempt: 0,
            });
            context.stage = Some(stage);
            context.substage = substage.clone();
        }
        let operation = context.current.clone();
        let (stage, substage) = if matches!(kind, EventKind::Log | EventKind::Heartbeat) {
            (context.stage.unwrap_or(stage), context.substage.clone())
        } else {
            (stage, substage)
        };
        drop(context);
        if kind == EventKind::Stage
            && !matches!(
                stage,
                PipelineStage::Completed | PipelineStage::Failed | PipelineStage::Cancelled
            )
        {
            *self
                .last_stage
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(stage);
        }
        let phase = self.panorama_phase.load(Ordering::Relaxed);
        let (start, end) = if stage == PipelineStage::TrainingSplats && phase != 0 {
            if matches!(phase, 2 | 3 | 5 | 6 | 7) {
                (60.0, 66.0)
            } else {
                (66.0, 98.0)
            }
        } else {
            stage_progress_range(stage)
        };
        let progress = stage_progress
            .map(|value| start + (end - start) * value.clamp(0.0, 1.0))
            .unwrap_or(start);
        let progress = if stage != PipelineStage::Completed {
            progress.min(99.9)
        } else {
            progress
        };
        self.last_progress_milli_percent.fetch_max(
            (progress.max(0.0) * 1_000.0).round() as u64,
            Ordering::Relaxed,
        );
        let stage_percent = stage_progress.map(|value| {
            if stage == PipelineStage::TrainingSplats && phase != 0 {
                (start + (end - start) * value.clamp(0.0, 1.0) - 60.0) / 38.0 * 100.0
            } else {
                value.clamp(0.0, 1.0) * 100.0
            }
        });
        let progress = self.last_progress_milli_percent.load(Ordering::Relaxed) as f32 / 1_000.0;
        (self.emit)(PipelineEvent {
            operation,
            task_id: None,
            run_id: None,
            revision: 0,
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed) + 1,
            timestamp: Utc::now(),
            kind,
            level,
            stage,
            substage,
            engine,
            progress,
            stage_progress: stage_percent,
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
            operation: None,
            task_id: None,
            run_id: None,
            revision: 0,
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed) + 1,
            timestamp: Utc::now(),
            kind: EventKind::Capability,
            level: if status.use_gpu() {
                EventLevel::Info
            } else {
                EventLevel::Warning
            },
            stage: PipelineStage::Created,
            substage: None,
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
        let context = self.context.lock().unwrap_or_else(|p| p.into_inner());
        if self
            .bound_operation
            .as_ref()
            .is_some_and(|op| context.current.as_ref().map(|active| active.id) != Some(op.id))
        {
            return;
        }
        (self.emit)(PipelineEvent {
            operation: context.current.clone(),
            task_id: None,
            run_id: None,
            revision: 0,
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed) + 1,
            timestamp: Utc::now(),
            kind: EventKind::Runtime,
            level: EventLevel::Info,
            stage,
            substage: context.substage.clone(),
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
            operation: self
                .context
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .current
                .clone(),
            task_id: None,
            run_id: None,
            revision: 0,
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
            substage: None,
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

#[derive(Debug, Clone)]
pub enum RunnerUpdate {
    /// Telemetry-only boundary: a new attempt must not borrow the previous GPU.
    BrushPreparation,
    /// A quality/algorithm-changing retry actually started in this run.
    AutomaticRetry,
    Configuration(serde_json::Value),
    Project(Box<crate::project::ProjectMetadata>),
    LogOpened {
        path: PathBuf,
        offset: u64,
    },
    ProcessExited {
        engine: PipelineEngine,
        exit_code: Option<i32>,
    },
}

type ProjectReadyFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>;
type ProjectReadyObserver = Arc<dyn Fn(ProjectMetadata) -> ProjectReadyFuture + Send + Sync>;

pub struct PipelineRunner {
    project_ready: Option<ProjectReadyObserver>,
    creation_intent: Option<crate::project::creation::ProjectCreationIntent>,
    invocation_access: Option<super::access::InvocationAccess>,
    artifact_recorder: super::artifacts::ArtifactRecorder,
    inputs: Option<super::inputs::GenerationInputs>,
    camera_grouping_policy_version: std::sync::RwLock<Option<u32>>,
    ordinary_image_preparation_version: std::sync::RwLock<Option<u32>>,
    generation_policy: std::sync::RwLock<Option<super::generation_policy::GenerationPolicy>>,
    reshoot_options: Option<crate::pipeline::reshoot::ReshootOptions>,
    image_matching_strategy: crate::project::ImageMatchingStrategy,
    video_projection: crate::video::panorama::VideoProjection,
    lifecycle: Option<Arc<dyn Fn(RunnerUpdate) + Send + Sync>>,
    input_boundary: Option<PathBuf>,
    engines: EnginePaths,
    process_manager: ProcessManager,
    events: EventSink,
    diagnostic_logs: Arc<std::sync::Mutex<crate::diagnostics::logs::ExecutionLogs>>,
    active_project: Arc<std::sync::Mutex<Option<ActiveProjectContext>>>,
    preparation_context: std::sync::Mutex<Option<PreparationContext>>,
    current_acceleration: Arc<std::sync::Mutex<Option<crate::engines::ColmapAccelerationStatus>>>,
    automatic_retry_this_run: AtomicBool,
    elapsed_offset_ms: AtomicU64,
    workspace_task_id: Option<uuid::Uuid>,
    execution_run_id: Option<uuid::Uuid>,
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

fn image_matching_policy(
    input: ProjectInputType,
    strategy: crate::project::ImageMatchingStrategy,
    quality: Quality,
    planner_enabled: bool,
) -> (bool, bool) {
    let sequential = input == ProjectInputType::Video
        || strategy == crate::project::ImageMatchingStrategy::Sequential;
    (
        sequential,
        sequential && quality.loop_detection_enabled(planner_enabled),
    )
}

impl PipelineRunner {
    async fn record_collection_artifacts(
        &self,
        paths: &ProjectPaths,
        state: &mut PipelineStateFile,
    ) -> Result<()> {
        let activity = self.observed_activity("正在保存生成检查点");
        let result = self
            .artifact_recorder
            .record(
                paths,
                state,
                self.execution_run_id,
                &self.process_manager.child_token(),
            )
            .await;
        if result.is_ok() {
            activity.finish();
        }
        result.map_err(|error| {
            error.with_io_diagnostic(
                "artifact_operation=validate_and_capture\npath_role=work_artifacts",
            )
        })
    }

    pub fn with_inputs(mut self, inputs: Option<super::inputs::GenerationInputs>) -> Self {
        self.inputs = inputs;
        self
    }
    pub fn new(engines: EnginePaths, emit: impl Fn(PipelineEvent) + Send + Sync + 'static) -> Self {
        Self::new_with_planner(engines, true, emit).with_generation_policy(Some(Default::default()))
    }

    pub fn new_with_planner(
        engines: EnginePaths,
        planner_enabled: bool,
        emit: impl Fn(PipelineEvent) + Send + Sync + 'static,
    ) -> Self {
        Self {
            invocation_access: None,
            project_ready: None,
            creation_intent: None,
            artifact_recorder: Default::default(),
            reshoot_options: None,
            inputs: None,
            camera_grouping_policy_version: std::sync::RwLock::new(Some(
                super::inputs::CAMERA_GROUPING_POLICY_VERSION,
            )),
            ordinary_image_preparation_version: std::sync::RwLock::new(Some(
                super::inputs::ORDINARY_IMAGE_PREPARATION_VERSION,
            )),
            generation_policy: std::sync::RwLock::new(None),
            video_projection: crate::video::panorama::VideoProjection::Auto,
            image_matching_strategy: crate::project::ImageMatchingStrategy::default(),
            engines,
            lifecycle: None,
            input_boundary: None,
            process_manager: ProcessManager::new(),
            events: EventSink {
                context: Default::default(),
                bound_operation: None,
                panorama_phase: Arc::new(AtomicU64::new(0)),
                panorama_images: Arc::new(AtomicBool::new(false)),
                emit: Arc::new(emit),
                sequence: Arc::new(AtomicU64::new(0)),
                last_progress_milli_percent: Arc::new(AtomicU64::new(0)),
                last_stage: Arc::new(std::sync::Mutex::new(None)),
                dispatch: Arc::new(std::sync::Mutex::new(())),
                started: Instant::now(),
            },
            active_project: Arc::new(std::sync::Mutex::new(None)),
            diagnostic_logs: Default::default(),
            preparation_context: std::sync::Mutex::new(None),
            current_acceleration: Arc::new(std::sync::Mutex::new(None)),
            automatic_retry_this_run: AtomicBool::new(false),
            elapsed_offset_ms: AtomicU64::new(0),
            workspace_task_id: None,
            execution_run_id: None,
            planner_enabled,
            effectiveness: None,
        }
    }

    pub fn with_invocation_access(
        mut self,
        access: Option<super::access::InvocationAccess>,
    ) -> Self {
        self.invocation_access = access;
        self
    }

    fn invocation_input_access(&self) -> Option<super::access::InvocationAccess> {
        self.invocation_access.as_ref().map(|access| {
            let project = self
                .active_project
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            match project.as_ref() {
                Some(project) => access.clone().with_owned_project(&project.project_path),
                None => access.clone(),
            }
        })
    }

    fn authorize_external_path(&self, path: &Path) -> Result<()> {
        if let Some(access) = self.invocation_input_access() {
            access.authorize(path)?;
        }
        Ok(())
    }

    fn authorize_external_images(&self, path: &Path) -> Result<()> {
        if let Some(access) = self.invocation_input_access() {
            access.authorize_image_directory(path)?;
        }
        Ok(())
    }

    fn authorize_external_inputs(&self, inputs: &super::inputs::GenerationInputs) -> Result<()> {
        if let Some(access) = self.invocation_input_access() {
            access.authorize_inputs(inputs)?;
        }
        Ok(())
    }

    async fn authorize_reshoot_source(&self, id: uuid::Uuid) -> Result<()> {
        if self.invocation_access.is_none() {
            return Ok(());
        }
        let (project, metadata) = catalog::load_registered_project(id).await?;
        // Completion validation can inspect the registered published Gaussian
        // even when it will not be used as a warm seed. A referenced external
        // model is not an implicit permission grant either.
        if let Some(model) = catalog::project_model_path(&project, &metadata) {
            let owned = std::fs::canonicalize(&project)
                .ok()
                .zip(std::fs::canonicalize(&model).ok())
                .is_some_and(|(root, file)| file.starts_with(root));
            if !owned {
                self.authorize_external_path(&model)?;
            }
        }
        if let Some(colmap) = metadata.imported.as_ref().and_then(|v| v.colmap.as_ref()) {
            if let Some(access) = &self.invocation_access {
                access.authorize_model(&colmap.sparse_model_path)?;
            }
            if let Some(database) = &colmap.database_path {
                self.authorize_external_path(database)?;
            }
            if let Some(images) = &colmap.images_path {
                self.authorize_external_images(images)?;
            }
        }
        Ok(())
    }

    pub fn with_camera_grouping_policy_version(mut self, version: Option<u32>) -> Self {
        *self
            .camera_grouping_policy_version
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = version;
        self
    }

    fn camera_grouping_policy_version(&self) -> Option<u32> {
        *self
            .camera_grouping_policy_version
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn with_ordinary_image_preparation_version(mut self, version: Option<u32>) -> Self {
        *self
            .ordinary_image_preparation_version
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = version;
        self
    }

    fn ordinary_image_preparation_version(&self) -> Option<u32> {
        *self
            .ordinary_image_preparation_version
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn with_generation_policy(
        mut self,
        policy: Option<super::generation_policy::GenerationPolicy>,
    ) -> Self {
        self.planner_enabled = policy.is_some() || self.planner_enabled;
        *self
            .generation_policy
            .get_mut()
            .unwrap_or_else(|p| p.into_inner()) = policy;
        self
    }

    fn policy(&self) -> Option<super::generation_policy::GenerationPolicy> {
        *self
            .generation_policy
            .read()
            .unwrap_or_else(|p| p.into_inner())
    }

    pub fn with_failed_retry_policy(
        self,
        _policy: Option<Option<super::generation_policy::GenerationPolicy>>,
    ) -> Self {
        // Old callers may still supply this flag. Saved work is authoritative;
        // a stop reason can no longer reset a recipe or grant another retry.
        self
    }

    fn resolution_plan(
        &self,
        quality: Quality,
        memory: Option<u64>,
        width: u32,
        height: u32,
        video: bool,
    ) -> crate::presets::PlannerResolutionPlan {
        self.policy().map_or_else(
            || resolve_planner_resolution_plan(quality, memory, width, height, video),
            |p| p.resolution(quality, memory, width, height, video),
        )
    }

    pub fn with_input_boundary(mut self, path: Option<PathBuf>) -> Self {
        self.input_boundary = path;
        self
    }

    pub fn with_reshoot_options(
        mut self,
        options: Option<crate::pipeline::reshoot::ReshootOptions>,
    ) -> Self {
        self.reshoot_options = options;
        self
    }

    pub fn with_video_projection(
        mut self,
        projection: crate::video::panorama::VideoProjection,
    ) -> Self {
        self.video_projection = projection;
        self
    }

    pub fn with_image_matching_strategy(
        mut self,
        strategy: crate::project::ImageMatchingStrategy,
    ) -> Self {
        self.image_matching_strategy = strategy;
        self
    }

    pub fn with_lifecycle_observer(
        mut self,
        observer: impl Fn(RunnerUpdate) + Send + Sync + 'static,
    ) -> Self {
        let observer: Arc<dyn Fn(RunnerUpdate) + Send + Sync> = Arc::new(observer);
        let observe = observer.clone();
        self.process_manager =
            self.process_manager
                .with_lifecycle(Arc::new(move |update| match update {
                    ProcessUpdate::LogOpened { path, offset } => {
                        observe(RunnerUpdate::LogOpened { path, offset })
                    }
                    ProcessUpdate::Exited {
                        executable,
                        exit_code,
                    } => {
                        let name = executable.file_stem().unwrap_or_default().to_string_lossy();
                        let engine = if name.contains("brush") {
                            PipelineEngine::Brush
                        } else if name.contains("colmap") {
                            PipelineEngine::Colmap
                        } else {
                            PipelineEngine::Ffmpeg
                        };
                        observe(RunnerUpdate::ProcessExited { engine, exit_code });
                    }
                    _ => {}
                }));
        self.lifecycle = Some(observer);
        self
    }

    pub fn with_effectiveness_tracker(mut self, tracker: PlannerEffectivenessTracker) -> Self {
        self.effectiveness = Some(tracker);
        self
    }

    pub(crate) fn record_frame_metrics(
        &self,
        state: &PipelineStateFile,
        selected_count: u64,
        plan: Option<&FramePlan>,
    ) {
        let Some(tracker) = &self.effectiveness else {
            return;
        };
        let (source_width, source_height, source_item_count) =
            match (state.video.as_ref(), state.image_sequence.as_ref()) {
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
                _ => return,
            };
        let frames = state.frames.as_ref();
        let is_reshoot = state.reshoot.is_some();
        let is_planned_video = state.planner_enabled && state.video.is_some();
        let initial_selected_count = if is_reshoot {
            selected_count
        } else {
            frames
                .map(|frames| frames.initial_extracted_frames)
                .filter(|count| *count > 0)
                .unwrap_or(selected_count)
        };
        let has_candidates = is_planned_video && !is_reshoot;
        let source_candidates = plan
            .and_then(|plan| plan.bridge_source_frames)
            .or_else(|| frames.and_then(|frames| frames.bridge_source_frames));
        let candidate_count = if has_candidates {
            source_candidates
                .or_else(|| plan.map(FramePlan::candidate_count))
                .or_else(|| frames.map(|frames| frames.candidate_frames.len() as u64))
                .filter(|count| *count > 0)
                .unwrap_or(initial_selected_count)
        } else {
            initial_selected_count
        };
        let preset = state.preset.preset();
        let effective_fps = |count| {
            state
                .video
                .as_ref()
                .map(|video| count as f64 / video.duration.max(0.001))
        };
        tracker.record_frame_plan(PlannerFramePlanMetrics {
            source_width,
            source_height,
            source_item_count,
            configured_target_fps: is_planned_video.then_some(preset.initial_fps).flatten(),
            configured_candidate_fps: (has_candidates && source_candidates.is_none())
                .then_some(preset.rescue_max_fps)
                .flatten(),
            effective_target_fps: effective_fps(initial_selected_count),
            effective_candidate_fps: (!is_reshoot)
                .then(|| effective_fps(candidate_count))
                .flatten(),
            initial_selected_count,
            candidate_count,
            minimum_frame_override_applied: is_planned_video
                && if is_reshoot {
                    state.video.as_ref().is_some_and(|video| {
                        preset.initial_fps.is_some_and(|fps| {
                            crate::video::minimum_selected_frames() as f64
                                / video.duration.max(0.001)
                                > fps
                        })
                    })
                } else {
                    plan.map(|plan| plan.minimum_frame_override_applied)
                        .or_else(|| frames.map(|frames| frames.minimum_frame_override_applied))
                        .unwrap_or(false)
                },
            minimum_frame_target_unreachable: is_planned_video
                && source_item_count < crate::video::minimum_selected_frames(),
        });
    }

    pub(super) fn record_input_plan(
        &self,
        quality: Quality,
        video: &VideoInfo,
        plan: &FramePlan,
        is_reshoot: bool,
    ) {
        let mut state = PipelineStateFile::created(quality);
        state.planner_enabled = self.planner_enabled;
        state.video = Some(video.clone());
        state.frames = Some(FrameState::from(plan));
        if is_reshoot {
            state.reshoot = Some(Default::default());
        }
        self.record_frame_metrics(&state, plan.estimated_frames, Some(plan));
    }

    fn record_material_frame_plan(
        &self,
        quality: Quality,
        video: &VideoInfo,
        plan: &FramePlan,
        is_reshoot: bool,
        removal_fps: Option<f64>,
    ) {
        if let Some(policy) = self.policy() {
            if let (Some(tracker), Some(metrics)) = (
                &self.effectiveness,
                PlannerFramePlanMetrics::video(video, plan, quality, Some(policy), removal_fps),
            ) {
                tracker.record_frame_plan(metrics);
            }
        } else {
            self.record_input_plan(quality, video, plan, is_reshoot);
        }
    }

    /// Restore known source facts before a resumed run performs fallible cache checks.
    /// A reshoot's frame checkpoint contains old + new images, not its source count.
    fn restore_source_metrics(&self, state: &PipelineStateFile) {
        if let Some(checkpoint) = &state.inputs {
            self.restore_collection_metrics(checkpoint, state.preset, state.generation_policy);
            return;
        }
        let selected = if let Some(reshoot) = &state.reshoot {
            reshoot
                .supplemental_frames_complete
                .then_some(reshoot.reshoot_frame_count)
        } else {
            state
                .frames
                .as_ref()
                .map(|frames| frames.extracted_frames.unwrap_or(frames.estimated_frames))
        };
        if let Some(selected) = selected.filter(|count| *count > 0) {
            self.record_frame_metrics(state, selected, None);
        } else if let Some(images) = &state.image_sequence {
            if let Some(tracker) = &self.effectiveness {
                tracker.record_frame_plan(PlannerFramePlanMetrics::images(
                    images.width,
                    images.height,
                    images.image_count,
                ));
            }
        } else if let Some(video) = &state.video {
            let plan = if let Some(policy) = state.generation_policy {
                policy.frame_plan(video, state.preset)
            } else if state.planner_enabled {
                QualityV2FrameSelection.create_plan(video, &state.preset.preset())
            } else {
                UniformRatioFrameSelection.create_plan(video, &state.preset.preset())
            };
            self.record_frame_metrics(state, plan.estimated_frames, Some(&plan));
        }
    }

    pub(super) fn restore_collection_metrics(
        &self,
        checkpoint: &crate::pipeline::inputs::InputCheckpoint,
        quality: Quality,
        policy: Option<crate::pipeline::generation_policy::GenerationPolicy>,
    ) {
        let Some(tracker) = &self.effectiveness else {
            return;
        };
        let requested = match &checkpoint.request {
            crate::pipeline::inputs::GenerationInputs::Materials { items } => {
                match items.as_slice() {
                    [item] => Some(item),
                    _ => None,
                }
            }
            _ => None,
        };
        let saved_frame = checkpoint
            .telemetry
            .as_ref()
            .filter(|saved| {
                requested.is_some_and(|item| {
                    saved.source_id.as_deref() == Some(item.id.as_str())
                        && item.identity.is_some()
                        && saved.source_identity.as_deref() == item.identity.as_deref()
                })
            })
            .and_then(|saved| saved.frame_plan.clone());
        let frame = saved_frame.or_else(|| {
            let source = checkpoint.single_requested_source()?;
            if let (Some(video), Some(frames)) = (&source.video, &source.frames) {
                let plan = FramePlan {
                    retention_ratio: frames.retention_ratio,
                    sampling_fps: frames.sampling_fps,
                    estimated_frames: if frames.initial_extracted_frames > 0 {
                        frames.initial_extracted_frames
                    } else {
                        frames.estimated_frames
                    },
                    // This is a scalar observation, not an execution plan.
                    // Keep availability without copying potentially millions of indices.
                    selected_frames: Vec::new(),
                    candidate_frames: Vec::new(),
                    bridge_source_frames: Some(
                        frames
                            .bridge_source_frames
                            .unwrap_or(frames.candidate_frames.len() as u64),
                    ),
                    rescue_max_frames: frames.rescue_max_frames,
                    minimum_frame_override_applied: frames.minimum_frame_override_applied,
                };
                PlannerFramePlanMetrics::video(
                    video,
                    &plan,
                    quality,
                    policy,
                    requested.and_then(|item| item.frame_removal_fps),
                )
            } else if source.dimensions.len() == 1 {
                source.image_sequence.as_ref().map(|images| {
                    PlannerFramePlanMetrics::images(images.width, images.height, images.image_count)
                })
            } else {
                None
            }
        });
        if let Some(frame) = frame {
            tracker.record_frame_plan(frame);
        }
        if let Some(saved) = checkpoint.telemetry.as_ref() {
            if let Some(vendor) = saved.gpu_vendor {
                tracker.record_gpu_vendor(vendor);
            }
            if let Some(initial) = &saved.initial_reconstruction {
                tracker.record_initial_reconstruction(initial.clone());
            }
            if let Some(bridge) = &saved.bridge {
                tracker.record_bridge(bridge.clone());
            }
        }
    }

    pub(super) fn observe_collection_source(
        &self,
        checkpoint: &mut crate::pipeline::inputs::InputCheckpoint,
        item: &crate::pipeline::inputs::MaterialInput,
        analysis: &crate::pipeline::inputs::MaterialAnalysis,
        quality: Quality,
        policy: crate::pipeline::generation_policy::GenerationPolicy,
    ) {
        let frame = match (&analysis.video, &analysis.image_sequence, &analysis.plan) {
            (Some(video), _, Some(plan)) => PlannerFramePlanMetrics::video(
                video,
                plan,
                quality,
                Some(policy),
                item.frame_removal_fps,
            ),
            (_, Some(images), _) if analysis.dimensions.len() == 1 => Some(
                PlannerFramePlanMetrics::images(images.width, images.height, images.image_count),
            ),
            _ => None,
        };
        if let Some(frame) = frame {
            self.observe_collection_frame(checkpoint, item, &analysis.identity, frame);
        }
    }

    pub(super) fn observe_collection_frame(
        &self,
        checkpoint: &mut crate::pipeline::inputs::InputCheckpoint,
        item: &crate::pipeline::inputs::MaterialInput,
        identity: &str,
        frame: PlannerFramePlanMetrics,
    ) {
        let crate::pipeline::inputs::GenerationInputs::Materials { items } = &checkpoint.request
        else {
            return;
        };
        if items.len() != 1 || items[0].id != item.id || item.identity.as_deref() != Some(identity)
        {
            return;
        }
        let saved = checkpoint.telemetry.get_or_insert_with(Default::default);
        saved.source_id = Some(item.id.clone());
        saved.source_identity = Some(identity.to_owned());
        saved.frame_plan = Some(frame.clone());
        if let Some(tracker) = &self.effectiveness {
            tracker.record_frame_plan(frame);
        }
    }

    pub(crate) fn record_reconstruction_metrics(
        &self,
        report: &ReconstructionReport,
        acceleration: &crate::engines::ColmapAccelerationStatus,
        allow_two_view_tracks: bool,
        is_reshoot: bool,
    ) {
        let Some(tracker) = &self.effectiveness else {
            return;
        };
        tracker.record_gpu_vendor(if acceleration.detected_nvidia_device_count > 0 {
            PlannerGpuVendor::Nvidia
        } else if cfg!(target_os = "macos") {
            PlannerGpuVendor::Apple
        } else {
            PlannerGpuVendor::Unknown
        });
        tracker.record_initial_reconstruction(PlannerInitialReconstructionMetrics {
            input_images: report.input_images,
            registered_images: report.registered_images,
            points_3d: report.points_3d,
            backend: acceleration.backend,
            allow_two_view_tracks,
        });
        if is_reshoot {
            tracker.record_bridge(PlannerBridgeMetrics {
                status: PlannerBridgeStatus::NotApplicable,
                trigger_ratio: bridge_trigger_ratio(),
                initial_input_images: report.input_images,
                initial_registered_images: report.registered_images,
                initial_points_3d: report.points_3d,
                final_input_images: report.input_images,
                final_registered_images: report.registered_images,
                final_points_3d: report.points_3d,
                ..Default::default()
            });
        }
    }

    pub(crate) fn record_brush_metrics(&self, state: &PipelineStateFile) {
        if let Some(tracker) = &self.effectiveness {
            tracker.restore_brush(
                state.brush_training.initial_resolved,
                state.brush_training.resolved,
                state.brush_training.oom_retry_used,
            );
        }
    }

    pub(super) fn prepare_brush_observation(&self, state: &PipelineStateFile) {
        if let Some(observer) = &self.lifecycle {
            observer(RunnerUpdate::BrushPreparation);
        }
        if let Some(tracker) = &self.effectiveness {
            tracker.prepare_brush(
                state.brush_training.initial_resolved,
                state.brush_training.resolved,
                state.brush_training.oom_retry_used,
            );
        }
    }

    pub(super) fn mark_automatic_retry(&self) {
        self.automatic_retry_this_run.store(true, Ordering::Relaxed);
        if let Some(observer) = &self.lifecycle {
            observer(RunnerUpdate::AutomaticRetry);
        }
    }

    #[cfg(test)]
    pub(super) fn finalize_manual_retry_markers(&self, _state: &mut PipelineStateFile) -> bool {
        // Compatibility adapter: continuing never erases a previous remedy.
        false
    }

    pub(crate) fn record_final_metrics(&self, output: &ProjectOutput) {
        if let Some(tracker) = &self.effectiveness {
            tracker.record_final_result(PlannerFinalResultMetrics {
                input_images: output.input_images,
                registered_images: output.registered_images,
                points_3d: output.points_3d,
                splat_count: output.splat_count,
                ply_size_bytes: output.file_size,
            });
        }
    }

    pub fn with_workspace_task_id(mut self, workspace_task_id: Option<uuid::Uuid>) -> Self {
        self.workspace_task_id = workspace_task_id;
        self
    }

    pub fn with_creation_intent(
        mut self,
        intent: Option<crate::project::creation::ProjectCreationIntent>,
    ) -> Self {
        if let Some(intent) = &intent {
            *self
                .camera_grouping_policy_version
                .get_mut()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                intent.camera_grouping_policy_version;
            *self
                .ordinary_image_preparation_version
                .get_mut()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                intent.ordinary_image_preparation_version;
        }
        self.creation_intent = intent;
        self
    }

    pub fn with_project_ready_observer(
        mut self,
        observer: impl Fn(
                ProjectMetadata,
            )
                -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        self.project_ready = Some(Arc::new(observer));
        self
    }

    pub fn with_execution_run_id(mut self, execution_run_id: Option<uuid::Uuid>) -> Self {
        self.execution_run_id = execution_run_id;
        self
    }

    pub fn current_project_identity(&self) -> Option<(uuid::Uuid, Option<uuid::Uuid>)> {
        self.active_project
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|project| (project.project_id, project.workspace_task_id))
    }

    fn bind_project_context(&self, paths: &ProjectPaths, metadata: &ProjectMetadata) {
        self.retry_preparation_context(paths);
        *self
            .active_project
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(ActiveProjectContext {
            project_id: paths.id,
            workspace_task_id: metadata.workspace_task_id,
            project_path: paths.project.clone(),
            logs_directory: paths.logs.clone(),
        });
        if let Some(observer) = &self.lifecycle {
            observer(RunnerUpdate::Project(Box::new(metadata.clone())));
        }
    }

    fn retry_preparation_context(&self, paths: &ProjectPaths) -> PreparationContext {
        let mut active = self
            .preparation_context
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if active.as_ref().is_none_or(|context| {
            let context = context.snapshot();
            context.project_id != Some(paths.id)
                || context.project_root.as_ref() != Some(&paths.project)
        }) {
            *active = Some(PreparationContext::new(
                Some(paths.id),
                Some(paths.project.clone()),
            ));
        }
        active.as_ref().expect("bound preparation context").clone()
    }

    pub(crate) fn preparation_diagnostic_context(&self) -> Option<PreparationDiagnosticContext> {
        self.preparation_context
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(PreparationContext::snapshot)
    }

    async fn retry_preparation_step<T, F>(
        &self,
        paths: &ProjectPaths,
        operation: RetryPreparationOperation,
        path_role: &'static str,
        cache: Option<&str>,
        future: F,
    ) -> Result<T>
    where
        F: std::future::Future<Output = Result<T>>,
    {
        let context = self.retry_preparation_context(paths);
        let result = context.step(operation, path_role, cache, future).await;
        if let (Some(observer), Some((path, offset))) = (&self.lifecycle, context.registered_log())
        {
            observer(RunnerUpdate::LogOpened { path, offset });
        }
        result
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.events.started.elapsed().as_millis() as u64
    }

    pub fn elapsed_offset_ms(&self) -> u64 {
        self.elapsed_offset_ms.load(Ordering::Relaxed)
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

    /// Only the failed operation's proven byte range may become a report.
    /// Unobserved/legacy processes have no evidence and therefore no log tail.
    pub(crate) fn freeze_failure_log(
        &self,
        error: &SplatError,
        engine: Option<PipelineEngine>,
    ) -> Option<crate::diagnostics::logs::FrozenLog> {
        let expected_termination = match error {
            SplatError::EngineProcess(failure) if Some(failure.engine) == engine => {
                Some(&failure.termination)
            }
            // FFmpeg's legacy adapters still return these types. A failed
            // native status in this exact operation is required below; a
            // successful extraction followed by mapping validation is not it.
            SplatError::Process(_) | SplatError::InvalidVideo(_)
                if engine == Some(PipelineEngine::Ffmpeg) =>
            {
                None
            }
            _ => return None,
        };
        let operation = self
            .events
            .context
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .current
            .clone()?;
        let log = self
            .diagnostic_logs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .for_failure(self.execution_run_id?, &operation, engine?)?;
        if expected_termination.is_some_and(|status| !log.termination_matches(status)) {
            return None;
        }
        log.freeze()
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
        colmap_output: &Path,
        masks: &Path,
        logs: Option<&Path>,
        planner_enabled: bool,
        probed_video: Option<VideoInfo>,
        target_dimensions: Option<(u32, u32)>,
        panorama: bool,
        frame_removal_fps: Option<f64>,
    ) -> Result<PreparedFrames> {
        self.authorize_external_path(input)?;
        self.events
            .stage(PipelineStage::ProbingVideo, 0.0, "正在读取视频信息");
        let mut video = match probed_video {
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
        let mut plan = if let Some(policy) = self.policy() {
            policy.frame_plan_for_material(&video, quality, frame_removal_fps)?
        } else if planner_enabled {
            QualityV2FrameSelection.create_plan(&video, &quality.preset())
        } else {
            UniformRatioFrameSelection.create_plan(&video, &quality.preset())
        };
        self.record_material_frame_plan(
            quality,
            &video,
            &plan,
            self.reshoot_options.is_some(),
            frame_removal_fps,
        );
        // Exact frame timing is shared by ordinary and ERP videos. It does not
        // imply a projection change; only `panorama` selects the ERP path.
        if panorama || self.policy().is_some_and(|p| p.version >= 4) {
            target_dimensions.ok_or_else(|| SplatError::Process("抽帧缺少目标尺寸".into()))?;
            self.events
                .panorama_phase
                .store(if panorama { 4 } else { 8 }, Ordering::Relaxed);
            self.events.send(
                PipelineStage::ExtractingFrames,
                Some(PipelineEngine::Ffmpeg),
                EventKind::Stage,
                EventLevel::Info,
                None,
                true,
                "正在读取视频帧时间信息",
                None,
                None,
                None,
            );
            let times = crate::video::panorama::actual_frame_plan(
                &self.engines.ffprobe,
                input,
                &mut plan,
                &self.process_manager,
            )
            .await?;
            self.events
                .panorama_phase
                .store(u64::from(panorama), Ordering::Relaxed);
            if let Some(total) = plan.bridge_source_frames {
                video.total_frames = total;
            }
            // The real PTS table can refine counts and the effective sampling
            // plan. Observe the final plan, not only its earlier estimate.
            self.record_material_frame_plan(
                quality,
                &video,
                &plan,
                self.reshoot_options.is_some(),
                frame_removal_fps,
            );
            self.events.stage(
                PipelineStage::ExtractingFrames,
                0.0,
                if panorama {
                    "正在提取全景画面"
                } else {
                    "FFmpeg 开始提取画面"
                },
            );
            tokio::fs::write(
                output
                    .parent()
                    .unwrap_or(output)
                    .join("panorama-frame-times.json"),
                serde_json::to_vec_pretty(&times)?,
            )
            .await?;
        }
        self.events.stage(
            PipelineStage::PlanningFrames,
            1.0,
            format!(
                "预计提取 {} 帧（目标 {:.2} FPS，候选池 {} 帧 / 最高约 {:.2} FPS）",
                plan.estimated_frames,
                plan.sampling_fps,
                plan.candidate_count(),
                plan.candidate_count() as f64 / video.duration.max(f64::EPSILON)
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
        let extraction = if planner_enabled || panorama {
            extract_selected_frames(
                &self.engines.ffmpeg,
                input,
                output,
                colmap_output,
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
                colmap_output,
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
        colmap_output: &Path,
        masks: &Path,
        probe_already_complete: bool,
    ) -> Result<PreparedFrames> {
        self.authorize_external_images(input)?;
        if !probe_already_complete {
            self.events
                .stage(PipelineStage::ProbingVideo, 0.0, "正在快速读取图片头");
        }
        let source = input.to_path_buf();
        let cancellation = self.process_manager.child_token();
        let scan = crate::presets::spawn_pipeline_blocking(move || {
            scan_image_sequence(&source, Some(&cancellation))
        })
        .await
        .map_err(|error| SplatError::Process(format!("图片序列分析任务失败：{error}")))??;
        let image_sequence = scan.info.clone();
        if let Some(tracker) = &self.effectiveness {
            tracker.record_frame_plan(PlannerFramePlanMetrics::images(
                image_sequence.width,
                image_sequence.height,
                image_sequence.image_count,
            ));
        }
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
        let colmap_frames = colmap_output.to_path_buf();
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
        let ordinary_preparation = self.ordinary_image_preparation_version();
        let prepared = crate::presets::spawn_pipeline_blocking(move || {
            prepare_scanned_image_sequence_with_preparation(
                scan,
                &frames,
                &colmap_frames,
                &mask_root,
                ImageSequenceNaming::Primary,
                Some(observer),
                Some(&cancellation),
                ordinary_preparation,
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

    async fn prepare_panorama_images(
        &self,
        input: &Path,
        quality: Quality,
        config: &crate::video::panorama::PanoramaCheckpoint,
        frames: &Path,
        colmap: &Path,
        masks: &Path,
    ) -> Result<PreparedFrames> {
        let source = input.to_path_buf();
        let config = config.clone();
        let (frames, colmap, masks) = (
            frames.to_path_buf(),
            colmap.to_path_buf(),
            masks.to_path_buf(),
        );
        let cancellation = self.process_manager.child_token();
        let events = self.events.clone();
        let effectiveness = self.effectiveness.clone();
        crate::presets::spawn_pipeline_blocking(move || {
            let scan = crate::video::image_projection::analyze(&source)?;
            if let Some(tracker) = &effectiveness {
                tracker.record_frame_plan(PlannerFramePlanMetrics::images(
                    scan.info.width,
                    scan.info.height,
                    scan.info.image_count,
                ));
            }
            let plan = crate::video::create_image_plan(&scan.info, &quality.preset());
            events.stage(
                PipelineStage::PlanningFrames,
                1.0,
                format!("将处理全部 {} 张图片", scan.info.image_count),
            );
            events.stage(
                PipelineStage::ExtractingFrames,
                0.0,
                format!("正在准备全景图片 0/{}", scan.info.image_count),
            );
            let prepared = crate::video::image_projection::prepare(
                &source,
                &config,
                &frames,
                &colmap,
                &masks,
                &cancellation,
                |current, total| {
                    events.send(
                        PipelineStage::ExtractingFrames,
                        Some(PipelineEngine::System),
                        EventKind::Stage,
                        EventLevel::Info,
                        Some(current as f32 / total.max(1) as f32),
                        false,
                        format!("正在准备全景图片 {current}/{total}"),
                        Some(current),
                        Some(total),
                        Some("images"),
                    );
                },
            )?;
            Ok(PreparedFrames {
                input_type: ProjectInputType::Images,
                video: None,
                image_sequence: Some(scan.info),
                plan,
                extracted_frames: prepared.image_count,
                image_format: "images".into(),
                mask_count: prepared.mask_count,
                has_alpha: prepared.has_alpha,
                working_width: config.sfm_width,
                working_height: config.sfm_height,
            })
        })
        .await
        .map_err(|e| SplatError::Process(e.to_string()))?
    }

    pub async fn generate(
        &self,
        input: &Path,
        quality: Quality,
        projects_root: &Path,
    ) -> Result<PipelineResult> {
        Box::pin(self.generate_with_manager(
            input,
            quality,
            ProjectManager::with_root(projects_root.to_path_buf()),
        ))
        .await
    }

    pub async fn generate_for_diagnostics(
        &self,
        input: &Path,
        quality: Quality,
        projects_root: &Path,
    ) -> Result<PipelineResult> {
        Box::pin(self.generate_with_manager(
            input,
            quality,
            ProjectManager::for_diagnostics(projects_root.to_path_buf()),
        ))
        .await
    }

    async fn generate_with_manager(
        &self,
        input: &Path,
        quality: Quality,
        project_manager: ProjectManager,
    ) -> Result<PipelineResult> {
        let project_manager = project_manager.with_creation_intent(self.creation_intent.clone());
        if let Some(inputs) = &self.inputs {
            return Box::pin(self.generate_inputs(project_manager, inputs.clone(), quality, None))
                .await;
        }
        let project_manager = project_manager.with_input_boundary(self.input_boundary.clone());
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
        state.video_projection = self.video_projection;
        if state.input_type == ProjectInputType::Images {
            state.image_matching_strategy = self.image_matching_strategy;
            state.gps = Some(crate::reconstruction::gps::Checkpoint::pending());
        }
        state.configuration = Some((*crate::presets::pipeline_optimization_config()).clone());
        state.generation_policy = self.policy();
        state.planner_enabled = self.planner_enabled;
        state.resolution_policy_version = self
            .planner_enabled
            .then_some(PLANNER_RESOLUTION_POLICY_VERSION);
        project_manager.write_state(&paths.state, &state).await?;
        Box::pin(self.execute_project(project_manager, paths, &mut metadata, state, &acceleration))
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
            source_identity: String::new(),
            projection: crate::video::panorama::VideoProjection::Perspective,
            warm_available: false,
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
            imported: metadata.imported.is_some(),
        };
        if !catalog::project_is_durably_completed(&project, &metadata).await {
            info.reason = Some("只有已完成且结果完整的项目可以进行高清补拍".into());
            return Ok(info);
        }
        if let Some(imported) = &metadata.imported {
            let Some(colmap) = imported.colmap.as_ref() else {
                info.reason = Some("外部项目没有可复用的 COLMAP 数据".into());
                return Ok(info);
            };
            if !colmap.reshoot_eligible {
                info.reason = colmap
                    .reshoot_reason
                    .clone()
                    .or_else(|| Some("外部 COLMAP 数据不满足高清补拍条件".into()));
                return Ok(info);
            }
            let colmap = colmap.clone();
            let parsed = match crate::presets::spawn_pipeline_blocking(move || {
                crate::imports::revalidate_imported_colmap(&colmap)
            })
            .await
            .map_err(|error| SplatError::Process(format!("读取外部相机信息失败：{error}")))?
            {
                Ok(parsed) => parsed,
                Err(error) => {
                    info.reason = Some(error.to_string());
                    return Ok(info);
                }
            };
            info.camera_id = parsed.0.id;
            info.camera_model = parsed.0.model;
            info.width = parsed.0.width;
            info.height = parsed.0.height;
            info.source_image_count = parsed.1.len() as u64;
            info.eligible = true;
            return Ok(info);
        }
        let source_state: PipelineStateFile =
            serde_json::from_slice(&tokio::fs::read(project.join("state.json")).await?)?;
        info.planner_enabled = source_state.planner_enabled;
        if source_state.panorama.is_some() {
            info.reason = Some("全景项目暂不支持高清补拍。".into());
            return Ok(info);
        }
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
        let resolved = match resolve_reshoot_colmap_input(
            &project,
            &metadata,
            &source_state,
            &model,
            &database,
        )
        .await
        {
            Ok(value) => value,
            Err(error) => {
                info.reason = Some(format!(
                    "原项目的 COLMAP 专用图片不完整或与原始帧不一致，无法安全进行增量补拍：{error}"
                ));
                return Ok(info);
            }
        };
        let database_for_validation = database.clone();
        let validate_exact_camera = resolved.mode == ReshootColmapInputMode::CurrentV2;
        let parsed = crate::presets::spawn_pipeline_blocking(move || {
            let camera = read_single_camera(&model)?;
            let images = read_registered_images(&model)?;
            if validate_exact_camera {
                validate_database_camera(&database_for_validation, &camera)?;
            }
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
                    let images = crate::presets::spawn_pipeline_blocking(move || {
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
            video: None,
            source_identity: None,
            projection_detection: None,
            image_sequence: None,
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
        requested_quality: Quality,
    ) -> Result<PipelineResult> {
        if let Some(inputs) = &self.inputs {
            return Box::pin(self.generate_inputs(
                ProjectManager::with_root(projects_root.to_path_buf()),
                inputs.clone(),
                requested_quality,
                Some(source_project_id),
            ))
            .await;
        }
        if let Some(options) = &self.reshoot_options {
            return self
                .generate_reshoot_v2(
                    source_project_id,
                    reshoot_input,
                    input_type,
                    projects_root,
                    requested_quality,
                    options.clone(),
                )
                .await;
        }
        validate_legacy_v1_preparation_policy(self.ordinary_image_preparation_version())?;
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
        let source_state: PipelineStateFile = if source_metadata.imported.is_some() {
            let mut state =
                PipelineStateFile::created_for(requested_quality, ProjectInputType::Images);
            state.planner_enabled = self.planner_enabled;
            state
        } else {
            serde_json::from_slice(&tokio::fs::read(source_root.join("state.json")).await?)?
        };
        let quality = if source_metadata.imported.is_some() {
            requested_quality
        } else {
            source_metadata.quality
        };
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
                    quality,
                    Some(observer),
                    Some(self.process_manager.child_token()),
                )
                .await?
        } else {
            project_manager.create(reshoot_input, quality).await?
        };
        let stored_source = metadata.source_path.clone();
        metadata.name = reshoot_project_name(&source_metadata.name);
        metadata.workspace_task_id = self.workspace_task_id;
        metadata.transform = source_metadata.transform;
        metadata.editing = Default::default();
        metadata.reshoot = Some(ReshootProvenance {
            source_project_id,
            source_project_path: source_root.clone(),
            source_final_ply: catalog::project_model_path(&source_root, &source_metadata)
                .unwrap_or_else(|| source_root.join("final.ply")),
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
        if source_metadata.imported.is_some() {
            state.planner_enabled = self.planner_enabled;
        } else {
            inherit_reshoot_planner_settings(&source_state, &mut state);
        }
        state.reshoot = Some(ReshootState {
            source_image_count: source.source_image_count,
            reshoot_frame_count: input.estimated_frames,
            mask_count: input.mask_count,
            has_alpha: input.has_alpha,
            ..Default::default()
        });
        project_manager.write_state(&paths.state, &state).await?;
        Box::pin(self.execute_project(project_manager, paths, &mut metadata, state, &acceleration))
            .await
    }

    pub async fn resume(&self, project_id: uuid::Uuid) -> Result<PipelineResult> {
        // A caller may reuse a runner without a run UUID. Continuation is a
        // fresh validation boundary even then; never import an earlier call's
        // in-memory evidence instead of checking saved artifacts.
        self.artifact_recorder.invalidate();
        let context = PreparationContext::new(Some(project_id), None);
        *self
            .preparation_context
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(context.clone());
        let project = context
            .step(
                RetryPreparationOperation::ReadRetryProject,
                "project_index",
                None,
                catalog::registered_project_path(project_id),
            )
            .await?;
        let context = context.with_project_root(project.clone());
        *self
            .preparation_context
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(context);
        let paths = ProjectPaths::existing(project_id, project.clone());
        self.retry_preparation_context(&paths);
        // Bind the internally registered location even if project.json cannot be
        // read. No Project/stage event is emitted until metadata is available.
        *self
            .active_project
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(ActiveProjectContext {
            project_id,
            workspace_task_id: self.workspace_task_id,
            project_path: project.clone(),
            logs_directory: paths.logs.clone(),
        });
        let mut metadata = self
            .retry_preparation_step(
                &paths,
                RetryPreparationOperation::ReadRetryProject,
                "project_metadata",
                None,
                catalog::read_project_metadata_at(&project, project_id),
            )
            .await?;
        let manager = ProjectManager::with_root(
            project
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| project.clone()),
        );
        self.bind_project_context(&paths, &metadata);
        let state = self
            .retry_preparation_step(
                &paths,
                RetryPreparationOperation::ReadRetryState,
                "project_state",
                None,
                manager.read_state(&paths.state),
            )
            .await?;
        // Publication and existing artifacts are inspected before probing any
        // engine or requiring original source media that may no longer be needed.
        Box::pin(self.execute_work_order(manager, paths, &mut metadata, state, None)).await
    }

    async fn actual_transparency(
        &self,
        paths: &ProjectPaths,
        state: &mut PipelineStateFile,
        candidate: bool,
    ) -> Result<bool> {
        if let Some(value) = state.actual_transparency {
            return Ok(value);
        }
        if !candidate {
            state.actual_transparency = Some(false);
            return Ok(false);
        }
        let frames = paths.frames.clone();
        let cancel = self.process_manager.child_token();
        let value = crate::presets::spawn_pipeline_blocking(move || -> Result<bool> {
            for file in crate::video::list_images(&frames)? {
                if cancel.is_cancelled() {
                    return Err(SplatError::Cancelled);
                }
                let mut reader = image::ImageReader::open(&file)?.with_guessed_format()?;
                let mut limits = image::Limits::default();
                limits.max_alloc = Some(crate::video::panorama::MAX_DECODE_BYTES);
                reader.limits(limits);
                let decoder = reader
                    .into_decoder()
                    .map_err(|e| SplatError::Process(e.to_string()))?;
                use image::ImageDecoder;
                if decoder.color_type().has_alpha() {
                    let image = image::DynamicImage::from_decoder(decoder)
                        .map_err(|e| SplatError::Process(e.to_string()))?;
                    if crate::video::image_sequence::has_native_transparency(&image) {
                        return Ok(true);
                    }
                }
            }
            Ok(false)
        })
        .await
        .map_err(|e| SplatError::Process(e.to_string()))??;
        state.actual_transparency = Some(value);
        Ok(value)
    }

    #[cfg(test)]
    async fn prepare_failed_retry(
        &self,
        manager: &ProjectManager,
        paths: &ProjectPaths,
        state: &mut PipelineStateFile,
        _policy: Option<super::generation_policy::GenerationPolicy>,
    ) -> Result<()> {
        self.prepare_continuation(manager, paths, state).await
    }

    /// Stop reasons never reset configuration or allowances. Finish the same
    /// durable invalidation barrier for every entry into the shared executor.
    async fn prepare_continuation(
        &self,
        manager: &ProjectManager,
        paths: &ProjectPaths,
        state: &mut PipelineStateFile,
    ) -> Result<()> {
        if state.retry_cache_pending.is_empty() {
            return Ok(());
        }
        if state.database_recovery_pending.is_some() {
            self.validate_database_rebuild_base(paths, state).await?;
        }
        self.retry_preparation_step(
            paths,
            RetryPreparationOperation::WriteRetryReuseBarrier,
            "project_state",
            None,
            manager.write_state_for_preparation(&paths.state, state),
        )
        .await?;
        self.archive_retry_cache(manager, paths, state).await
    }

    async fn archive_retry_cache(
        &self,
        manager: &ProjectManager,
        paths: &ProjectPaths,
        state: &mut PipelineStateFile,
    ) -> Result<()> {
        if state.retry_cache_pending.is_empty() {
            return Ok(());
        }
        let archive = paths
            .work
            .join("cache-archive")
            .join(format!("generation-policy-{}", uuid::Uuid::new_v4()));
        self.retry_preparation_step(
            paths,
            RetryPreparationOperation::CreateRetryArchive,
            "cache_archive_directory",
            None,
            async {
                tokio::fs::create_dir_all(&archive)
                    .await
                    .map_err(SplatError::from)
            },
        )
        .await?;
        while let Some(name) = state.retry_cache_pending.last().cloned() {
            if ![
                "brush",
                "colmap",
                "frames",
                "colmap_frames",
                "masks",
                "panorama-training",
                "inputs",
                "joint-bridge",
                "undistorted-training",
                "ordinary-training-model",
                "sphere-training-model",
                "sphere-training-images",
                "gps",
            ]
            .contains(&name.as_str())
            {
                return Err(SplatError::Process("Invalid retry cache entry".into()));
            }
            let source = paths.work.join(&name);
            let cancel = self.process_manager.child_token();
            let exists = self
                .retry_preparation_step(
                    paths,
                    RetryPreparationOperation::ArchiveRetryCache,
                    "cache_entry",
                    Some(&name),
                    async {
                        retry_archive::inspect_entry(&source, "cache_entry", &cancel, |event| {
                            self.append_retry_archive_observation(paths, &name, event)
                        })
                        .await
                        .map(|metadata| metadata.is_some())
                    },
                )
                .await?;
            if exists {
                self.retry_preparation_step(
                    paths,
                    RetryPreparationOperation::ArchiveRetryCache,
                    "cache_entry",
                    Some(&name),
                    async {
                        retry_archive::rename_entry(
                            &source,
                            &archive.join(&name),
                            &cancel,
                            |event| self.append_retry_archive_observation(paths, &name, event),
                        )
                        .await
                        .map(|_| ())
                    },
                )
                .await?;
            }
            state.retry_cache_pending.pop();
            self.retry_preparation_step(
                paths,
                RetryPreparationOperation::WriteRetryArchiveProgress,
                "project_state",
                Some(&name),
                manager.write_state_for_preparation(&paths.state, state),
            )
            .await?;
        }
        Ok(())
    }

    async fn append_retry_archive_observation(
        &self,
        paths: &ProjectPaths,
        cache: &str,
        observation: retry_archive::ArchiveIoObservation,
    ) {
        // Deliberately omit paths, ACL owners and image/GPS data. Logging is
        // best effort and must not replace the original rename error.
        self.retry_preparation_context(paths)
            .record_archive_observation(cache, &observation)
            .await;
    }

    async fn record_reshoot_connection_model(
        &self,
        manager: &ProjectManager,
        paths: &ProjectPaths,
        state: &mut PipelineStateFile,
        model: &Path,
    ) -> Result<()> {
        if !crate::pipeline::reshoot_connection::required(state) {
            return Ok(());
        }
        let snapshot = Box::new(state.clone());
        let model = model.to_path_buf();
        let cancel = self.process_manager.child_token();
        let proof = crate::presets::spawn_pipeline_blocking(move || {
            crate::pipeline::reshoot_connection::validate_model(&snapshot, &model, &cancel)
        })
        .await
        .map_err(|error| SplatError::Process(error.to_string()))??;
        state
            .reshoot
            .as_mut()
            .ok_or_else(|| SplatError::Process(legacy_work_order::UNRESTORABLE_RECIPE.into()))?
            .connection_proof = proof;
        manager.write_state(&paths.state, state).await?;
        Ok(())
    }

    async fn ensure_reshoot_candidate_connection(
        &self,
        paths: &ProjectPaths,
        state: &mut PipelineStateFile,
        candidate_sha256: String,
    ) -> Result<()> {
        use crate::pipeline::reshoot_connection;
        if !reshoot_connection::required(state)
            || reshoot_connection::candidate_is_proven(state, &candidate_sha256)?
        {
            return Ok(());
        }
        // Old candidates have no bound proof. Only the derived project's
        // owned reconstruction may establish it; never reopen original media.
        let model = if state
            .gps
            .as_ref()
            .is_some_and(|gps| matches!(gps.status.as_str(), "complete" | "matching"))
        {
            let model = paths.colmap.join("gps-v1/model");
            let expected = state
                .gps
                .as_ref()
                .and_then(|gps| gps.output_identity.clone());
            let checked = model.clone();
            let identity = crate::presets::spawn_pipeline_blocking(move || {
                gps_runner::model_identity(&checked)
            })
            .await
            .map_err(|error| SplatError::Process(error.to_string()))??;
            if expected.as_deref() != Some(identity.as_str()) {
                return Err(SplatError::Process(
                    legacy_work_order::UNRESTORABLE_RECIPE.into(),
                ));
            }
            model
        } else if state.inputs.is_some() {
            paths.colmap.join("authoritative")
        } else {
            let full = state
                .reshoot
                .as_ref()
                .and_then(|reshoot| reshoot.recipe.as_ref())
                .is_some_and(|recipe| {
                    recipe.actual.reconstruction
                        == crate::pipeline::reshoot::ReconstructionMode::Full
                });
            let root = paths
                .colmap
                .join(if full { "sparse-full" } else { "sparse" });
            let count = state
                .frames
                .as_ref()
                .and_then(|frames| frames.extracted_frames)
                .ok_or_else(|| {
                    SplatError::Process(legacy_work_order::UNRESTORABLE_RECIPE.into())
                })?;
            best_sparse_model_with_input_count(&root, count)
                .await
                .map_err(|error| {
                    if error.io_error().is_some() {
                        error
                    } else {
                        SplatError::Process(legacy_work_order::UNRESTORABLE_RECIPE.into())
                    }
                })?
                .0
        };
        let snapshot = Box::new(state.clone());
        let cancel = self.process_manager.child_token();
        let proof = crate::presets::spawn_pipeline_blocking(move || {
            let mut proof = reshoot_connection::validate_model(&snapshot, &model, &cancel)?
                .ok_or_else(|| {
                    SplatError::Process(legacy_work_order::UNRESTORABLE_RECIPE.into())
                })?;
            reshoot_connection::bind_candidate(&mut proof, candidate_sha256);
            Ok::<_, SplatError>(proof)
        })
        .await
        .map_err(|error| SplatError::Process(error.to_string()))??;
        state
            .reshoot
            .as_mut()
            .ok_or_else(|| SplatError::Process(legacy_work_order::UNRESTORABLE_RECIPE.into()))?
            .connection_proof = Some(proof);
        Ok(())
    }

    /// One publication/terminal path for new, imported, reshoot and continued
    /// work. The candidate stays immutable until every durable view is saved.
    #[allow(clippy::too_many_arguments)]
    async fn publish_work_order(
        &self,
        manager: &ProjectManager,
        paths: &ProjectPaths,
        metadata: &mut ProjectMetadata,
        state: &mut PipelineStateFile,
        candidate: &Path,
        output: ProjectOutput,
        attempt: String,
        warning: Option<String>,
    ) -> Result<PipelineResult> {
        self.events.source(None);
        let activity =
            self.application_activity(PipelineStage::Exporting, "正在校验并发布 final.ply", None);
        // Freeze the facts before publication. A crash here must not send the
        // next continuation back through training-data preparation or Brush.
        let info = inspect_gaussian_ply(candidate)?;
        if info.splat_count != output.splat_count || info.file_size != output.file_size {
            return Err(SplatError::Process(
                "Validated candidate output has changed".into(),
            ));
        }
        if metadata.reshoot.is_some() && !crate::pipeline::reshoot_connection::required(state) {
            return Err(SplatError::Process(
                legacy_work_order::UNRESTORABLE_RECIPE.into(),
            ));
        }
        if crate::pipeline::reshoot_connection::required(state) {
            let file = candidate.to_path_buf();
            let hash = crate::presets::spawn_pipeline_blocking(move || {
                crate::video::panorama::fingerprint(&file)
            })
            .await
            .map_err(|error| SplatError::Process(error.to_string()))??;
            self.ensure_reshoot_candidate_connection(paths, state, hash)
                .await?;
        }
        state.validated_output = Some(output.clone());
        manager.write_state(&paths.state, state).await?;
        let publication = crate::pipeline::publication::publish_candidate(
            &paths.project,
            &paths
                .work
                .join(crate::pipeline::publication::TRANSACTION_FILE_NAME),
            candidate,
            &paths.project.join("final.ply"),
            output,
            crate::pipeline::publication::PublicationIdentity {
                run_id: self.execution_run_id,
                attempt_id: attempt,
                source_identity: state
                    .reshoot
                    .as_ref()
                    .and_then(|value| value.snapshot_v3_identity.clone()),
                configuration_identity: state
                    .generation_policy
                    .map(|policy| format!("generation-policy-v{}", policy.version)),
            },
            &self.process_manager.child_token(),
        )
        .await?;
        state.brush_complete = true;
        state.stage = PipelineStage::Completed;
        state.validated_output = Some(publication.output.clone());
        let completed_at = publication
            .transaction
            .committed_at
            .unwrap_or_else(Utc::now);
        let duration_ms = metadata.duration_ms.unwrap_or(0).saturating_add(
            metadata
                .started_at
                .map(|started| (completed_at - started).num_milliseconds().max(0) as u64)
                .unwrap_or(0),
        );
        metadata.status = ProjectStatus::Completed;
        metadata.completed_at = Some(completed_at);
        metadata.duration_ms = Some(duration_ms);
        metadata.failure_message = None;
        metadata.output = Some(publication.output.clone());
        manager.write_state(&paths.state, state).await?;
        manager.write_metadata(&paths.metadata, metadata).await?;
        crate::pipeline::publication::mark_project_views_reconciled(
            &paths.project,
            &paths
                .work
                .join(crate::pipeline::publication::TRANSACTION_FILE_NAME),
            publication.transaction.transaction_id,
        )
        .await?;
        self.record_final_metrics(&publication.output);
        drop(activity);
        self.events.stage(
            PipelineStage::Exporting,
            1.,
            format!("已发布 {} 个 Splat", publication.output.splat_count),
        );
        self.events
            .stage(PipelineStage::Completed, 1., "全部处理完成");
        let mut result =
            pipeline_result_from_publication(&paths.project, metadata, state, &publication);
        result.warning = warning.or(result.warning);
        Ok(result)
    }

    async fn reconcile_publish_transaction(
        &self,
        project_manager: &ProjectManager,
        paths: &ProjectPaths,
        metadata: &mut ProjectMetadata,
        state: &mut PipelineStateFile,
    ) -> Result<Option<PipelineResult>> {
        let reconciliation_started = Instant::now();
        let transaction_path = paths
            .work
            .join(crate::pipeline::publication::TRANSACTION_FILE_NAME);
        // Do not create a publishing lifecycle for tasks with no transaction.
        let _activity = if tokio::fs::try_exists(&transaction_path).await? {
            self.events.source(None);
            // A transaction may still prove that training must be rebuilt.
            // Its inspection must not jump the new run to 98% before that is known.
            Some(self.observed_activity("正在恢复结果发布"))
        } else {
            None
        };
        // Prepared/installed transactions have not committed yet. Validate
        // reshoot connectivity before reconciliation can install or commit;
        // committed historical output remains authoritative and unchanged.
        let mut was_pending = false;
        match tokio::fs::read(&transaction_path).await {
            Ok(bytes) => {
                let transaction: crate::pipeline::publication::PublicationTransaction =
                    serde_json::from_slice(&bytes)?;
                was_pending = matches!(
                    transaction.phase,
                    crate::pipeline::publication::PublicationPhase::Prepared
                        | crate::pipeline::publication::PublicationPhase::OutputInstalled
                );
                if was_pending
                    && (metadata.reshoot.is_some()
                        || crate::pipeline::reshoot_connection::required(state))
                    && crate::pipeline::publication::has_publication_payload(
                        &paths.project,
                        &transaction,
                    )
                    .await?
                {
                    if !crate::pipeline::reshoot_connection::required(state) {
                        return Err(SplatError::Process(
                            legacy_work_order::UNRESTORABLE_RECIPE.into(),
                        ));
                    }
                    self.ensure_reshoot_candidate_connection(
                        paths,
                        state,
                        transaction.candidate_sha256,
                    )
                    .await?;
                    project_manager.write_state(&paths.state, state).await?;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let Some(reconciled) = crate::pipeline::publication::reconcile_publication(
            &paths.project,
            &transaction_path,
            &self.process_manager.child_token(),
        )
        .await?
        else {
            return Ok(None);
        };

        if reconciled.disposition
            == crate::pipeline::publication::ReconcileDisposition::CandidateRebuildRequired
        {
            // Publication proved that neither the validated candidate nor the
            // installed output belongs to this attempt. Invalidate training
            // and publication only; the artifact normalizer can still retain
            // prepared media, features, matches and reconstruction.
            state.brush_complete = false;
            state.validated_output = None;
            state.stage = PipelineStage::TrainingSplats;
            project_manager.write_state(&paths.state, state).await?;
            let archived = transaction_path.with_file_name(format!(
                "publish-transaction.rebuild-required-{}.json",
                reconciled.publication.transaction.transaction_id
            ));
            crate::project::manager::atomic_replace_file(&transaction_path, &archived).await?;
            return Ok(None);
        }

        let publication = reconciled.publication;
        let output = publication.output.clone();
        state.brush_complete = true;
        state.validated_output = Some(output.clone());
        state.stage = PipelineStage::Completed;
        let completed_at = publication
            .transaction
            .committed_at
            .unwrap_or_else(Utc::now);
        metadata.status = ProjectStatus::Completed;
        metadata.completed_at = Some(completed_at);
        metadata.failure_message = None;
        metadata.output = Some(output.clone());
        if was_pending {
            metadata.duration_ms = Some(
                metadata
                    .duration_ms
                    .unwrap_or(0)
                    .saturating_add(reconciliation_started.elapsed().as_millis() as u64),
            );
        }

        // The committed transaction is authoritative. These writes are
        // idempotent views and are deliberately completed before Reconciled.
        project_manager.write_state(&paths.state, state).await?;
        project_manager
            .write_metadata(&paths.metadata, metadata)
            .await?;
        crate::pipeline::publication::mark_project_views_reconciled(
            &paths.project,
            &transaction_path,
            publication.transaction.transaction_id,
        )
        .await?;
        self.record_final_metrics(&output);
        self.events.stage(
            PipelineStage::Exporting,
            1.0,
            format!("已发布 {} 个 Splat", output.splat_count),
        );
        self.events
            .stage(PipelineStage::Completed, 1.0, "全部处理完成");
        Ok(Some(pipeline_result_from_publication(
            &paths.project,
            metadata,
            state,
            &publication,
        )))
    }

    async fn execute_project(
        &self,
        project_manager: ProjectManager,
        paths: ProjectPaths,
        metadata: &mut ProjectMetadata,
        mut state: PipelineStateFile,
        acceleration: &crate::engines::ColmapAccelerationStatus,
    ) -> Result<PipelineResult> {
        project_manager
            .finish_creation(&paths, metadata, &mut state)
            .await?;
        if let Some(ready) = &self.project_ready {
            ready(metadata.clone()).await?;
        }
        Box::pin(self.execute_work_order(
            project_manager,
            paths,
            metadata,
            state,
            Some(acceleration),
        ))
        .await
    }

    async fn execute_work_order(
        &self,
        project_manager: ProjectManager,
        paths: ProjectPaths,
        metadata: &mut ProjectMetadata,
        state: PipelineStateFile,
        acceleration: Option<&crate::engines::ColmapAccelerationStatus>,
    ) -> Result<PipelineResult> {
        // GUI already installs this scope, but CLI and compatibility callers
        // must replay the same saved configuration as well.
        if let Some(configuration) = state.configuration.clone() {
            Box::pin(crate::presets::with_pipeline_config(
                configuration,
                Box::pin(self.execute_configured_work_order(
                    project_manager,
                    paths,
                    metadata,
                    state,
                    acceleration,
                )),
            ))
            .await
        } else {
            Box::pin(self.execute_configured_work_order(
                project_manager,
                paths,
                metadata,
                state,
                acceleration,
            ))
            .await
        }
    }

    async fn execute_configured_work_order(
        &self,
        project_manager: ProjectManager,
        paths: ProjectPaths,
        metadata: &mut ProjectMetadata,
        mut state: PipelineStateFile,
        acceleration: Option<&crate::engines::ColmapAccelerationStatus>,
    ) -> Result<PipelineResult> {
        self.restore_source_metrics(&state);
        if state.brush_complete {
            self.record_brush_metrics(&state);
        }
        if let Some(policy) = state.generation_policy {
            policy.validate()?;
        }
        self.bind_project_context(&paths, metadata);
        if let Some(result) = self
            .reconcile_publish_transaction(&project_manager, &paths, metadata, &mut state)
            .await?
        {
            return Ok(result);
        }
        let previous_duration = metadata.duration_ms.unwrap_or(0);
        let started = Instant::now();
        // Every non-committed continuation, including the legacy candidate
        // fast path, starts a fresh active interval. Never count stopped time.
        metadata.started_at = Some(Utc::now());
        if let Some(candidate) = crate::pipeline::artifacts::recover_saved_legacy_candidate(
            &paths,
            &state,
            &self.process_manager.child_token(),
        )
        .await?
        {
            let output = state.validated_output.clone().ok_or_else(|| {
                SplatError::Process(legacy_work_order::UNRESTORABLE_RECIPE.into())
            })?;
            return self
                .publish_work_order(
                    &project_manager,
                    &paths,
                    metadata,
                    &mut state,
                    &candidate,
                    output,
                    "continued-legacy-training".into(),
                    None,
                )
                .await;
        }
        if state.preset != metadata.quality {
            return Err(SplatError::Process(
                "项目档位与检查点不一致，无法安全继续".into(),
            ));
        }
        if let Some(candidate) = crate::pipeline::artifacts::recover_collection_candidate(
            &paths,
            &state,
            &self.process_manager.child_token(),
        )
        .await?
        {
            let output = state.validated_output.clone().ok_or_else(|| {
                SplatError::Process(legacy_work_order::UNRESTORABLE_RECIPE.into())
            })?;
            let warning = state
                .inputs
                .as_ref()
                .and_then(crate::pipeline::inputs::source_adoption_warning);
            return self
                .publish_work_order(
                    &project_manager,
                    &paths,
                    metadata,
                    &mut state,
                    &candidate,
                    output,
                    "continued-training".into(),
                    warning,
                )
                .await;
        }
        if state.inputs.is_none()
            && legacy_bridge_recovery::recover_pending(
                &paths,
                &mut state,
                &self.process_manager.child_token(),
            )
            .await?
        {
            self.retry_preparation_step(
                &paths,
                RetryPreparationOperation::WriteRetryReuseBarrier,
                "project_state",
                Some("colmap"),
                project_manager.write_state_for_preparation(&paths.state, &state),
            )
            .await?;
            legacy_bridge_recovery::acknowledge_rollback(&paths).await?;
        }
        self.prepare_continuation(&project_manager, &paths, &mut state)
            .await?;
        if self
            .try_migrate_pending_reshoot_v2(&project_manager, &paths, metadata, &mut state)
            .await?
        {
            tracing::info!(
                project_id = %paths.id,
                "Migrated an untouched reshoot V2 task to the shared generation executor"
            );
        }
        if state.inputs.is_some() {
            self.reconcile_collection_bridge(&project_manager, &paths, &mut state)
                .await?;
        }
        self.recover_work_order_database(&project_manager, &paths, &mut state)
            .await?;
        if state.inputs.is_some() {
            self.apply_collection_focal_preflight(&project_manager, &paths, &mut state)
                .await?;
        }
        let normalization_activity = self.observed_activity("正在检查可复用结果");
        let artifacts_changed = if state.inputs.is_some() {
            crate::pipeline::artifacts::normalize_collection_cancellable(
                &paths,
                &mut state,
                self.execution_run_id,
                &self.process_manager.child_token(),
            )
            .await?
        } else if state
            .reshoot
            .as_ref()
            .is_some_and(|value| value.recipe.is_some())
        {
            let owned_paths = paths.clone();
            let mut normalized = Box::new(state.clone());
            let cancel = self.process_manager.child_token();
            let (normalized, changed) = crate::presets::spawn_pipeline_blocking(move || {
                if cancel.is_cancelled() {
                    return Err(SplatError::Cancelled);
                }
                let changed = crate::pipeline::artifacts::normalize_reshoot_v2(
                    &owned_paths,
                    &mut normalized,
                )?;
                if cancel.is_cancelled() {
                    return Err(SplatError::Cancelled);
                }
                Ok::<_, SplatError>((normalized, changed))
            })
            .await
            .map_err(|error| SplatError::Process(error.to_string()))??;
            state = *normalized;
            changed
        } else {
            let before = serde_json::to_vec(&state)?;
            normalize_checkpoints(&paths, &mut state).await?;
            before != serde_json::to_vec(&state)?
        };
        normalization_activity.finish();
        drop(normalization_activity);
        if artifacts_changed {
            self.retry_preparation_step(
                &paths,
                RetryPreparationOperation::WriteRetryReuseBarrier,
                "project_state",
                None,
                project_manager.write_state_for_preparation(&paths.state, &state),
            )
            .await?;
        }
        self.prepare_continuation(&project_manager, &paths, &mut state)
            .await?;
        self.finish_database_rebuild(&project_manager, &paths, &mut state)
            .await?;
        if state.brush_complete {
            if let Some(candidate) = work_order_candidate(&paths, &state) {
                let output = match state.validated_output.clone() {
                    Some(output) => output,
                    None => recovered_candidate_output(&paths, &state, &candidate).await?,
                };
                let warning = state
                    .inputs
                    .as_ref()
                    .and_then(crate::pipeline::inputs::source_adoption_warning);
                return self
                    .publish_work_order(
                        &project_manager,
                        &paths,
                        metadata,
                        &mut state,
                        &candidate,
                        output,
                        "continued-training".into(),
                        warning,
                    )
                    .await;
            }
        }
        // A saved recipe can be adopted from a strictly validated old dataset
        // once. An absent/untrusted dataset is not proof of compatible inputs.
        if state.inputs.is_some()
            && state.brush_training.resolved.is_some()
            && state.brush_training.dataset_identity.is_none()
            && !state.brush_complete
        {
            if !state
                .artifact_manifests
                .entries
                .contains_key("trainingDataset")
            {
                return Err(SplatError::Process(
                    "Saved training recipe has no verifiable training dataset".into(),
                ));
            }
            state.brush_training.dataset_identity =
                Some(read_training_dataset_identity(&paths.brush.join("dataset")).await?);
            project_manager.write_state(&paths.state, &state).await?;
        }
        let detected;
        let acceleration = match acceleration {
            Some(value) => value,
            None => {
                detected = self.verify_pipeline_engines().await?;
                self.events.acceleration(detected.clone());
                &detected
            }
        };
        if let Some(observer) = &self.lifecycle {
            observer(RunnerUpdate::Configuration(
                crate::tasks::checkpoint_snapshot(&state),
            ));
        }
        // A resumed legacy snapshot must not inherit the new runner defaults.
        *self
            .generation_policy
            .write()
            .unwrap_or_else(|p| p.into_inner()) = state.generation_policy;
        *self
            .camera_grouping_policy_version
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            state.camera_grouping_policy_version;
        *self
            .ordinary_image_preparation_version
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            state.ordinary_image_preparation_version;
        let lifecycle = self.lifecycle.clone();
        let project_manager = project_manager.with_checkpoint_observer(Arc::new(move |state| {
            if let Some(observer) = &lifecycle {
                observer(RunnerUpdate::Configuration(
                    crate::tasks::checkpoint_snapshot(state),
                ));
            }
        }));
        self.elapsed_offset_ms
            .store(previous_duration, Ordering::Relaxed);
        metadata.status = ProjectStatus::Running;
        metadata.completed_at = None;
        metadata.failure_message = None;
        project_manager
            .write_metadata(&paths.metadata, metadata)
            .await?;
        let result = if state.inputs.is_some() {
            Box::pin(self.run_inputs(&project_manager, &paths, metadata, state, acceleration)).await
        } else if metadata.reshoot.is_some() {
            Box::pin(self.run_incremental_reshoot_project(
                &project_manager,
                &paths,
                metadata,
                state,
                acceleration,
            ))
            .await
        } else {
            Box::pin(self.run_project(&project_manager, &paths, metadata, state, acceleration))
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
            // A failed read is not an empty work order. Never overwrite a
            // damaged or temporarily inaccessible checkpoint with defaults.
            if let Ok(state) = project_manager.read_state(&paths.state).await {
                let state = mark_state_terminal(state, cancelled);
                let _ = project_manager.write_state(&paths.state, &state).await;
            }
        }
        result
    }

    /// Migrate only an untouched V2 reshoot. Once any V2 artifact has been
    /// accepted, translating it would require proving its old cache naming and
    /// dependency graph. Their reconstruction adapter must prove a safe
    /// handoff before entering the shared training tail; it never chooses a
    /// new default recipe. Untouched tasks can migrate before preparation.
    async fn try_migrate_pending_reshoot_v2(
        &self,
        manager: &ProjectManager,
        paths: &ProjectPaths,
        metadata: &mut ProjectMetadata,
        state: &mut PipelineStateFile,
    ) -> Result<bool> {
        if state.inputs.is_some() || metadata.reshoot.is_none() {
            return Ok(false);
        }
        let Some(checkpoint) = state.reshoot.as_ref() else {
            return Ok(false);
        };
        let Some(recipe) = checkpoint.recipe.as_ref() else {
            return Ok(false);
        };
        if recipe.version != crate::pipeline::reshoot::RECIPE_VERSION
            || recipe.normal_parameters_version != Some(1)
            || state.generation_policy.is_none_or(|policy| {
                // V1-V3 video checkpoints do not guarantee the persisted
                // frame-time table required by the shared per-source Bridge.
                // Keep those tasks on the compatibility runner instead of
                // silently changing their selection algorithm.
                policy.version < 4 || policy.validate().is_err()
            })
            || checkpoint.source_snapshot_complete
            || checkpoint.supplemental_frames_complete
            || checkpoint.supplemental_features_complete
            || checkpoint.incremental_matching_complete
            || checkpoint.incremental_reconstruction_complete
            || checkpoint.snapshot_v3_ready
            || state.features_complete
            || state.matching_complete
            || state.reconstruction_complete
            || state.brush_complete
        {
            return Ok(false);
        }

        if metadata.input_type == ProjectInputType::Images {
            self.authorize_external_images(&metadata.source_path)?;
        } else {
            self.authorize_external_path(&metadata.source_path)?;
        }
        let source = match metadata.input_type {
            ProjectInputType::Video if metadata.source_path.is_file() => {
                crate::pipeline::inputs::InputSource::Video {
                    path: metadata.source_path.clone(),
                }
            }
            ProjectInputType::Images if metadata.source_path.is_dir() => {
                crate::pipeline::inputs::InputSource::ImageFolder {
                    path: metadata.source_path.clone(),
                }
            }
            _ => return Ok(false),
        };
        let Ok(stored_identity) = crate::video::panorama::source_identity(&metadata.source_path)
        else {
            return Ok(false);
        };
        if recipe.stored_input_identity.as_deref() != Some(stored_identity.as_str()) {
            // Let the V2 compatibility path report its existing INPUT_CHANGED
            // error. Migration must never bless a replacement input.
            return Ok(false);
        }
        // Failure to prove the owned supplemental input still has a stable
        // identity is a migration refusal, not a reason to invalidate the
        // existing V2 compatibility path.
        let Ok(identity) = source.identity() else {
            return Ok(false);
        };
        let item = crate::pipeline::inputs::MaterialInput {
            id: "reshoot-v2-input".into(),
            source,
            identity: Some(identity),
            projection: recipe.options.projection,
            image_matching_strategy: recipe.options.image_matching_strategy,
            camera_relation: recipe.options.camera_relation,
            // The task already passed the V2 confirmation flow. These flags do
            // not invent answers; they preserve that durable admission result.
            confirmed_small_images: true,
            confirmed_image_order: true,
            frame_removal_fps: None,
        };
        let mut migrated_state = state.clone();
        let mut migrated_metadata = metadata.clone();
        if let Err(error) = self
            .ensure_reshoot_snapshot_v3(paths, &mut migrated_metadata, &mut migrated_state)
            .await
        {
            if matches!(error, SplatError::Cancelled) || error.io_error().is_some() {
                return Err(error);
            }
            tracing::warn!(
                project_id = %paths.id,
                %error,
                "Reshoot V2 snapshot migration was not provably safe; retaining compatibility runner"
            );
            return Ok(false);
        }
        migrated_state.inputs = Some(crate::pipeline::inputs::InputCheckpoint::new(
            crate::pipeline::inputs::GenerationInputs::Materials { items: vec![item] },
        ));
        if let Some(checkpoint) = &mut migrated_state.inputs {
            checkpoint.camera_grouping_policy_version =
                migrated_state.camera_grouping_policy_version;
            checkpoint.ordinary_image_preparation_version =
                migrated_state.ordinary_image_preparation_version;
        }
        // The state write is the migration switch. Snapshot construction and
        // metadata validation complete first, so an interrupted attempt either
        // remains V2 or has a fully usable immutable snapshot.
        manager
            .write_metadata(&paths.metadata, &migrated_metadata)
            .await?;
        manager.write_state(&paths.state, &migrated_state).await?;
        *metadata = migrated_metadata;
        *state = migrated_state;
        Ok(true)
    }

    async fn run_incremental_reshoot_project(
        &self,
        project_manager: &ProjectManager,
        paths: &ProjectPaths,
        metadata: &mut ProjectMetadata,
        mut state: PipelineStateFile,
        acceleration: &crate::engines::ColmapAccelerationStatus,
    ) -> Result<PipelineResult> {
        if state
            .reshoot
            .as_ref()
            .and_then(|r| r.recipe.as_ref())
            .is_some()
        {
            return self
                .run_reshoot_v2(project_manager, paths, metadata, state, acceleration)
                .await;
        }
        validate_legacy_v1_preparation_policy(state.ordinary_image_preparation_version)?;
        let provenance = metadata
            .reshoot
            .clone()
            .ok_or_else(|| SplatError::Process("补拍项目缺少来源信息".into()))?;
        self.authorize_reshoot_source(provenance.source_project_id)
            .await?;
        let mut checkpoint = state.reshoot.clone().unwrap_or_default();
        let source_root = &provenance.source_project_path;
        let source_metadata: ProjectMetadata =
            serde_json::from_slice(&tokio::fs::read(source_root.join("project.json")).await?)?;
        if !catalog::project_is_durably_completed(source_root, &source_metadata).await {
            return Err(SplatError::Process(
                "只有已完成且结果完整的项目可以进行高清补拍".into(),
            ));
        }
        let (
            source_frames,
            source_colmap_frames,
            source_database,
            source_model,
            source_colmap_mode,
        ) = if let Some(imported) = &source_metadata.imported {
            let colmap = imported.colmap.as_ref().ok_or_else(|| {
                SplatError::Process("外部导入项目没有可复用的 COLMAP 数据".into())
            })?;
            if !colmap.reshoot_eligible {
                return Err(SplatError::Process(
                    colmap
                        .reshoot_reason
                        .clone()
                        .unwrap_or_else(|| "外部 COLMAP 数据不满足高清补拍条件".into()),
                ));
            }
            let images = colmap
                .images_path
                .clone()
                .filter(|path| path.is_dir())
                .ok_or_else(|| SplatError::Process("外部 COLMAP 项目缺少原始图片".into()))?;
            let database = colmap
                .database_path
                .clone()
                .filter(|path| path.is_file())
                .ok_or_else(|| SplatError::Process("外部 COLMAP 项目缺少 database.db".into()))?;
            (
                images.clone(),
                images,
                database,
                colmap.sparse_model_path.clone(),
                None,
            )
        } else {
            let state: PipelineStateFile =
                serde_json::from_slice(&tokio::fs::read(source_root.join("state.json")).await?)?;
            let frames = source_root.join("work/frames");
            let database = source_root.join("work/colmap/database.db");
            let reconstruction =
                reusable_reconstruction(source_root, &frames, source_metadata.output.as_ref())
                    .await?;
            let resolved = resolve_reshoot_colmap_input(
                source_root,
                &source_metadata,
                &state,
                &reconstruction.model,
                &database,
            )
            .await
            .map_err(|error| {
                SplatError::Process(format!(
                    "原项目的 COLMAP 专用图片不完整或与原始帧不一致，不能作为补拍基线：{error}"
                ))
            })?;
            (
                frames,
                resolved.colmap_images,
                database,
                reconstruction.model,
                Some(resolved.mode),
            )
        };
        archive_incompatible_colmap_input_cache(paths, &state).await?;
        if !source_database.is_file()
            || std::fs::metadata(&source_database).map_or(true, |value| value.len() == 0)
        {
            return Err(SplatError::Process(
                "原项目缺少可复用的 COLMAP 数据库".into(),
            ));
        }
        let source_model_for_validation = source_model.clone();
        let source_database_for_validation = source_database.clone();
        let validate_exact_camera =
            source_colmap_mode != Some(ReshootColmapInputMode::LegacySharedFrames);
        if let Some(colmap) = source_metadata
            .imported
            .as_ref()
            .and_then(|imported| imported.colmap.clone())
        {
            crate::presets::spawn_pipeline_blocking(move || {
                crate::imports::revalidate_imported_colmap(&colmap).map(|_| ())
            })
            .await
            .map_err(|error| SplatError::Process(format!("复检外部 COLMAP 数据失败：{error}")))??;
        }
        crate::presets::spawn_pipeline_blocking(move || {
            let camera = read_single_camera(&source_model_for_validation)?;
            if validate_exact_camera {
                validate_database_camera(&source_database_for_validation, &camera)?;
            }
            Ok::<(), SplatError>(())
        })
        .await
        .map_err(|error| SplatError::Process(format!("校验补拍基线模型失败：{error}")))??;
        let source_model_files = reusable_model_files(&source_model)?;
        let base_model = paths.colmap.join("base-model");
        let database = paths.colmap.join("database.db");
        let base_database = paths.colmap.join("base-database.db");
        let reshoot_masks = paths.work.join("reshoot-masks");
        let reshoot_list = paths.colmap.join("reshoot-images.txt");
        let mapper_list = paths.colmap.join("mapper-images.txt");

        let snapshot_frames_valid = count_image_files(&paths.frames)
            .await
            .is_ok_and(|count| count >= provenance.source_image_count);
        let snapshot_colmap_frames_valid = count_image_files(&paths.colmap_frames)
            .await
            .is_ok_and(|count| count >= provenance.source_image_count);
        let snapshot_database_valid = tokio::fs::metadata(&database)
            .await
            .is_ok_and(|value| value.is_file() && value.len() > 0);
        let base_database_valid = tokio::fs::metadata(&base_database)
            .await
            .is_ok_and(|value| value.is_file() && value.len() > 0);
        if !checkpoint.source_snapshot_complete
            || checkpoint.colmap_input_version != Some(COLMAP_INPUT_VERSION)
            || !snapshot_frames_valid
            || !snapshot_colmap_frames_valid
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
            reset_directory(&paths.colmap_frames).await?;
            reset_directory(&paths.colmap).await?;
            reset_directory(&reshoot_masks).await?;
            copy_image_files(&source_frames, &paths.frames).await?;
            copy_image_files(&source_colmap_frames, &paths.colmap_frames).await?;
            tokio::fs::copy(&source_database, &base_database).await?;
            tokio::fs::copy(&base_database, &database).await?;
            tokio::fs::create_dir_all(&base_model).await?;
            for source in &source_model_files {
                let name = source.file_name().expect("model file has name");
                tokio::fs::copy(&source, base_model.join(name)).await?;
            }
            if source_colmap_mode == Some(ReshootColmapInputMode::LegacySharedFrames) {
                reshoot_runner::sync_refined_cameras(&base_database, &base_model)?;
                tokio::fs::copy(&base_database, &database).await?;
            }
            checkpoint = ReshootState {
                source_snapshot_complete: true,
                source_image_count: provenance.source_image_count,
                reshoot_frame_count: provenance.reshoot_frame_count,
                mask_count: provenance.mask_count,
                has_alpha: provenance.has_alpha,
                colmap_input_version: Some(COLMAP_INPUT_VERSION),
                ..Default::default()
            };
            state.reshoot = Some(checkpoint.clone());
            project_manager.write_state(&paths.state, &state).await?;
        }

        let ordinary_preparation = (metadata.input_type == ProjectInputType::Images)
            .then_some(state.ordinary_image_preparation_version)
            .flatten();
        if checkpoint.supplemental_frames_complete
            && (checkpoint.colmap_input_version != Some(COLMAP_INPUT_VERSION)
                || validate_image_sequence_with_preparation(
                    &paths.frames,
                    &paths.colmap_frames,
                    &reshoot_masks,
                    checkpoint.reshoot_frame_count,
                    checkpoint.has_alpha,
                    ImageSequenceNaming::Reshoot,
                    None,
                    ordinary_preparation,
                )
                .is_err())
        {
            checkpoint.supplemental_frames_complete = false;
            checkpoint.supplemental_features_complete = false;
            checkpoint.incremental_matching_complete = false;
            checkpoint.incremental_reconstruction_complete = false;
            state.brush_complete = false;
        }
        let prepared = if checkpoint.supplemental_frames_complete {
            validate_image_sequence_with_preparation(
                &paths.frames,
                &paths.colmap_frames,
                &reshoot_masks,
                checkpoint.reshoot_frame_count,
                checkpoint.has_alpha,
                ImageSequenceNaming::Reshoot,
                None,
                ordinary_preparation,
            )?
        } else {
            remove_reshoot_images(&paths.frames).await?;
            remove_reshoot_images(&paths.colmap_frames).await?;
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
            let prepared_result: Result<PreparedImageSequence> = async {
                Ok(match metadata.input_type {
                ProjectInputType::Images => {
                    let source = metadata.source_path.clone();
                    let cancellation = self.process_manager.child_token();
                    let scan = crate::presets::spawn_pipeline_blocking(move || {
                        scan_image_sequence(&source, Some(&cancellation))
                    })
                    .await
                    .map_err(|error| SplatError::Process(format!("补拍图片扫描失败：{error}")))??;
                    state.image_sequence = Some(scan.info.clone());
                    let frames = paths.frames.clone();
                    let colmap_frames = paths.colmap_frames.clone();
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
                    crate::presets::spawn_pipeline_blocking(move || {
                        prepare_scanned_image_sequence_with_preparation(
                            scan,
                            &frames,
                            &colmap_frames,
                            &masks,
                            ImageSequenceNaming::Reshoot,
                            Some(observer),
                            Some(&cancellation),
                            ordinary_preparation,
                        )
                    })
                    .await
                    .map_err(|error| SplatError::Process(format!("补拍图片准备失败：{error}")))??
                }
                ProjectInputType::Video => {
                    let temporary = paths.work.join("reshoot-extract");
                    let temporary_colmap = paths.work.join("reshoot-extract-colmap");
                    let temporary_masks = paths.work.join("reshoot-extract-masks");
                    reset_directory(&temporary).await?;
                    reset_directory(&temporary_colmap).await?;
                    reset_directory(&temporary_masks).await?;
                    let supplemental_video = probe_video(
                        &self.engines.ffprobe,
                        &metadata.source_path,
                        Some(paths.logs.join("ffprobe.log")),
                        &self.process_manager,
                    )
                    .await?;
                    state.video = Some(supplemental_video.clone());
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
                            &temporary_colmap,
                            &temporary_masks,
                            Some(&paths.logs),
                            state.planner_enabled,
                            Some(supplemental_video),
                            Some((target_width, target_height)),
                            false,
                            None,
                        )
                        .await?;
                    move_extracted_reshoot(
                        &temporary,
                        &temporary_colmap,
                        &temporary_masks,
                        &paths.frames,
                        &paths.colmap_frames,
                        &reshoot_masks,
                        extracted.has_alpha,
                    )
                    .await?;
                    validate_reshoot_image_sequence(
                        &paths.frames,
                        &paths.colmap_frames,
                        &reshoot_masks,
                        extracted.extracted_frames,
                        extracted.has_alpha,
                    )?
                }
                })
            }
            .await;
            let prepared = match prepared_result {
                Ok(prepared) => prepared,
                Err(error) => {
                    remove_reshoot_images(&paths.frames).await?;
                    remove_reshoot_images(&paths.colmap_frames).await?;
                    reset_directory(&reshoot_masks).await?;
                    return Err(error);
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
            checkpoint.colmap_input_version = Some(COLMAP_INPUT_VERSION);
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
        self.record_frame_metrics(&state, prepared.image_count, None);
        let reshoot_names = image_names_with_prefix(&paths.colmap_frames, "reshoot_").await?;
        write_image_list(&reshoot_list, &reshoot_names).await?;

        let backend_label = if acceleration.use_gpu() { "GPU" } else { "CPU" };
        let sfm_preset = metadata.quality.preset();
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
                Path::new("../colmap_frames"),
                Path::new("reshoot-images.txt"),
                provenance.camera_id,
                prepared.has_alpha.then_some(Path::new("../reshoot-masks")),
                (!state.planner_enabled).then_some(sfm_preset.sfm_max_image_size),
                Some(if state.planner_enabled {
                    sfm_preset.planned_sfm_max_features
                } else {
                    sfm_preset.sfm_max_features
                }),
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
            let original_names =
                crate::presets::spawn_pipeline_blocking(move || read_registered_images(&base))
                    .await
                    .map_err(|error| {
                        SplatError::Process(format!("读取原项目画面列表失败：{error}"))
                    })??
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
                Path::new("../colmap_frames"),
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
        // Incremental Mapper does not enable two-view tracks or bridge backfill.
        self.record_reconstruction_metrics(&report, acceleration, false, true);
        let model_copy = model.clone();
        let registered_reshoot_count =
            crate::presets::spawn_pipeline_blocking(move || read_registered_images(&model_copy))
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
            self.record_brush_metrics(&state);
            brush_candidate(&paths.brush)
                .ok_or_else(|| SplatError::Process("Brush 检查点文件缺失".into()))?
        } else {
            if let Some(result) = Box::pin(self.handoff_legacy_training(
                project_manager,
                paths,
                metadata,
                state.clone(),
                &model,
                acceleration,
            ))
            .await?
            {
                return Ok(result);
            }
            return Err(SplatError::Process(
                legacy_work_order::UNRESTORABLE_RECIPE.into(),
            ));
        };

        self.events
            .stage(PipelineStage::Exporting, 0.0, "正在校验并发布 final.ply");
        let ply = inspect_gaussian_ply(&candidate)?;
        let final_ply = paths.project.join("final.ply");
        let output = ProjectOutput {
            final_ply: final_ply.clone(),
            file_size: ply.file_size,
            splat_count: ply.splat_count,
            input_images: report.input_images,
            registered_images: report.registered_images,
            registered_ratio: report.registered_ratio,
            points_3d: report.points_3d,
        };
        let reshoot_ratio = registered_reshoot_count as f64 / prepared.image_count.max(1) as f64;
        let warning = (reshoot_ratio < crate::reconstruction::good_registered_ratio()).then(|| format!(
            "补拍画面成功加入 {registered_reshoot_count}/{}，结果已生成，但部分补拍画面未能找到与原项目的联系。",
            prepared.image_count
        ));
        self.publish_work_order(
            project_manager,
            paths,
            metadata,
            &mut state,
            &candidate,
            output,
            "legacy-reshoot-training".into(),
            warning,
        )
        .await
    }

    async fn temporary_png_focal_preflight(
        &self,
        project_manager: &ProjectManager,
        paths: &ProjectPaths,
        state: &mut PipelineStateFile,
    ) -> Result<Option<crate::video::temporary_png_focal_workaround::FocalOverride>> {
        use crate::video::temporary_png_focal_workaround::{detect, Request};
        let focal = detect(
            &self.engines.colmap,
            &self.process_manager,
            Request {
                images: &paths.colmap_frames,
                image_list: None,
                ordinary_images: true,
                camera_model: "SIMPLE_RADIAL",
                single_camera: true,
                explicit_params: None,
                existing_camera_id: None,
                calibrated: false,
            },
        )
        .await;
        let message = if let Some(focal) = &focal {
            let invalidated = apply_temporary_png_focal_checkpoint(
                project_manager,
                paths,
                state,
                &focal.checkpoint,
            )
            .await?;
            format!("[TemporaryPngFocal] enabled=true image={} focalLength={} xResolution={} unit={} cameraParams={} cacheInvalidated={invalidated}",
                focal.checkpoint.image_name, focal.raw_focal_length, focal.raw_x_resolution,
                focal.unit, focal.camera_params())
        } else {
            "[TemporaryPngFocal] enabled=false (input/runtime not confirmed applicable)".to_owned()
        };
        self.events.send(
            // A preflight log must not advance progress before a lifecycle stage.
            PipelineStage::Created,
            Some(PipelineEngine::System),
            EventKind::Log,
            EventLevel::Info,
            None,
            true,
            message,
            None,
            None,
            None,
        );
        Ok(focal)
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
        // This compatibility executor still probes its source eagerly. The
        // candidate/Commit-only paths have already returned before reaching it.
        if metadata.input_type == ProjectInputType::Images {
            self.authorize_external_images(&metadata.source_path)?;
        } else {
            self.authorize_external_path(&metadata.source_path)?;
        }
        let mut panorama_video = None;
        if metadata.input_type == ProjectInputType::Video
            && state.video_projection != crate::video::panorama::VideoProjection::Perspective
        {
            let video = probe_video(
                &self.engines.ffprobe,
                &metadata.source_path,
                Some(paths.logs.join("ffprobe.log")),
                &self.process_manager,
            )
            .await?;
            let detection = crate::video::panorama::detect(
                &self.engines.ffprobe,
                &self.engines.ffmpeg,
                &metadata.source_path,
                &video,
                &self.process_manager,
                false,
            )
            .await?;
            if crate::video::panorama::resolve(state.video_projection, &detection)? {
                let input = metadata.source_path.clone();
                let info = video.clone();
                let native = state.generation_policy.is_some();
                let cancellation = self.process_manager.child_token();
                let config = crate::presets::spawn_pipeline_blocking(move || {
                    crate::video::panorama::PanoramaCheckpoint::new_for_policy(
                        &input,
                        quality,
                        &info,
                        &cancellation,
                        native,
                    )
                })
                .await
                .map_err(|e| SplatError::Process(e.to_string()))??;
                if state
                    .panorama
                    .as_ref()
                    .is_some_and(|previous| previous != &config)
                {
                    return Err(SplatError::Process("INPUT_CHANGED".into()));
                }
                state.panorama = Some(config);
                state.video = Some(video.clone());
                panorama_video = Some(video);
                project_manager.write_state(&paths.state, &state).await?;
            } else if state.panorama.is_some() {
                return Err(SplatError::Process(
                    "源视频投影已发生变化，不能复用球面重建；请将修改后的素材创建为新任务".into(),
                ));
            }
        }
        if metadata.input_type == ProjectInputType::Images
            && state.video_projection != crate::video::panorama::VideoProjection::Perspective
        {
            let source = metadata.source_path.clone();
            let cancellation = self.process_manager.child_token();
            let requested = state.video_projection;
            let native = state.generation_policy.is_some();
            let config = crate::presets::spawn_pipeline_blocking(move || {
                let scan = crate::video::image_projection::analyze(&source)?;
                if requested == crate::video::panorama::VideoProjection::Auto && !scan.detection.confirmed_erp {
                    if scan.detection.requires_choice {
                        return Err(SplatError::InvalidVideo("无法判断图片类型。请选择普通图片或全景图片。当前不支持双鱼眼、双目、VR180、裁切全景和厂商私有原片。".into()));
                    }
                    return Ok(None);
                }
                crate::video::image_projection::checkpoint_for_policy(&source, quality, &cancellation, native).map(Some)
            }).await.map_err(|e| SplatError::Process(e.to_string()))??;
            if let Some(config) = config {
                if state
                    .panorama
                    .as_ref()
                    .is_some_and(|previous| previous != &config)
                {
                    return Err(SplatError::Process("INPUT_CHANGED".into()));
                }
                state.panorama = Some(config);
                state.video_projection = crate::video::panorama::VideoProjection::Equirectangular;
                project_manager.write_state(&paths.state, &state).await?;
            }
        }
        self.events.panorama_images.store(
            state
                .panorama
                .as_ref()
                .is_some_and(|p| p.image_sources.is_some()),
            Ordering::Relaxed,
        );
        self.events
            .panorama_phase
            .store(u64::from(state.panorama.is_some()), Ordering::Relaxed);
        if state.input_type != metadata.input_type {
            return Err(SplatError::Process(
                "项目输入类型与检查点不一致，无法安全继续".into(),
            ));
        }
        archive_incompatible_colmap_input_cache(paths, &state).await?;
        // Temporary #73 preflight MUST precede checkpoint/database reuse.
        let ordinary_images = metadata.input_type == ProjectInputType::Images
            && metadata.imported.is_none()
            && metadata.reshoot.is_none()
            && state.panorama.is_none();
        let mut focal_checked = false;
        let mut focal_override = None;
        if ordinary_images
            && state.frames.as_ref().is_some_and(|frames| {
                frames.colmap_input_version == Some(COLMAP_INPUT_VERSION)
                    && frames.extracted_frames.is_some_and(|count| count > 0)
            })
        {
            focal_override = self
                .temporary_png_focal_preflight(project_manager, paths, &mut state)
                .await?;
            focal_checked = true;
        }
        recover_interrupted_publish(paths, &state).await?;
        normalize_checkpoints(paths, &mut state).await?;
        project_manager.write_state(&paths.state, &state).await?;
        let uses_current_resolution_policy = state.planner_enabled
            && state.resolution_policy_version == Some(PLANNER_RESOLUTION_POLICY_VERSION);
        let mut preprobed_video = None;
        if state.panorama.is_none()
            && uses_current_resolution_policy
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
            let resolution = self.resolution_plan(
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
            .panorama
            .as_ref()
            .map(|p| (p.sfm_width, p.sfm_height))
            .or_else(|| {
                state
                    .resolution_plan
                    .map(|plan| (plan.working_width, plan.working_height))
            });
        let mut prepared =
            if let Some(prepared) = prepared_frames_from_checkpoint(paths, &state).await? {
                self.events.stage(
                    PipelineStage::ExtractingFrames,
                    1.0,
                    format!("已复用 {} 帧检查点", prepared.extracted_frames),
                );
                prepared
            } else {
                focal_checked = false;
                focal_override = None;
                reset_directory(&paths.frames).await?;
                reset_directory(&paths.colmap_frames).await?;
                reset_directory(&paths.masks).await?;
                reset_directory(&paths.colmap).await?;
                reset_directory(&paths.brush).await?;
                let prepared = match metadata.input_type {
                    ProjectInputType::Video => {
                        self.prepare_frames(
                            &metadata.source_path,
                            quality,
                            &paths.frames,
                            &paths.colmap_frames,
                            &paths.masks,
                            Some(&paths.logs),
                            state.planner_enabled,
                            panorama_video
                                .or(preprobed_video)
                                .or_else(|| state.video.clone()),
                            target_dimensions,
                            state.panorama.is_some(),
                            None,
                        )
                        .await?
                    }
                    ProjectInputType::Images => {
                        if let Some(config) = &state.panorama {
                            self.prepare_panorama_images(
                                &metadata.source_path,
                                quality,
                                config,
                                &paths.frames,
                                &paths.colmap_frames,
                                &paths.masks,
                            )
                            .await?
                        } else {
                            self.prepare_images(
                                &metadata.source_path,
                                quality,
                                &paths.frames,
                                &paths.colmap_frames,
                                &paths.masks,
                                state.image_sequence.is_some(),
                            )
                            .await?
                        }
                    }
                };
                state.input_type = prepared.input_type;
                state.video = prepared.video.clone();
                state.image_sequence = prepared.image_sequence.clone();
                if state.panorama.is_none()
                    && uses_current_resolution_policy
                    && state.resolution_plan.is_none()
                {
                    state.resolution_plan = Some(self.resolution_plan(
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
                frames.colmap_input_version = Some(COLMAP_INPUT_VERSION);
                state.frames = Some(frames);
                state.features_complete = false;
                state.matching_complete = false;
                state.reconstruction_complete = false;
                state.brush_complete = false;
                state.stage = PipelineStage::ExtractingFrames;
                project_manager.write_state(&paths.state, &state).await?;
                prepared
            };
        if ordinary_images && !focal_checked {
            focal_override = self
                .temporary_png_focal_preflight(project_manager, paths, &mut state)
                .await?;
        }
        let camera_params = focal_override.as_ref().map(|focal| focal.camera_params());
        let source_duration_seconds = prepared.video.as_ref().map(|video| video.duration);
        self.record_frame_metrics(&state, prepared.extracted_frames, Some(&prepared.plan));

        let database = paths.colmap.join("database.db");
        let sparse = paths.colmap.join("sparse");
        let colmap_log = paths.logs.join("colmap.log");
        let preset = quality.preset();
        let sfm_max_image_size = state
            .panorama
            .as_ref()
            .map(|p| {
                if state.generation_policy.is_some() && quality == Quality::High {
                    p.sfm_width.min(4096)
                } else {
                    p.sfm_width
                }
            })
            .unwrap_or_else(|| {
                state
                    .resolution_plan
                    .map(|plan| plan.sfm_max_image_size)
                    .unwrap_or(if state.planner_enabled {
                        preset.planned_sfm_max_image_size
                    } else {
                        preset.sfm_max_image_size
                    })
            });
        if let Some(config) = &state.panorama {
            tokio::fs::create_dir_all(&paths.colmap).await?;
            tokio::fs::write(
                paths.colmap.join("panorama-camera.json"),
                serde_json::to_vec(config)?,
            )
            .await?;
        }
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
        let colmap_images = Path::new("../colmap_frames");
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
            if let Some(config) = &state.panorama {
                tokio::fs::write(
                    paths.colmap.join("panorama-camera.json"),
                    serde_json::to_vec(config)?,
                )
                .await?;
            }
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
                    preset.planned_sfm_max_features,
                    colmap_log.clone(),
                    &self.process_manager,
                    observer,
                    gpu_index,
                    camera_params.as_deref(),
                )
                .await?;
            } else {
                colmap::extract_features(
                    &self.engines.colmap,
                    &database,
                    colmap_images,
                    colmap_masks,
                    Some(sfm_max_image_size),
                    Some(preset.sfm_max_features),
                    colmap_log.clone(),
                    &self.process_manager,
                    observer,
                    gpu_index,
                    camera_params.as_deref(),
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

        // Resumes deliberately consult the persisted strategy, never the new-run builder.
        let (sequential_matching, loop_detection) = image_matching_policy(
            prepared.input_type,
            state.image_matching_strategy,
            quality,
            state.planner_enabled,
        );
        if state.matching_complete {
            self.events.stage(
                PipelineStage::Matching,
                1.0,
                if !sequential_matching {
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
                    if !sequential_matching {
                        "穷举匹配"
                    } else if loop_detection {
                        "顺序匹配（已开启闭环检测）"
                    } else {
                        "顺序匹配"
                    }
                ),
            );
            let observer = Some(self.process_observer(
                PipelineStage::Matching,
                PipelineEngine::Colmap,
                Some(prepared.extracted_frames),
                ObserverMode::Matching,
            ));
            if !sequential_matching {
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
                    loop_detection,
                )
                .await?;
            }
            state.stage = PipelineStage::Matching;
            state.matching_complete = true;
            project_manager.write_state(&paths.state, &state).await?;
            self.events.stage(
                PipelineStage::Matching,
                1.0,
                if !sequential_matching {
                    "穷举匹配完成"
                } else if loop_detection {
                    "顺序匹配（已开启闭环检测）完成"
                } else {
                    "顺序匹配完成"
                },
            );
        }

        let allow_two_view_tracks = if state.generation_policy.is_some() {
            self.actual_transparency(paths, &mut state, prepared.has_alpha)
                .await?
        } else {
            should_allow_two_view_tracks(
                state.planner_enabled,
                preset.sfm_allow_two_view_tracks,
                prepared.has_alpha,
            )
        };
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
                allow_two_view_tracks,
                state.planner_enabled,
                colmap_log.clone(),
                &self.process_manager,
                Some(self.process_observer(
                    PipelineStage::Reconstructing,
                    PipelineEngine::Colmap,
                    Some(prepared.extracted_frames),
                    ObserverMode::mapper_for_projection(state.panorama.is_some()),
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
        self.record_reconstruction_metrics(
            &initial_report,
            acceleration,
            allow_two_view_tracks,
            false,
        );
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
        let (model, report) =
            if state.gps.is_some() && prepared.input_type == ProjectInputType::Images {
                self.authorize_external_images(&metadata.source_path)?;
                let sources = gps_runner::generation_inputs(metadata, &state)?;
                let cancel = self.process_manager.child_token();
                let records = tokio::task::spawn_blocking(move || {
                    crate::reconstruction::gps::collect(&sources, &cancel)
                })
                .await
                .map_err(|e| SplatError::Process(e.to_string()))??;
                if let Some(adjusted) = self
                    .apply_gps(
                        project_manager,
                        paths,
                        &mut state,
                        records,
                        None,
                        &model,
                        None,
                        None,
                    )
                    .await?
                {
                    let report = ReconstructionValidator::validate(&paths.frames, &adjusted)?;
                    (adjusted, report)
                } else {
                    (model, report)
                }
            } else {
                (model, report)
            };
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
            self.record_brush_metrics(&state);
            self.events.stage(
                PipelineStage::TrainingSplats,
                1.0,
                "已复用 Brush 训练检查点",
            );
            brush_candidate(&paths.brush)
                .ok_or_else(|| SplatError::Process("Brush 检查点文件缺失，无法继续发布".into()))?
        } else {
            if let Some(result) = Box::pin(self.handoff_legacy_training(
                project_manager,
                paths,
                metadata,
                state.clone(),
                &model,
                acceleration,
            ))
            .await?
            {
                return Ok(result);
            }
            return Err(SplatError::Process(
                legacy_work_order::UNRESTORABLE_RECIPE.into(),
            ));
        };

        self.events
            .stage(PipelineStage::Exporting, 0.0, "正在校验并发布 final.ply");
        let ply = inspect_gaussian_ply(&candidate)?;
        let final_ply = paths.project.join("final.ply");
        let output = ProjectOutput {
            final_ply: final_ply.clone(),
            file_size: ply.file_size,
            splat_count: ply.splat_count,
            input_images: report.input_images,
            registered_images: report.registered_images,
            registered_ratio: report.registered_ratio,
            points_3d: report.points_3d,
        };
        let mut result = self
            .publish_work_order(
                project_manager,
                paths,
                metadata,
                &mut state,
                &candidate,
                output,
                "normal-training".into(),
                warning,
            )
            .await?;
        result.source_duration_seconds = source_duration_seconds;
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    async fn prepare_fine_splat_budget(
        &self,
        state: &mut PipelineStateFile,
        quality: Quality,
        model: &Path,
        images: &Path,
        panorama: bool,
        max_resolution: u32,
    ) -> Result<()> {
        if panorama
            || state
                .generation_policy
                .is_none_or(|p| !p.requires_pixel_budget(quality))
            || state.brush_training.resolved.is_some()
        {
            return Ok(());
        }
        if let Some(saved) = state.brush_training.fine_splat_budget {
            if quality == Quality::High {
                saved.validated_ceiling(saved.sparse_points)?;
            } else {
                saved.validated_ceiling_for_quality(
                    saved.sparse_points,
                    quality,
                    max_resolution,
                )?;
            }
            return Ok(());
        }
        let model = model.to_owned();
        let images = images.to_owned();
        let cancel = self.process_manager.child_token();
        let budget = crate::presets::spawn_pipeline_blocking(move || {
            if quality == Quality::High {
                // Retain the existing Detailed snapshot and hash format.
                super::splat_budget::calculate(&model, &images, &cancel)
            } else {
                super::splat_budget::calculate_for_quality(
                    &model,
                    &images,
                    quality,
                    max_resolution,
                    &cancel,
                )
            }
        })
        .await
        .map_err(|e| SplatError::Process(e.to_string()))?
        .map_err(|error| error.with_io_diagnostic(
            "training_preparation_operation=calculate_splat_budget\npath_role=training_images_and_model\nio_substep=read_and_validate"
        ))?;
        self.events.send(
            PipelineStage::TrainingSplats, Some(PipelineEngine::System), EventKind::Log,
            EventLevel::Info, None, false,
            format!("[SplatBudget] version={} k={} effectivePixels={} registeredImages={} meanTrackLength={:.6} maxSplats={}",
                budget.version, budget.coefficient, budget.effective_pixels, budget.registered_images,
                budget.mean_track_length, budget.requested_max_splats),
            None, None, None,
        );
        state.brush_training.fine_splat_budget = Some(budget);
        Ok(())
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
        let mut resolved = resolved;
        if state.generation_policy.is_some() && state.brush_training.oom_retry_used {
            if let Some(count) = state.brush_training.oom_retry_source_count {
                let cap = count
                    .checked_mul(4)
                    .map(|n| n / 5)
                    .and_then(|n| u32::try_from(n).ok())
                    .filter(|cap| *cap > 0 && u64::from(*cap) >= initial_sfm_points)
                    .ok_or_else(|| {
                        SplatError::Process(
                            "Initialization exceeds the saved OOM retry budget".into(),
                        )
                    })?;
                resolved.preset.max_splats =
                    Some(resolved.preset.max_splats.map_or(cap, |old| old.min(cap)));
                state.brush_training.resolved = Some(resolved);
                project_manager
                    .write_state_for_training(&paths.state, state)
                    .await?;
            }
        }
        self.prepare_brush_observation(state);
        self.log_brush_profile("BrushProfile", &resolved, EventLevel::Info);
        let warm = state
            .reshoot
            .as_ref()
            .and_then(|r| r.recipe.as_ref())
            .and_then(|r| r.warm_parameters);
        let alpha_mode = state.brush_training.alpha_mode(
            state
                .reshoot
                .as_ref()
                .and_then(|r| r.recipe.as_ref())
                .is_some(),
        )?;
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
        let (first_observer, first_started) = track_process_start(self.process_observer(
            PipelineStage::TrainingSplats,
            PipelineEngine::Brush,
            Some(resolved.preset.total_steps as u64),
            ObserverMode::Brush,
        ));
        let first = brush::train(
            &self.engines.brush,
            dataset,
            &paths.brush,
            brush::BrushTrainingOptions {
                init_sh_degree: state.inputs.as_ref().and_then(|i| i.init_sh_degree),
                alpha_mode,
                warm,
                preset: resolved.preset,
                panorama: resolved.panorama.is_some(),
                log_path: first_log,
                gpu_launch_policy: &gpu_launch_policy,
            },
            &self.process_manager,
            Some(first_observer),
        )
        .await;
        if matches!(&first, Err(SplatError::Cancelled)) {
            return first;
        }
        if state.brush_training.oom_retry_used && first_started.load(Ordering::Relaxed) {
            state.brush_training.oom_retry_started = true;
            if let Err(write_error) = project_manager
                .write_state_for_training(&paths.state, state)
                .await
            {
                return Err(match first {
                    Err(engine_error) => write_error.with_previous_attempt(engine_error),
                    Ok(_) => write_error,
                });
            }
            self.mark_automatic_retry();
        }

        match first {
            Ok(candidate) => Ok(candidate),
            Err(error)
                if error.is_brush_oom()
                    && state.generation_policy.map_or(
                        state.planner_enabled
                            && (quality == Quality::High || resolved.panorama.is_some()),
                        |p| p.auto_retry_enabled,
                    )
                    && !state.brush_training.oom_retry_used =>
            {
                let fallback = if let Some(policy) = state.generation_policy {
                    let count = match &error {
                        SplatError::EngineProcess(f) => f.last_splat_count,
                        _ => None,
                    };
                    policy.oom_retry(
                        resolved,
                        state.brush_training.oom_retry_used,
                        count,
                        initial_sfm_points,
                    )
                } else {
                    brush_oom_fallback(
                        quality,
                        state.planner_enabled,
                        state.brush_training.oom_retry_used,
                        resolved,
                        initial_sfm_points,
                    )
                };
                let Some(mut downgraded) = fallback else {
                    return Err(error);
                };
                if warm.is_some() && state.generation_policy.is_none() {
                    downgraded.preset.total_steps = resolved.preset.total_steps;
                    downgraded.preset.densification = resolved.preset.densification;
                    downgraded.preset.max_splats = Some(
                        downgraded
                            .preset
                            .max_splats
                            .unwrap_or(10_000_000)
                            .max(initial_sfm_points.min(u32::MAX as u64) as u32),
                    );
                }
                state.brush_training.oom_retry_used = true;
                state.brush_training.oom_retry_source_count = match &error {
                    SplatError::EngineProcess(f) => f.last_splat_count,
                    _ => None,
                };
                state.brush_training.resolved = Some(downgraded);
                if let Err(write_error) = project_manager
                    .write_state_for_training(&paths.state, state)
                    .await
                {
                    return Err(write_error.with_previous_attempt(error));
                }
                self.events.send(
                    PipelineStage::TrainingSplats,
                    Some(PipelineEngine::Brush),
                    EventKind::Log,
                    EventLevel::Warning,
                    None,
                    true,
                    format!(
                        "[BrushRetry] reason=OutOfMemory from={} to={} lastReportedSplats={:?} maxSplats={:?}，复用现有 SfM 和训练数据重试一次",
                        resolved.profile.label(),
                        downgraded.profile.label(),
                        state.brush_training.oom_retry_source_count,
                        downgraded.preset.max_splats,
                    ),
                    None,
                    None,
                    None,
                );
                self.log_brush_profile("BrushRetryProfile", &downgraded, EventLevel::Warning);
                self.prepare_brush_observation(state);
                let (retry_observer, retry_started) = track_process_start(self.process_observer(
                    PipelineStage::TrainingSplats,
                    PipelineEngine::Brush,
                    Some(downgraded.preset.total_steps as u64),
                    ObserverMode::Brush,
                ));
                let retried = brush::train(
                    &self.engines.brush,
                    dataset,
                    &paths.brush,
                    brush::BrushTrainingOptions {
                        init_sh_degree: state.inputs.as_ref().and_then(|i| i.init_sh_degree),
                        alpha_mode,
                        warm,
                        preset: downgraded.preset,
                        panorama: downgraded.panorama.is_some(),
                        log_path: paths.logs.join("brush-retry.log"),
                        gpu_launch_policy: &gpu_launch_policy,
                    },
                    &self.process_manager,
                    Some(retry_observer),
                )
                .await;
                if matches!(&retried, Err(SplatError::Cancelled)) {
                    return retried;
                }
                if retry_started.load(Ordering::Relaxed) {
                    state.brush_training.oom_retry_started = true;
                    if let Err(write_error) = project_manager
                        .write_state_for_training(&paths.state, state)
                        .await
                    {
                        let write_error = write_error.with_previous_attempt(error);
                        return Err(match retried {
                            Err(retry_error) => write_error.with_previous_attempt(retry_error),
                            Ok(_) => write_error,
                        });
                    }
                    self.mark_automatic_retry();
                }
                retried.map_err(|retry_error| retry_error.with_previous_attempt(error))
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
        if let Some(snapshot) = resolved.panorama {
            self.events.send(
                PipelineStage::TrainingSplats, Some(PipelineEngine::Brush),
                EventKind::Log, level, None, true,
                format!("[PanoramaBrushRecipe] version={} quality={} automaticOptimization={} memoryMaxSplats={:?} initialPoints={} initialPointsOverride={} growthStart=0 shDegree=3",
                    snapshot.recipe_version, snapshot.quality, snapshot.automatic_optimization,
                    snapshot.memory_max_splats, snapshot.initial_points, snapshot.initial_points_override),
                None, None, None,
            );
        }
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
                    if let Some((model, report)) =
                        colmap::select_reconstruction_model(&model, prepared.extracted_frames)?
                    {
                        return Ok((model, report));
                    }
                }
                // A missing completed output is not evidence that the original
                // database/frames are still the pre-Bridge set.
                return Err(SplatError::Process(
                    legacy_work_order::UNRESTORABLE_RECIPE.into(),
                ));
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
            let registered = read_registered_source_indices(&initial_model)?;
            let frame_plan = prepared.plan.clone();
            let registered_images = initial_report.registered_images;
            let cancellation = self.process_manager.child_token();
            let plan = crate::presets::spawn_pipeline_blocking(move || {
                plan_bridge_backfill_cancellable(
                    &frame_plan,
                    &registered,
                    registered_images,
                    &cancellation,
                )
            })
            .await
            .map_err(|e| SplatError::Process(e.to_string()))??;
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
            plan
        };

        validate_manual_bridge_plan(&prepared.plan, &bridge_plan)?;
        let work = paths.work.clone();
        let panorama = state.panorama.is_some();
        let plan = prepared.plan.clone();
        let indices = bridge_plan.selected_frame_indices.clone();
        let fps = prepared.video.as_ref().map(|v| v.fps);
        let additional = crate::presets::spawn_pipeline_blocking(move || {
            resolve_bridge_frames(&work, panorama, &plan, &indices, fps)
        })
        .await
        .map_err(|e| SplatError::Process(e.to_string()))??;
        if additional.len() != bridge_plan.selected_frame_indices.len() {
            return Err(SplatError::Process(
                legacy_work_order::UNRESTORABLE_RECIPE.into(),
            ));
        }

        legacy_bridge_recovery::prepare(
            paths,
            state,
            prepared,
            &bridge_plan,
            &initial_model,
            &initial_report,
            &self.process_manager.child_token(),
        )
        .await?;
        state.bridge_backfill.status = BridgeBackfillStatus::Running;
        project_manager.write_state(&paths.state, state).await?;

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

        self.authorize_external_path(&metadata.source_path)?;
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
            Ok(LegacyBridgeAttempt::Accepted(model, report)) => {
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
            Ok(LegacyBridgeAttempt::GeometryRejected(error)) => {
                let bridge_duration_ms = bridge_started.elapsed().as_millis() as u64;
                let seed = initial_model.clone();
                let count = bridge_plan.initial_input_images;
                let (_, verified) = crate::presets::spawn_pipeline_blocking(move || {
                    colmap::select_reconstruction_model(&seed, count)
                })
                .await
                .map_err(|error| SplatError::Process(error.to_string()))??
                .ok_or_else(|| {
                    SplatError::Process(legacy_work_order::UNRESTORABLE_RECIPE.into())
                })?;
                if verified.registered_images != initial_report.registered_images
                    || verified.points_3d != initial_report.points_3d
                {
                    return Err(SplatError::Process(
                        legacy_work_order::UNRESTORABLE_RECIPE.into(),
                    ));
                }
                let rollback = async {
                    if self.process_manager.child_token().is_cancelled() {
                        return Err(SplatError::Cancelled);
                    }
                    legacy_bridge_recovery::mark_rollback(paths, bridge_duration_ms).await?;
                    legacy_bridge_recovery::recover_pending(
                        paths,
                        state,
                        &self.process_manager.child_token(),
                    )
                    .await?;
                    legacy_bridge_recovery::restore_prepared(paths, prepared)?;
                    project_manager.write_state(&paths.state, state).await?;
                    legacy_bridge_recovery::acknowledge_rollback(paths).await
                }
                .await;
                if let Err(restore_error) = rollback {
                    return Err(restore_error.with_previous_attempt(error));
                }
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
                    Some(verified.registered_images),
                    Some(verified.input_images),
                    Some("images"),
                );
                Ok((initial_model, verified))
            }
            Err(error) => Err(error),
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
    ) -> Result<LegacyBridgeAttempt> {
        self.authorize_external_path(&metadata.source_path)?;
        self.events.stage(
            PipelineStage::ValidatingReconstruction,
            0.0,
            format!("Bridge Backfill 正在提取 {} 张新增帧", additional.len()),
        );
        let extraction = extract_additional_frames(
            &self.engines.ffmpeg,
            &metadata.source_path,
            &paths.frames,
            &paths.colmap_frames,
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
            .unwrap_or(if state.planner_enabled {
                preset.planned_sfm_max_image_size
            } else {
                preset.sfm_max_image_size
            });
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
            preset.planned_sfm_max_features,
            paths.logs.join("colmap-bridge-features.log"),
            &self.process_manager,
            Some(self.process_observer(
                PipelineStage::ValidatingReconstruction,
                PipelineEngine::Colmap,
                Some(additional.len() as u64),
                ObserverMode::BracketProgress,
            )),
            gpu_index,
            None,
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
                ObserverMode::Matching,
            )),
            gpu_index,
        )
        .await?;

        let bridge_sparse = paths.colmap.join("sparse-bridge");
        let bridge_two_view = if state.generation_policy.is_some() {
            if state.actual_transparency != Some(true) {
                state.actual_transparency = None;
            }
            self.actual_transparency(paths, state, prepared.has_alpha)
                .await?
        } else {
            false
        };
        reset_directory(&bridge_sparse).await?;
        self.events.stage(
            PipelineStage::ValidatingReconstruction,
            0.0,
            "Bridge Backfill 正在复用初始模型继续增量重建",
        );
        let input_model = initial_model
            .strip_prefix(&paths.colmap)
            .unwrap_or(initial_model);
        let mapped = colmap::map_from_existing(
            &self.engines.colmap,
            database,
            colmap_images,
            input_model,
            &bridge_sparse,
            state.planner_enabled,
            bridge_two_view,
            colmap_log.with_file_name("colmap-bridge-mapper.log"),
            &self.process_manager,
            Some(self.process_observer(
                PipelineStage::ValidatingReconstruction,
                PipelineEngine::Colmap,
                Some(prepared.extracted_frames),
                ObserverMode::mapper_for_projection(state.panorama.is_some()),
            )),
            gpu_index,
        )
        .await;
        if let Err(error) = mapped {
            return if colmap::is_recoverable_mapper_geometry_failure(&error) {
                Ok(LegacyBridgeAttempt::GeometryRejected(error))
            } else {
                Err(error)
            };
        }
        let sparse = bridge_sparse.clone();
        let count = prepared.extracted_frames;
        let candidate = crate::presets::spawn_pipeline_blocking(move || {
            colmap::select_reconstruction_model(&sparse, count)
        })
        .await
        .map_err(|error| SplatError::Process(error.to_string()))??;
        let Some((model, report)) = candidate else {
            return Ok(LegacyBridgeAttempt::GeometryRejected(SplatError::Process(
                "COLMAP did not produce a usable sparse model".into(),
            )));
        };
        if report.registered_images < initial_report.registered_images {
            return Ok(LegacyBridgeAttempt::GeometryRejected(SplatError::Process(
                format!(
                    "Bridge model registered fewer images than the initial model ({} < {})",
                    report.registered_images, initial_report.registered_images
                ),
            )));
        }
        Ok(LegacyBridgeAttempt::Accepted(model, report))
    }

    fn process_observer(
        &self,
        stage: PipelineStage,
        engine: PipelineEngine,
        expected_total: Option<u64>,
        mode: ObserverMode,
    ) -> ProcessObserver {
        let feature = if mode == ObserverMode::BracketProgress {
            expected_total.filter(|total| *total > 0).map(|total| {
                let source = self
                    .events
                    .context
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .source_id
                    .clone()
                    .unwrap_or_default();
                let key = format!("single-feature:{source}");
                FeatureProgressScope::new([(key.clone(), total)]).group(key)
            })
        } else {
            None
        };
        self.process_observer_with_feature(stage, engine, expected_total, mode, feature)
    }

    fn feature_process_observer(
        &self,
        stage: PipelineStage,
        scope: &FeatureProgressScope,
        logical_group: String,
    ) -> ProcessObserver {
        let feature = scope.group(logical_group);
        self.process_observer_with_feature(
            stage,
            PipelineEngine::Colmap,
            feature.expected(),
            ObserverMode::BracketProgress,
            Some(feature),
        )
    }

    fn complete_feature_progress(&self, stage: PipelineStage, scope: &FeatureProgressScope) {
        if self.process_manager.child_token().is_cancelled() {
            return;
        }
        if let Some((current, total)) = scope.validated_complete() {
            self.events.send(
                stage,
                Some(PipelineEngine::System),
                EventKind::Progress,
                EventLevel::Info,
                Some(1.0),
                false,
                format!("正在分析画面（{current}/{total}）"),
                Some(current),
                Some(total),
                Some("images"),
            );
        }
    }

    fn complete_feature_group(
        &self,
        stage: PipelineStage,
        scope: &FeatureProgressScope,
        key: String,
    ) {
        if self.process_manager.child_token().is_cancelled() {
            return;
        }
        let operation = self
            .events
            .context
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .current
            .clone();
        if let Some((current, total)) =
            operation.and_then(|operation| scope.group(key).validated(operation.id))
        {
            self.events.send(
                stage,
                Some(PipelineEngine::System),
                EventKind::Progress,
                EventLevel::Info,
                Some((current as f32 / total as f32).min(0.999)),
                false,
                format!("正在分析画面（{current}/{total}）"),
                Some(current),
                Some(total),
                Some("images"),
            );
        }
    }

    fn process_observer_with_feature(
        &self,
        stage: PipelineStage,
        engine: PipelineEngine,
        expected_total: Option<u64>,
        mode: ObserverMode,
        feature: Option<FeatureGroupProgress>,
    ) -> ProcessObserver {
        let events = self.events.clone();
        let mapper_count = Arc::new(AtomicU64::new(0));
        let mapper_estimate = Arc::new(std::sync::Mutex::new(MapperProgressEstimate::default()));
        let runtime = Arc::new(std::sync::Mutex::new(RuntimeTracker::new()));
        let matching_notice_elapsed = Arc::new(AtomicU64::new(0));
        let process_active = Arc::new(AtomicBool::new(false));
        let cancellation = self.process_manager.child_token();
        let operation = Arc::new(std::sync::Mutex::new(None));
        let acceleration = self.current_acceleration.clone();
        let effectiveness = self.effectiveness.clone();
        let artifact_recorder = self.artifact_recorder.clone();
        let display = Arc::new(std::sync::Mutex::new(EngineDisplayGate::default()));
        let quiet_visible_at = Arc::new(std::sync::Mutex::new(None::<Instant>));
        let runtime_signature = Arc::new(AtomicU64::new(0));
        let diagnostic_logs = self.diagnostic_logs.clone();
        let project = self.active_project.clone();
        let run_id = self.execution_run_id;
        Arc::new(move |update| {
            let mut scoped = events.clone();
            if matches!(update, ProcessUpdate::Started { .. }) {
                scoped = if let Some(feature) = &feature {
                    events.begin_process_with_key(stage, Some(feature.key()))
                } else {
                    events.begin_process(stage)
                };
                *operation.lock().unwrap_or_else(|p| p.into_inner()) =
                    scoped.bound_operation.clone();
            } else {
                scoped.bound_operation =
                    operation.lock().unwrap_or_else(|p| p.into_inner()).clone();
            }
            let events = scoped;
            if matches!(
                &update,
                ProcessUpdate::Line { .. }
                    | ProcessUpdate::Heartbeat { .. }
                    | ProcessUpdate::Resources(_)
            ) && events.bound_operation.as_ref().is_some_and(|bound| {
                events
                    .context
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .current
                    .as_ref()
                    .map(|current| current.id)
                    != Some(bound.id)
            }) {
                return;
            }
            let emit_display = |event: EngineDisplay| {
                let owner = if feature.is_some() && event.kind == EventKind::Progress {
                    PipelineEngine::System
                } else {
                    engine
                };
                event.emit(&events, stage, owner);
            };
            match update {
                ProcessUpdate::LogOpened { path, offset } => {
                    let root = project
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .as_ref()
                        .map(|value| value.logs_directory.clone());
                    if let (Some(run), Some(operation), Some(root)) =
                        (run_id, events.bound_operation.as_ref(), root)
                    {
                        let process_id = runtime
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .snapshot
                            .process_id;
                        diagnostic_logs
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .opened(run, operation, engine, process_id, &root, &path, offset);
                    }
                }
                ProcessUpdate::LogClosed {
                    path,
                    end,
                    success,
                    termination,
                } => {
                    if let (Some(run), Some(operation)) = (run_id, events.bound_operation.as_ref())
                    {
                        let process_id = runtime
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .snapshot
                            .process_id;
                        diagnostic_logs
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .closed(
                                run,
                                operation,
                                engine,
                                process_id,
                                &path,
                                end,
                                success,
                                termination,
                            );
                    }
                }
                ProcessUpdate::Exited { .. } => {
                    process_active.store(false, Ordering::Relaxed);
                    let now = Instant::now();
                    let pending = display
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .flush(now, true);
                    for event in pending {
                        emit_display(event);
                    }
                    let mut tracker = runtime.lock().unwrap_or_else(|p| p.into_inner());
                    tracker.snapshot.phase = "stopped".into();
                    events.runtime(stage, engine, tracker.refresh(now));
                }
                ProcessUpdate::Started { process_id } => {
                    if engine == PipelineEngine::Colmap {
                        mode.invalidate_colmap_artifacts(&artifact_recorder);
                    }
                    let mut tracker = runtime
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    *tracker = RuntimeTracker::new();
                    mapper_count.store(0, Ordering::Relaxed);
                    *mapper_estimate.lock().unwrap_or_else(|p| p.into_inner()) =
                        MapperProgressEstimate::default();
                    matching_notice_elapsed.store(0, Ordering::Relaxed);
                    *display.lock().unwrap_or_else(|p| p.into_inner()) =
                        EngineDisplayGate::default();
                    *quiet_visible_at.lock().unwrap_or_else(|p| p.into_inner()) = None;
                    process_active.store(true, Ordering::Relaxed);
                    tracker.snapshot.process_id = process_id;
                    if mode == ObserverMode::Brush {
                        if let Some(metrics) = &effectiveness {
                            metrics.brush_started();
                        }
                    }
                    if mode != ObserverMode::Brush {
                        tracker.snapshot.phase = "running".into();
                    }
                    let baseline = feature
                        .as_ref()
                        .and_then(|feature| feature.begin(events.bound_operation.as_ref()?.id));
                    let baseline_progress = baseline
                        .map(|(current, total)| (current as f32 / total as f32).min(0.999))
                        .or_else(|| {
                            (mode == ObserverMode::OrdinaryMapper
                                && expected_total.is_some_and(|total| total > 0))
                            .then_some(0.0)
                        });
                    events.send(
                        stage,
                        Some(engine),
                        EventKind::Stage,
                        EventLevel::Info,
                        baseline_progress,
                        baseline_progress.is_none(),
                        format!("进程已启动 · PID {process_id}"),
                        baseline.map(|(current, _)| current),
                        if feature.is_some() {
                            baseline.map(|(_, total)| total)
                        } else {
                            expected_total
                        },
                        baseline.map(|_| "images"),
                    );
                    if let Some((current, total)) = baseline {
                        let ready = display.lock().unwrap_or_else(|p| p.into_inner()).submit(
                            EngineDisplay {
                                kind: EventKind::Progress,
                                level: EventLevel::Info,
                                progress: Some((current as f32 / total as f32).min(0.999)),
                                message: format!("正在分析画面（{current}/{total}）"),
                                current: Some(current),
                                total: Some(total),
                                unit: Some("images"),
                                deduplicate_count: true,
                            },
                            Instant::now(),
                            true,
                        );
                        for event in ready {
                            emit_display(event);
                        }
                    }
                    runtime_signature
                        .store(runtime_line_signature(&tracker.snapshot), Ordering::Relaxed);
                    events.runtime(stage, engine, tracker.refresh(Instant::now()));
                }
                ProcessUpdate::Resources(sample) => {
                    let mut tracker = runtime
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if tracker.snapshot.process_id != sample.process_id {
                        return;
                    }
                    tracker.snapshot.resources = Some(sample);
                    runtime_signature
                        .store(runtime_line_signature(&tracker.snapshot), Ordering::Relaxed);
                    events.runtime(stage, engine, tracker.refresh(Instant::now()));
                }
                ProcessUpdate::Heartbeat { elapsed_ms } => {
                    let mut tracker = runtime
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let snapshot = tracker.refresh(Instant::now());
                    let raw_quiet_ms = snapshot.last_output_age_ms;
                    runtime_signature.store(runtime_line_signature(&snapshot), Ordering::Relaxed);
                    events.runtime(stage, engine, snapshot);
                    drop(tracker);
                    let now = Instant::now();
                    let pending = display
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .flush(now, false);
                    for event in pending {
                        emit_display(event);
                    }
                    let quiet_ms = if mode == ObserverMode::Matching {
                        raw_quiet_ms
                    } else {
                        let visible = display.lock().unwrap_or_else(|p| p.into_inner()).last_sent;
                        let mut previous =
                            quiet_visible_at.lock().unwrap_or_else(|p| p.into_inner());
                        if *previous != visible {
                            matching_notice_elapsed.store(0, Ordering::Relaxed);
                            *previous = visible;
                        }
                        visible.map_or(elapsed_ms, |at| now.duration_since(at).as_millis() as u64)
                    };
                    if process_active.load(Ordering::Relaxed)
                        && !cancellation.is_cancelled()
                        && matching_quiet_notice_due(&matching_notice_elapsed, elapsed_ms, quiet_ms)
                    {
                        let message = if mode == ObserverMode::Matching {
                            format!(
                            "图像匹配进程仍在运行，最近 {} 秒无新输出。此步骤可能耗时较长，请耐心等待",
                            quiet_ms / 1000
                        )
                        } else {
                            format!(
                                "{}。此步骤可能耗时较长，请耐心等待",
                                engine_quiet_step(stage, mode)
                            )
                        };
                        events.send(
                            stage,
                            Some(PipelineEngine::System),
                            EventKind::Log,
                            EventLevel::Info,
                            None,
                            true,
                            message,
                            None,
                            None,
                            None,
                        );
                    }
                }
                ProcessUpdate::Line { stream: _, line } => {
                    if line.is_empty() {
                        return;
                    }
                    if mode == ObserverMode::Matching {
                        matching_notice_elapsed.store(0, Ordering::Relaxed);
                    }
                    let brush = mode == ObserverMode::Brush;
                    let actual = {
                        let mut tracker = runtime
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        let actual = tracker.line(&line, Instant::now(), brush);
                        if brush {
                            if let Some(acceleration) = acceleration
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .as_ref()
                            {
                                tracker.enrich_device(acceleration);
                            }
                        }
                        if brush {
                            let signature = runtime_line_signature(&tracker.snapshot);
                            if runtime_signature.swap(signature, Ordering::Relaxed) != signature {
                                events.runtime(stage, engine, tracker.refresh(Instant::now()));
                            }
                        }
                        actual
                    };
                    let level = engine_line_level(&line);
                    if level != EventLevel::Info {
                        let ready = display.lock().unwrap_or_else(|p| p.into_inner()).submit(
                            EngineDisplay {
                                kind: EventKind::Log,
                                level,
                                progress: None,
                                message: friendly_engine_line(&line),
                                current: None,
                                total: expected_total,
                                unit: None,
                                deduplicate_count: false,
                            },
                            Instant::now(),
                            true,
                        );
                        for event in ready {
                            emit_display(event);
                        }
                        return;
                    }
                    let parsed = match mode {
                        ObserverMode::Generic => None,
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
                        ObserverMode::Matching => {
                            parse_matching_progress(&line).map(|(current, total)| {
                                (current, Some(total), friendly_engine_line(&line))
                            })
                        }
                        ObserverMode::TrainingPreparation => parse_undistortion_progress(&line)
                            .map(|(current, total)| {
                                (
                                    current,
                                    Some(total),
                                    format!("正在准备训练图片 {current}/{total}"),
                                )
                            }),
                        ObserverMode::Mapper | ObserverMode::OrdinaryMapper => {
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
                    let native_boundary = parsed.as_ref().is_some_and(|(current, total, _)| {
                        *current <= 1 || *total == Some(*current)
                    });
                    let parsed = if let Some(feature) = &feature {
                        parsed.and_then(|(current, total, _)| {
                            feature
                                .report(events.bound_operation.as_ref()?.id, current, total?)
                                .map(|(current, total)| {
                                    (
                                        current,
                                        Some(total),
                                        format!("正在分析画面（{current}/{total}）"),
                                    )
                                })
                        })
                    } else {
                        parsed
                    };
                    if let Some((current, total, message)) = parsed {
                        let progress = if mode == ObserverMode::OrdinaryMapper {
                            mapper_estimate
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .observe(&line, current, total)
                        } else {
                            total
                                .filter(|value| *value > 0)
                                .filter(|_| {
                                    !matches!(
                                        mode,
                                        ObserverMode::Matching
                                            | ObserverMode::Mapper
                                            | ObserverMode::TrainingPreparation
                                    )
                                })
                                .map(|value| current as f32 / value as f32)
                        };
                        let mapper_activity = mode.is_mapper()
                            && (line
                                .to_ascii_lowercase()
                                .contains("global bundle adjustment")
                                || line.to_ascii_lowercase().contains("retriangulation"));
                        let ready = display.lock().unwrap_or_else(|p| p.into_inner()).submit(
                            EngineDisplay {
                                kind: EventKind::Progress,
                                level: EventLevel::Info,
                                progress: progress.map(|value| {
                                    if brush {
                                        value.min(0.99)
                                    } else {
                                        value.min(0.999)
                                    }
                                }),
                                message,
                                current: Some(current),
                                total,
                                unit: Some(if brush {
                                    "steps"
                                } else if mode == ObserverMode::Matching {
                                    matching_unit(&line)
                                } else if mode.is_mapper() {
                                    "registeredImages"
                                } else {
                                    "images"
                                }),
                                deduplicate_count: !mapper_activity,
                            },
                            Instant::now(),
                            !mapper_activity && native_boundary,
                        );
                        for event in ready {
                            emit_display(event);
                        }
                    } else if let Some(activity) = (mode == ObserverMode::Matching)
                        .then(|| matching_activity(&line))
                        .flatten()
                    {
                        let ready = display.lock().unwrap_or_else(|p| p.into_inner()).submit(
                            EngineDisplay {
                                kind: EventKind::Progress,
                                level,
                                progress: None,
                                message: friendly_engine_line(&line),
                                current: None,
                                total: None,
                                unit: Some(activity),
                                deduplicate_count: false,
                            },
                            Instant::now(),
                            false,
                        );
                        for event in ready {
                            emit_display(event);
                        }
                    } else if mode == ObserverMode::Brush || is_useful_line(&line) {
                        let ready = display.lock().unwrap_or_else(|p| p.into_inner()).submit(
                            EngineDisplay {
                                kind: EventKind::Log,
                                level,
                                progress: None,
                                message: friendly_engine_line(&line),
                                current: None,
                                total: expected_total,
                                unit: None,
                                deduplicate_count: false,
                            },
                            Instant::now(),
                            engine_final_line(&line),
                        );
                        for event in ready {
                            emit_display(event);
                        }
                    }
                }
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObserverMode {
    Generic,
    Ffmpeg,
    BracketProgress,
    Matching,
    TrainingPreparation,
    Mapper,
    /// Only callers that have established the whole reconstruction has no ERP
    /// cameras may opt in. The default Mapper remains count-only/unknown.
    OrdinaryMapper,
    Brush,
}

impl ObserverMode {
    fn invalidate_colmap_artifacts(self, recorder: &super::artifacts::ArtifactRecorder) {
        match self.colmap_write() {
            Some(write) => recorder.invalidate_colmap(write),
            // Unknown commands cannot promise that upstream inputs are read-only.
            None => recorder.invalidate(),
        }
    }

    fn colmap_write(self) -> Option<super::artifacts::ColmapWrite> {
        use super::artifacts::ColmapWrite;
        match self {
            Self::BracketProgress | Self::Matching => Some(ColmapWrite::Database),
            Self::Mapper | Self::OrdinaryMapper => Some(ColmapWrite::Reconstruction),
            Self::TrainingPreparation => Some(ColmapWrite::TrainingDataset),
            _ => None,
        }
    }

    fn is_mapper(self) -> bool {
        matches!(self, Self::Mapper | Self::OrdinaryMapper)
    }
    fn mapper_for_projection(has_erp: bool) -> Self {
        if has_erp {
            Self::Mapper
        } else {
            Self::OrdinaryMapper
        }
    }
}

/// A conservative work estimate, not a registration percentage or an elapsed
/// time prediction. Reserve the final 10% for BA/model validation. Never sum
/// disconnected submodels, and reset this state on every real process start.
#[derive(Default)]
struct MapperProgressEstimate {
    best_single_model: Option<u64>,
}

impl MapperProgressEstimate {
    fn observe(&mut self, line: &str, current: u64, total: Option<u64>) -> Option<f32> {
        let total = total.filter(|total| *total > 0)?;
        if current > total {
            return None;
        }
        let lower = line.to_ascii_lowercase();
        let ba = lower.contains("retriangulation") || lower.contains("global bundle adjustment");
        let reported = line.contains("num_reg_frames=") || line.contains("num_reg_frames =");
        if reported && !ba && current > 0 {
            self.best_single_model = Some(self.best_single_model.unwrap_or(0).max(current));
        }
        // A known-input process can expose its zero estimate before the first
        // registration report. This is not a fabricated registered-image count.
        Some((self.best_single_model.unwrap_or(0) as f32 / total as f32) * 0.90)
    }
}

struct EngineDisplay {
    kind: EventKind,
    level: EventLevel,
    progress: Option<f32>,
    message: String,
    current: Option<u64>,
    total: Option<u64>,
    unit: Option<&'static str>,
    deduplicate_count: bool,
}

impl EngineDisplay {
    fn counter(&self) -> Option<(u64, Option<u64>, Option<&'static str>)> {
        self.deduplicate_count
            .then_some((self.current?, self.total, self.unit))
    }

    fn emit(self, events: &EventSink, stage: PipelineStage, engine: PipelineEngine) {
        events.send(
            stage,
            Some(engine),
            self.kind,
            self.level,
            self.progress,
            self.progress.is_none(),
            self.message,
            self.current,
            self.total,
            self.unit,
        );
    }
}

/// Keep at most one delayed count and one informational line. Errors and final
/// milestones are never throttled; disk logs remain complete and unfiltered.
#[derive(Default)]
struct EngineDisplayGate {
    last_sent: Option<Instant>,
    last_counter: Option<(u64, Option<u64>, Option<&'static str>)>,
    last_counter_progress: Option<f32>,
    last_activity: Option<&'static str>,
    pending_progress: Option<EngineDisplay>,
    pending_log: Option<EngineDisplay>,
}

impl EngineDisplayGate {
    fn due(&self, now: Instant) -> bool {
        self.last_sent
            .is_none_or(|at| now.duration_since(at) >= std::time::Duration::from_secs(1))
    }

    fn sent(&mut self, event: &EngineDisplay, now: Instant) {
        self.last_sent = Some(now);
        if let Some(counter) = event.counter() {
            self.last_counter = Some(counter);
            self.last_counter_progress = event.progress;
        }
        if event.kind == EventKind::Progress && event.current.is_none() {
            self.last_activity = event.unit;
        }
    }

    fn submit(&mut self, event: EngineDisplay, now: Instant, force: bool) -> Vec<EngineDisplay> {
        if event.counter().is_some_and(|counter| {
            self.last_counter == Some(counter) && self.last_counter_progress == event.progress
        }) {
            return vec![];
        }
        let activity_changed = event.kind == EventKind::Progress
            && event.current.is_none()
            && event.unit.is_some()
            && event.unit != self.last_activity;
        let first_counter = event.counter().is_some_and(|counter| {
            self.last_counter
                .is_none_or(|previous| previous.2 != counter.2)
        });
        if force || activity_changed || first_counter || event.level != EventLevel::Info {
            let mut ready = self.pending_progress.take().into_iter().collect::<Vec<_>>();
            self.pending_log = None;
            // A newer count supersedes, rather than duplicates, a pending count.
            if event.current.is_some() {
                ready.clear();
            }
            ready.push(event);
            for event in &ready {
                self.sent(event, now);
            }
            return ready;
        }
        if event.kind == EventKind::Progress {
            self.pending_log = None;
            if self.due(now) {
                self.pending_progress = None;
                self.sent(&event, now);
                vec![event]
            } else {
                self.pending_progress = Some(event);
                vec![]
            }
        } else if self.due(now) {
            if let Some(progress) = self.pending_progress.take() {
                self.pending_log = Some(event);
                self.sent(&progress, now);
                vec![progress]
            } else {
                self.pending_log = None;
                self.sent(&event, now);
                vec![event]
            }
        } else {
            self.pending_log = Some(event);
            vec![]
        }
    }

    fn flush(&mut self, now: Instant, force: bool) -> Vec<EngineDisplay> {
        if !force && !self.due(now) {
            return vec![];
        }
        let mut ready = self.pending_progress.take().into_iter().collect::<Vec<_>>();
        if force || ready.is_empty() {
            ready.extend(self.pending_log.take());
        }
        for event in &ready {
            self.sent(event, now);
        }
        ready
    }
}

fn runtime_line_signature(snapshot: &RuntimeSnapshot) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    (
        &snapshot.phase,
        &snapshot.device,
        &snapshot.backend,
        &snapshot.config,
    )
        .hash(&mut hash);
    snapshot
        .training
        .as_ref()
        .map(|training| {
            (
                training.iteration,
                training.total,
                training.start_iter,
                training.lod,
                training.splat_count,
                training.psnr.map(f64::to_bits),
                training.ssim.map(f64::to_bits),
            )
        })
        .hash(&mut hash);
    hash.finish()
}

fn engine_line_level(line: &str) -> EventLevel {
    let trimmed = line.trim();
    let lower = trimmed.to_ascii_lowercase();
    // A level token belongs to the logger header, not to metric prose such
    // as "Mean reprojection error 0.12" in an otherwise normal BA message.
    let bracketed = lower
        .strip_prefix('[')
        .and_then(|rest| rest.split_once(']'));
    let mut fields = bracketed
        .map_or(lower.as_str(), |(header, _)| header)
        .split_whitespace();
    let first = fields.next().unwrap_or("");
    let timestamp = first
        .get(..4)
        .is_some_and(|year| year.bytes().all(|byte| byte.is_ascii_digit()))
        && first.contains('-')
        && first.contains('t');
    let structured_level = if bracketed.is_some() && ["error", "warn", "warning"].contains(&first) {
        Some(first)
    } else if timestamp {
        fields.next()
    } else {
        None
    };
    let glog = lower
        .split_whitespace()
        .next()
        .filter(|prefix| {
            matches!(prefix.len(), 5 | 9) && prefix.as_bytes()[1..].iter().all(u8::is_ascii_digit)
        })
        .and_then(|prefix| prefix.chars().next());
    if matches!(glog, Some('e' | 'f'))
        || structured_level == Some("error")
        || trimmed.starts_with("ERROR ")
        || lower.starts_with("error:")
        || lower.starts_with("fatal ")
        || [
            "check failed:",
            "fatal error",
            "fatal:",
            "panicked at",
            "wgpu error:",
            "out of memory",
            "buffertoobig",
            "failed to create any sparse model",
            "failed to read",
            "cannot open database",
            "export failed after",
        ]
        .iter()
        .any(|marker| lower.contains(marker))
    {
        EventLevel::Error
    } else if glog == Some('w')
        || matches!(structured_level, Some("warn" | "warning"))
        || trimmed.starts_with("WARN ")
        || lower.starts_with("warning:")
        || lower.starts_with("warn:")
        || ["deprecated pixel format"]
            .iter()
            .any(|marker| lower.contains(marker))
    {
        EventLevel::Warning
    } else {
        EventLevel::Info
    }
}

fn engine_final_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    [
        "training loop completed",
        "done training!",
        "export succeeded",
        "export failed after",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn engine_quiet_step(stage: PipelineStage, mode: ObserverMode) -> &'static str {
    if mode == ObserverMode::TrainingPreparation {
        return "正在准备训练图片";
    }
    match stage {
        PipelineStage::ExtractingFrames => "画面准备",
        PipelineStage::ExtractingFeatures => "特征提取",
        PipelineStage::Matching => "图像匹配",
        PipelineStage::Reconstructing | PipelineStage::ValidatingReconstruction => "相机重建",
        PipelineStage::TrainingSplats => "Splat 训练",
        PipelineStage::Exporting => "结果导出",
        _ => "正在处理",
    }
}

fn parse_undistortion_progress(line: &str) -> Option<(u64, u64)> {
    let (_, suffix) = line.split_once("Undistorting image [")?;
    let (counter, _) = suffix.split_once(']')?;
    let (current, total) = counter.split_once('/')?;
    let current = current.parse::<u64>().ok()?;
    let total = total.parse::<u64>().ok()?;
    (total > 0 && current > 0 && current <= total).then_some((current, total))
}

fn matching_quiet_notice_due(last_notice: &AtomicU64, elapsed_ms: u64, quiet_ms: u64) -> bool {
    if quiet_ms < 15_000 || elapsed_ms == 0 {
        return false;
    }
    let previous = last_notice.load(Ordering::Relaxed);
    (previous == 0 || elapsed_ms.saturating_sub(previous) >= 30_000)
        && last_notice
            .compare_exchange(previous, elapsed_ms, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
}

fn parse_ffmpeg_frame(line: &str) -> Option<u64> {
    line.strip_prefix("frame=")?.trim().parse().ok()
}

fn parse_bracket_progress(line: &str) -> Option<(u64, u64)> {
    ["Processed file [", "Processed image ["]
        .iter()
        .find_map(|marker| parse_counter_after(line, marker))
}

fn parse_matching_progress(line: &str) -> Option<(u64, u64)> {
    [
        "Matching block [",
        "Matching image [",
        "Processing image [",
        "Indexing image [",
    ]
    .iter()
    .find_map(|marker| parse_counter_after(line, marker))
}

fn matching_unit(line: &str) -> &'static str {
    if line.contains("Indexing image [") {
        "matchingIndexImages"
    } else if line.contains("Processing image [") || line.contains("Matching image [") {
        "matchingImages"
    } else {
        "matchingBatches"
    }
}

fn matching_activity(line: &str) -> Option<&'static str> {
    let lower = line.to_ascii_lowercase();
    if lower.contains("skip")
        && (lower.contains("loop detection") || lower.contains("loop closure"))
    {
        return None;
    }
    if lower.contains("vocabulary tree")
        || lower.contains("loop detection")
        || lower.contains("loop closure")
    {
        Some("matchingLoop")
    } else if lower.contains("generating sequential image pairs") {
        Some("matchingSequential")
    } else if lower.contains("matching local") || lower.contains("importing image pairs") {
        Some("matchingLocal")
    } else {
        None
    }
}

fn parse_counter_after(line: &str, marker: &str) -> Option<(u64, u64)> {
    let value = line.split_once(marker)?.1.split_once(']')?.0;
    let (current, total) = value.split_once('/')?;
    let current = current.trim().parse().ok()?;
    let total = total.trim().parse().ok()?;
    (total > 0 && current <= total).then_some((current, total))
}

fn parse_mapper_progress(
    line: &str,
    counter: &AtomicU64,
    expected_total: Option<u64>,
) -> Option<(u64, Option<u64>, String)> {
    let expected_total = expected_total.filter(|total| *total > 0);
    if line.contains("Registering initial image pair")
        || line.contains("Initializing with image pair")
    {
        counter.store(0, Ordering::Relaxed);
        return Some((0, expected_total, friendly_engine_line(line)));
    }
    let reported_count = value_after(line, "num_reg_frames=")
        .or_else(|| value_after(line, "num_reg_frames ="))
        .and_then(|value| value.parse::<u64>().ok());
    let has_reported_count = line.contains("num_reg_frames=") || line.contains("num_reg_frames =");
    if has_reported_count && reported_count.is_none() {
        return None;
    }
    if reported_count.is_some_and(|current| expected_total.is_some_and(|total| current > total)) {
        return None;
    }
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
        counter.store(current, Ordering::Relaxed);
        return Some((current, expected_total, format!("已注册 {current} 张图像")));
    }
    None
}

fn value_after<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    let value = line.split_once(marker)?.1;
    let value = value
        .split_whitespace()
        .next()?
        .trim_end_matches([')', ']', ',', ';']);
    (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())).then_some(value)
}

fn is_useful_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    [
        "bundle",
        "register",
        "triang",
        "elapsed",
        "warning",
        "error",
        "failed",
        "cannot",
        "unable to",
        "writing",
        "loading",
        "matching",
        "loop detection",
        "aligning",
        "transforming",
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
    ((quality == Quality::High || resolved.panorama.is_some()) && planner_enabled && !retry_used)
        .then(|| resolved.downgrade_after_oom(initial_sfm_points))
        .flatten()
}

fn track_process_start(observer: ProcessObserver) -> (ProcessObserver, Arc<AtomicBool>) {
    let started = Arc::new(AtomicBool::new(false));
    let flag = started.clone();
    (
        Arc::new(move |update| {
            if matches!(update, ProcessUpdate::Started { .. }) {
                flag.store(true, Ordering::Relaxed);
            }
            observer(update);
        }),
        started,
    )
}

fn should_allow_two_view_tracks(
    planner_enabled: bool,
    preset_allows_two_view_tracks: bool,
    has_transparency_mask: bool,
) -> bool {
    planner_enabled && preset_allows_two_view_tracks && has_transparency_mask
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
    crate::presets::spawn_pipeline_blocking(move || {
        Ok::<u64, SplatError>(crate::video::list_images(&directory)?.len() as u64)
    })
    .await
    .map_err(|error| SplatError::Process(format!("无法统计输入画面：{error}")))?
}

fn validate_legacy_v1_preparation_policy(version: Option<u32>) -> Result<()> {
    // V1 has no per-source preparation manifest. It can preserve its saved
    // legacy pixels, but cannot claim that copied historical inputs follow a
    // newer derivative policy. Modern reshoots use the shared input executor.
    if version.is_some() {
        return Err(SplatError::Process(
            legacy_work_order::UNRESTORABLE_RECIPE.into(),
        ));
    }
    Ok(())
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
    let (left, right) = crate::presets::spawn_pipeline_blocking(move || {
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
    extracted_colmap_frames: &Path,
    extracted_masks: &Path,
    frames: &Path,
    colmap_frames: &Path,
    masks: &Path,
    has_alpha: bool,
) -> Result<()> {
    tokio::fs::create_dir_all(frames).await?;
    tokio::fs::create_dir_all(colmap_frames).await?;
    if has_alpha {
        tokio::fs::create_dir_all(masks).await?;
    }
    let mut pending = Vec::new();
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
        pending.push((source.clone(), frames.join(&new_name)));
        pending.push((
            extracted_colmap_frames.join(old_name),
            colmap_frames.join(&new_name),
        ));
        if has_alpha {
            pending.push((
                extracted_masks.join(format!("{old_name}.png")),
                masks.join(format!("{new_name}.png")),
            ));
        }
    }
    for (source, destination) in &pending {
        if !source.is_file() || destination.exists() {
            return Err(SplatError::Process(format!(
                "补拍三路图片无法安全合并：{} -> {}",
                source.display(),
                destination.display()
            )));
        }
    }
    let mut moved = Vec::new();
    for (source, destination) in pending {
        if let Err(error) = tokio::fs::rename(&source, &destination).await {
            for (original, target) in moved.into_iter().rev() {
                let _ = tokio::fs::rename(target, original).await;
            }
            return Err(error.into());
        }
        moved.push((source, destination));
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
    crate::presets::spawn_pipeline_blocking(move || {
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

/// Temporary #73 hook. Never archive the (potentially hard-linked) pictures,
/// masks, final.ply or edits. Persist the reuse barrier BEFORE moving caches.
async fn apply_temporary_png_focal_checkpoint(
    manager: &ProjectManager,
    paths: &ProjectPaths,
    state: &mut PipelineStateFile,
    checkpoint: &crate::video::temporary_png_focal_workaround::Checkpoint,
) -> Result<bool> {
    if state.temporary_png_focal.as_ref() == Some(checkpoint) {
        return Ok(false);
    }
    let mut caches = Vec::new();
    for (path, name) in [(&paths.colmap, "colmap"), (&paths.brush, "brush")] {
        if path.exists() {
            if tokio::fs::symlink_metadata(path)
                .await?
                .file_type()
                .is_symlink()
            {
                return Err(SplatError::InvalidPath(path.clone()));
            }
            if tokio::fs::read_dir(path)
                .await?
                .next_entry()
                .await?
                .is_some()
            {
                caches.push((path.clone(), name));
            }
        }
    }
    let invalidated = !caches.is_empty()
        || state.features_complete
        || state.matching_complete
        || state.reconstruction_complete
        || state.brush_complete;
    state.features_complete = false;
    state.matching_complete = false;
    state.reconstruction_complete = false;
    state.brush_complete = false;
    state.validated_output = None;
    state.stage = PipelineStage::ExtractingFrames;
    state.temporary_png_focal = None;
    manager.write_state(&paths.state, state).await?;
    if !caches.is_empty() {
        let archive = paths
            .work
            .join("cache-archive")
            .join(format!("temporary-png-focal-v1-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&archive).await?;
        let mut moved = Vec::new();
        for (source, name) in caches {
            let destination = archive.join(name);
            if let Err(error) = tokio::fs::rename(&source, &destination).await {
                for (original, saved) in moved.into_iter().rev() {
                    // State stays non-reusable even if rollback itself fails.
                    let _ = tokio::fs::rename(saved, original).await;
                }
                return Err(SplatError::Process(format!(
                    "Cannot archive temporary PNG focal cache: {error}"
                )));
            }
            moved.push((source, destination));
        }
    }
    state.temporary_png_focal = Some(checkpoint.clone());
    manager.write_state(&paths.state, state).await?;
    Ok(invalidated)
}

async fn archive_incompatible_colmap_input_cache(
    paths: &ProjectPaths,
    state: &PipelineStateFile,
) -> Result<()> {
    let frame_cache_incompatible = state.frames.as_ref().is_some_and(|frames| {
        frames.extracted_frames.is_some()
            && frames.colmap_input_version != Some(COLMAP_INPUT_VERSION)
    });
    let reshoot_cache_incompatible = state.reshoot.as_ref().is_some_and(|reshoot| {
        (reshoot.source_snapshot_complete
            || reshoot.supplemental_frames_complete
            || reshoot.supplemental_features_complete
            || reshoot.incremental_matching_complete
            || reshoot.incremental_reconstruction_complete)
            && reshoot.colmap_input_version != Some(COLMAP_INPUT_VERSION)
    });
    if !frame_cache_incompatible && !reshoot_cache_incompatible {
        return Ok(());
    }

    let archive_root = paths
        .work
        .join("cache-archive")
        .join(format!("colmap-input-vunknown-{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&archive_root).await?;
    let candidates = vec![
        (paths.frames.clone(), "frames"),
        (paths.colmap_frames.clone(), "colmap_frames"),
        (paths.masks.clone(), "masks"),
        (paths.colmap.clone(), "colmap"),
        (paths.brush.clone(), "brush"),
        (paths.work.join("reshoot-masks"), "reshoot-masks"),
    ];
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (source, name) in candidates {
        if !source.exists() {
            continue;
        }
        let destination = archive_root.join(name);
        if let Err(error) = tokio::fs::rename(&source, &destination).await {
            for (original, archived) in moved.into_iter().rev() {
                let _ = tokio::fs::rename(archived, original).await;
            }
            let _ = tokio::fs::remove_dir_all(&archive_root).await;
            return Err(SplatError::Process(format!(
                "无法保存旧版 COLMAP 缓存 {}：{error}",
                source.display()
            )));
        }
        moved.push((source, destination));
    }
    Ok(())
}

pub(super) fn ordinary_sfm_preparation_policies(
    project: &Path,
    state: &PipelineStateFile,
) -> Result<std::collections::BTreeMap<String, Option<u32>>> {
    inputs_runner::source_preparation_policies(project, state)
}

async fn validate_current_colmap_input_snapshot(
    project: &Path,
    state: &PipelineStateFile,
) -> Result<PreparedImageSequence> {
    let frame_state = state
        .frames
        .as_ref()
        .ok_or_else(|| SplatError::Process("项目缺少已准备图片的检查点记录".into()))?;
    if frame_state.colmap_input_version != Some(COLMAP_INPUT_VERSION) {
        return Err(SplatError::Process(
            "项目使用旧版或未知的 COLMAP 图片处理语义".into(),
        ));
    }
    let expected_count = frame_state
        .extracted_frames
        .filter(|count| *count > 0)
        .ok_or_else(|| SplatError::Process("项目缺少已准备图片数量".into()))?;
    // A completed V2 reshoot contains both the original inputs and all previous
    // reshoot batches. The preparation prefix only describes the first batch;
    // the saved recipe is the authority for the complete frozen image set.
    let expected_names = state
        .reshoot
        .as_ref()
        .and_then(|reshoot| reshoot.recipe.as_ref().map(|recipe| (reshoot, recipe)))
        .map(|(reshoot, recipe)| {
            if !reshoot.source_snapshot_complete
                || !reshoot.supplemental_frames_complete
                || reshoot.colmap_input_version != Some(COLMAP_INPUT_VERSION)
                || recipe.source_names.len() as u64 != reshoot.source_image_count
                || recipe.new_names.len() as u64 != reshoot.reshoot_frame_count
                || reshoot
                    .source_image_count
                    .checked_add(reshoot.reshoot_frame_count)
                    != Some(expected_count)
            {
                return Err(SplatError::Process(
                    "原项目的输入图片记录与已准备图片数量不一致".into(),
                ));
            }
            Ok(recipe
                .source_names
                .iter()
                .chain(&recipe.new_names)
                .cloned()
                .collect::<Vec<_>>())
        })
        .transpose()?;
    let frames = project.join("work/frames");
    let colmap_frames = project.join("work/colmap_frames");
    let masks = project.join("work/masks");
    let has_alpha = frame_state.has_alpha;
    let policies = ordinary_sfm_preparation_policies(project, state)?;
    let versioned = state.inputs.is_some() || policies.values().any(Option::is_some);
    if versioned && policies.len() as u64 != expected_count {
        return Err(SplatError::Process(
            "原项目的输入图片记录与已准备图片数量不一致".into(),
        ));
    }
    crate::presets::spawn_pipeline_blocking(move || {
        if versioned {
            if expected_names.as_ref().is_some_and(|names| {
                names.iter().collect::<std::collections::BTreeSet<_>>()
                    != policies.keys().collect::<std::collections::BTreeSet<_>>()
            }) {
                return Err(SplatError::Process(
                    "原项目的输入图片记录与已准备图片数量不一致".into(),
                ));
            }
            validate_versioned_colmap_image_snapshot(
                &frames,
                &colmap_frames,
                &masks,
                &policies,
                has_alpha,
                &tokio_util::sync::CancellationToken::new(),
            )
        } else if let Some(names) = expected_names {
            validate_prepared_image_names(&frames, &colmap_frames, &masks, &names, has_alpha)
        } else {
            validate_prepared_image_sequence(
                &frames,
                &colmap_frames,
                &masks,
                expected_count,
                has_alpha,
            )
        }
    })
    .await
    .map_err(|error| SplatError::Process(format!("校验 COLMAP 图片快照失败：{error}")))?
}

fn validate_versioned_colmap_image_snapshot(
    frames: &Path,
    colmap_frames: &Path,
    masks: &Path,
    policies: &std::collections::BTreeMap<String, Option<u32>>,
    require_masks: bool,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<crate::video::PreparedImageSequence> {
    let expected = policies
        .keys()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    if expected.is_empty()
        || expected
            .iter()
            .map(|name| name.to_ascii_lowercase())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != expected.len()
    {
        return Err(SplatError::Process(
            "Prepared image names are unsafe or duplicated".into(),
        ));
    }
    for directory in [frames, colmap_frames] {
        let actual = crate::video::list_images(directory)?
            .into_iter()
            .map(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned)
                    .ok_or_else(|| SplatError::InvalidPath(path.clone()))
            })
            .collect::<Result<std::collections::BTreeSet<_>>>()?;
        if actual != expected {
            return Err(SplatError::Process(
                "Prepared image set does not match the saved image names".into(),
            ));
        }
    }
    let mut mask_count = 0;
    let mut has_alpha = false;
    for (name, policy) in policies {
        if cancel.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        let frame = crate::reconstruction::direct_input::image_path(frames, name)?;
        let counterpart = crate::reconstruction::direct_input::image_path(colmap_frames, name)?;
        let mask = masks.join(format!("{name}.png"));
        let header = crate::video::image_projection::inspect(&frame)?;
        let mask_exists = match std::fs::symlink_metadata(&mask) {
            Ok(metadata) if metadata.file_type().is_file() => true,
            Ok(_) => return Err(SplatError::InvalidPath(mask)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        has_alpha |= header.alpha;
        match policy {
            Some(crate::pipeline::inputs::ORDINARY_IMAGE_PREPARATION_VERSION) => {
                crate::video::image_sequence::validate_ordinary_sfm_derivatives(
                    &frame,
                    &counterpart,
                    &mask,
                    cancel,
                )?;
            }
            None => {
                crate::video::image_sequence::validate_colmap_frame_copy(&frame, &counterpart)?;
                if require_masks || header.alpha || mask_exists {
                    let dimensions = if (5..=8).contains(&header.orientation) {
                        (header.height, header.width)
                    } else {
                        (header.width, header.height)
                    };
                    crate::video::image_sequence::validate_binary_mask(&mask, dimensions)?;
                }
            }
            Some(_) => {
                return Err(SplatError::Process(
                    "Unsupported source image preparation policy".into(),
                ))
            }
        }
        mask_count += u64::from(mask_exists);
    }
    Ok(crate::video::PreparedImageSequence {
        image_count: policies.len() as u64,
        mask_count,
        has_alpha,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReshootColmapInputMode {
    CurrentV2,
    LegacySharedFrames,
}

impl ReshootColmapInputMode {
    const fn identity_tag(self) -> &'static [u8] {
        match self {
            Self::CurrentV2 => b"current-v2",
            Self::LegacySharedFrames => b"legacy-shared-frames",
        }
    }
}

#[derive(Debug, Clone)]
struct ResolvedReshootColmapInput {
    colmap_images: PathBuf,
    mode: ReshootColmapInputMode,
    legacy_content_identity: Option<[u8; 32]>,
}

/// Resolve the exact image directory whose pixels own the reusable COLMAP
/// features. Current projects retain the strict two-directory snapshot. Older
/// projects may share `work/frames` only after their images, database and
/// sparse model have been proven to describe the same feature coordinates.
async fn resolve_reshoot_colmap_input(
    project: &Path,
    metadata: &ProjectMetadata,
    state: &PipelineStateFile,
    model: &Path,
    database: &Path,
) -> Result<ResolvedReshootColmapInput> {
    let frame_state = state
        .frames
        .as_ref()
        .ok_or_else(|| SplatError::Process("项目缺少已准备图片的检查点记录".into()))?;
    match frame_state.colmap_input_version {
        Some(COLMAP_INPUT_VERSION) => {
            // Multi-input projects have their own per-source name validation
            // in source_assets(). Older ordinary and Reshoot V2 projects use
            // the byte-identical snapshot validator with their saved names.
            if state.inputs.is_none() {
                validate_current_colmap_input_snapshot(project, state).await?;
            }
            Ok(ResolvedReshootColmapInput {
                colmap_images: project.join("work/colmap_frames"),
                mode: ReshootColmapInputMode::CurrentV2,
                legacy_content_identity: None,
            })
        }
        Some(_) => Err(SplatError::Process(
            "原项目使用旧版或未知的 COLMAP 图片处理语义".into(),
        )),
        None => {
            let frames = project.join("work/frames");
            let colmap_frames = project.join("work/colmap_frames");
            let masks = project.join("work/masks");
            let has_colmap_frames_entry = std::fs::symlink_metadata(&colmap_frames).is_ok();
            let has_masks = if masks.is_dir() {
                std::fs::read_dir(&masks)?.next().transpose()?.is_some()
            } else {
                std::fs::symlink_metadata(&masks).is_ok()
            };
            if metadata.app_id != crate::project::PROJECT_APP_ID
                || metadata.imported.is_some()
                || metadata.reshoot.is_some()
                || state.inputs.is_some()
                || state.panorama.is_some()
                || state.video_projection != crate::video::panorama::VideoProjection::Perspective
                || frame_state.has_alpha
                || frame_state.mask_count.unwrap_or(0) != 0
                || state.actual_transparency == Some(true)
                || has_colmap_frames_entry
                || has_masks
            {
                return Err(SplatError::Process(
                    "原项目的 COLMAP 专用图片不完整或与原始帧不一致，无法安全进行增量补拍".into(),
                ));
            }
            let expected = frame_state
                .extracted_frames
                .filter(|count| *count > 0)
                .ok_or_else(|| SplatError::Process("项目缺少已准备图片数量".into()))?;
            let output = metadata
                .output
                .as_ref()
                .ok_or_else(|| SplatError::Process("原项目缺少已发布结果的统计记录".into()))?;
            if output.input_images != expected {
                return Err(SplatError::Process(
                    "原项目的输入图片记录与已准备图片数量不一致".into(),
                ));
            }
            let frames_for_validation = frames.clone();
            let database_for_validation = database.to_owned();
            let model_for_validation = model.to_owned();
            let validation = crate::presets::spawn_pipeline_blocking(move || {
                crate::reconstruction::direct_input::validate_legacy_shared_frames(
                    &frames_for_validation,
                    &database_for_validation,
                    &model_for_validation,
                    expected,
                )
            })
            .await
            .map_err(|error| {
                SplatError::Process(format!("校验旧版 COLMAP 图片快照失败：{error}"))
            })??;
            if validation.image_count != expected {
                return Err(SplatError::Process(
                    "旧项目的图片校验数量与检查点记录不一致".into(),
                ));
            }
            if output.registered_images != validation.registered_images {
                return Err(SplatError::Process(
                    "原项目的注册图片记录与稀疏模型统计不一致".into(),
                ));
            }
            Ok(ResolvedReshootColmapInput {
                colmap_images: frames,
                mode: ReshootColmapInputMode::LegacySharedFrames,
                legacy_content_identity: Some(validation.content_identity),
            })
        }
    }
}

fn reusable_cache_validation<T>(result: Result<T>) -> Result<Option<T>> {
    crate::pipeline::artifacts::usable_validation(
        result.map(Some),
        &tokio_util::sync::CancellationToken::new(),
    )
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

    let database_complete = reusable_cache_validation(
        tokio::fs::metadata(paths.colmap.join("database.db"))
            .await
            .map_err(SplatError::from),
    )?
    .is_some_and(|metadata| metadata.is_file() && metadata.len() > 0);
    state.features_complete = frames_complete && state.features_complete && database_complete;
    state.matching_complete = state.features_complete && state.matching_complete;
    state.reconstruction_complete = state.matching_complete
        && state.reconstruction_complete
        && reusable_cache_validation(
            best_sparse_model(&paths.frames, &paths.colmap.join("sparse")).await,
        )?
        .is_some();
    if !state.reconstruction_complete {
        state.validated_output = None;
    }
    state.brush_complete = state.reconstruction_complete
        && state.brush_complete
        && match brush_candidate(&paths.brush) {
            Some(path) => reusable_cache_validation(inspect_gaussian_ply(&path))?.is_some(),
            None => false,
        };
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
    if frames.colmap_input_version != Some(COLMAP_INPUT_VERSION) {
        return Ok(None);
    }
    let Some(extracted_frames) = frames.extracted_frames.filter(|count| *count > 0) else {
        return Ok(None);
    };
    let plan = FramePlan {
        retention_ratio: frames.retention_ratio,
        sampling_fps: frames.sampling_fps,
        estimated_frames: frames.estimated_frames,
        selected_frames: frames.selected_frames.clone(),
        candidate_frames: frames.candidate_frames.clone(),
        bridge_source_frames: frames.bridge_source_frames,
        rescue_max_frames: frames.rescue_max_frames,
        minimum_frame_override_applied: frames.minimum_frame_override_applied,
    };
    match state.input_type {
        ProjectInputType::Video => {
            let Some(video) = state.video.clone() else {
                return Ok(None);
            };
            let has_alpha = frames.has_alpha || video.has_alpha;
            let Some(extraction) = reusable_cache_validation(
                validate_extraction(&paths.frames, &paths.colmap_frames, &paths.masks, has_alpha)
                    .await,
            )?
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
            let colmap_frames_dir = paths.colmap_frames.clone();
            let masks_dir = paths.masks.clone();
            let has_alpha = frames.has_alpha;
            let ordinary_preparation = (state.panorama.is_none()
                && state.video_projection == crate::video::panorama::VideoProjection::Perspective)
                .then_some(state.ordinary_image_preparation_version)
                .flatten();
            let prepared = crate::presets::spawn_pipeline_blocking(move || {
                validate_image_sequence_with_preparation(
                    &frames_dir,
                    &colmap_frames_dir,
                    &masks_dir,
                    extracted_frames,
                    has_alpha,
                    ImageSequenceNaming::Primary,
                    None,
                    ordinary_preparation,
                )
                .map(|prepared| (prepared, image_sequence))
            })
            .await
            .map_err(|error| SplatError::Process(format!("图片检查点校验失败：{error}")))?;
            let Some(prepared) = reusable_cache_validation(prepared)? else {
                return Ok(None);
            };
            let (prepared, image_sequence) = prepared;
            if frames
                .mask_count
                .is_some_and(|count| count != prepared.mask_count)
            {
                return Ok(None);
            }
            let (working_width, working_height) = state
                .panorama
                .as_ref()
                .map(|p| (p.sfm_width, p.sfm_height))
                .unwrap_or((image_sequence.width, image_sequence.height));
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

// New v4 single-path callers must resolve the same ERP budget as ordered inputs.
// Earlier policies and already-saved resolved snapshots retain their old recipe.
#[cfg(test)]
fn initial_panorama_recipe(
    policy: Option<crate::pipeline::generation_policy::GenerationPolicy>,
    quality: Quality,
    planner_enabled: bool,
    memory: Option<u64>,
    face_size: u32,
    sparse_points: u64,
) -> Result<ResolvedBrushTrainingPreset> {
    if let Some(policy) = policy.filter(|p| p.version >= 4) {
        policy.panorama_training(quality, memory, face_size, sparse_points, sparse_points)
    } else {
        crate::presets::resolve_panorama_training_preset(
            quality,
            planner_enabled,
            memory,
            face_size,
            sparse_points,
        )
    }
}

fn brush_candidate(root: &Path) -> Option<PathBuf> {
    [root.join("final.ply.tmp"), root.join("final.ply.tmp.ply")]
        .into_iter()
        .find(|path| path.is_file())
}

fn reusable_brush_candidate(root: &Path) -> Option<PathBuf> {
    brush_candidate(root).filter(|path| inspect_gaussian_ply(path).is_ok())
}

fn work_order_candidate(paths: &ProjectPaths, state: &PipelineStateFile) -> Option<PathBuf> {
    let training_attempt = state
        .reshoot
        .as_ref()
        .and_then(|value| value.recipe.as_ref())
        .map_or(0, |recipe| recipe.training_fallbacks);
    if state.inputs.is_some() {
        reusable_brush_candidate(
            &paths
                .brush
                .join(format!("collection-training-{training_attempt}")),
        )
    } else if let Some(recipe) = state
        .reshoot
        .as_ref()
        .and_then(|value| value.recipe.as_ref())
    {
        reusable_brush_candidate(&paths.brush.join(format!(
            "attempt-{}-{}",
            recipe.reconstruction_fallbacks, recipe.training_fallbacks
        )))
    } else {
        reusable_brush_candidate(&paths.brush)
    }
}

async fn read_training_dataset_identity(dataset: &Path) -> Result<String> {
    let value: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(dataset.join("inputs-dataset.json")).await
            .map_err(SplatError::from)
            .map_err(|error| error.with_io_diagnostic(
                "training_preparation_operation=read_dataset_identity\npath_role=training_dataset_manifest"
            ))?)?;
    value
        .get(0)
        .and_then(serde_json::Value::as_str)
        .filter(|identity| !identity.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| SplatError::Process("Missing training dataset identity".into()))
}

/// Old successful training checkpoints predate `validated_output`. Derive
/// their publication facts from the normalized reconstruction and candidate,
/// without starting engines or rebuilding disposable training images.
async fn recovered_candidate_output(
    paths: &ProjectPaths,
    state: &PipelineStateFile,
    candidate: &Path,
) -> Result<ProjectOutput> {
    let root = if state
        .gps
        .as_ref()
        .is_some_and(|gps| gps.status == "complete")
    {
        paths.colmap.join("gps-v1/model")
    } else if state.inputs.is_some() {
        paths.colmap.join("authoritative")
    } else if state
        .reshoot
        .as_ref()
        .and_then(|value| value.recipe.as_ref())
        .is_some_and(|recipe| {
            recipe.actual.reconstruction == crate::pipeline::reshoot::ReconstructionMode::Full
        })
    {
        paths.colmap.join("sparse-full")
    } else {
        paths.colmap.join("sparse")
    };
    let (_, report) = best_sparse_model(&paths.frames, &root).await?;
    let candidate = candidate.to_path_buf();
    let ply = crate::presets::spawn_pipeline_blocking(move || inspect_gaussian_ply(&candidate))
        .await
        .map_err(|error| SplatError::Process(error.to_string()))??;
    Ok(ProjectOutput {
        final_ply: paths.project.join("final.ply"),
        file_size: ply.file_size,
        splat_count: ply.splat_count,
        input_images: report.input_images,
        registered_images: report.registered_images,
        registered_ratio: report.registered_ratio,
        points_3d: report.points_3d,
    })
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
    if crate::presets::spawn_pipeline_blocking(move || inspect_gaussian_ply(&inspect_path))
        .await
        .map_err(|error| SplatError::Process(format!("PLY 恢复校验任务失败：{error}")))?
        .is_err()
    {
        return Ok(());
    }
    tokio::fs::create_dir_all(&paths.brush).await?;
    let recovered = paths.brush.join("final.ply.tmp");
    reshoot_runner::copy_reshoot_file(
        &orphan,
        &recovered,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await?;
    inspect_gaussian_ply(&recovered)?;
    Ok(())
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
    crate::presets::spawn_pipeline_blocking(move || {
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

    let connection = crate::reconstruction::direct_input::open_immutable_database(database)
        .map_err(|error| SplatError::Process(format!("无法读取原项目 COLMAP 数据库：{error}")))?;
    let stored = connection
        .query_row(
            "SELECT model, width, height, params FROM cameras WHERE camera_id = ?1",
            [camera.id],
            |row| {
                Ok((
                    row.get::<_, i32>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            },
        )
        .optional()
        .map_err(|error| SplatError::Process(format!("校验原项目相机数据库失败：{error}")))?;
    let Some((model, width, height, params)) = stored else {
        return Err(SplatError::Process(format!(
            "原项目 COLMAP 数据库缺少相机 ID {}",
            camera.id
        )));
    };
    let (chunks, remainder) = params.as_chunks::<8>();
    let stored_params = chunks
        .iter()
        .map(|bytes| f64::from_le_bytes(*bytes))
        .collect::<Vec<_>>();
    if model != camera.model_id
        || width != camera.width as i64
        || height != camera.height as i64
        || !remainder.is_empty()
        || stored_params != camera.params
    {
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
    crate::presets::spawn_pipeline_blocking(move || best_sparse_model_blocking(&frames, &sparse))
        .await
        .map_err(|error| SplatError::Process(format!("稀疏模型校验任务失败：{error}")))?
}

async fn best_sparse_model_with_input_count(
    sparse: &Path,
    input_images: u64,
) -> Result<(PathBuf, ReconstructionReport)> {
    let sparse = sparse.to_path_buf();
    crate::presets::spawn_pipeline_blocking(move || {
        best_sparse_model_with_input_count_blocking(&sparse, input_images)
    })
    .await
    .map_err(|error| SplatError::Process(format!("Sparse model validation task failed: {error}")))?
}

fn best_sparse_model_blocking(
    frames: &Path,
    sparse: &Path,
) -> Result<(PathBuf, ReconstructionReport)> {
    let mut input_images = 0;
    for entry in std::fs::read_dir(frames)? {
        let entry = entry?;
        if entry.path().extension().is_some_and(|extension| {
            extension.eq_ignore_ascii_case("jpg")
                || extension.eq_ignore_ascii_case("jpeg")
                || extension.eq_ignore_ascii_case("png")
        }) && entry.file_type()?.is_file()
        {
            input_images += 1;
        }
    }
    colmap::select_reconstruction_model(sparse, input_images)?
        .ok_or_else(|| SplatError::Process("COLMAP 未生成完整的稀疏模型".into()))
}

fn best_sparse_model_with_input_count_blocking(
    sparse: &Path,
    input_images: u64,
) -> Result<(PathBuf, ReconstructionReport)> {
    colmap::select_reconstruction_model(sparse, input_images)?
        .ok_or_else(|| SplatError::Process("COLMAP did not produce a usable sparse model".into()))
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
        frames.bridge_source_frames = prepared.plan.bridge_source_frames;
        frames.rescue_max_frames = prepared.plan.rescue_max_frames;
        frames.minimum_frame_override_applied = prepared.plan.minimum_frame_override_applied;
        frames.mask_count = Some(prepared.mask_count);
        frames.colmap_input_version = Some(COLMAP_INPUT_VERSION);
    }
}

fn validate_manual_bridge_plan(
    plan: &FramePlan,
    bridge: &crate::planner::bridge_backfill::BridgeBackfillPlan,
) -> Result<()> {
    if let Some(total) = plan.bridge_source_frames {
        let initial = bridge.initial_input_images;
        let budget = (initial / 5).min(total.saturating_sub(initial));
        if initial > total
            || bridge.available_budget != budget
            || plan.rescue_max_frames != initial.saturating_add(budget)
            || bridge.selected_frame_indices.len() as u64 > budget
            || bridge
                .selected_frame_indices
                .iter()
                .any(|index| *index >= total)
            || bridge
                .selected_frame_indices
                .iter()
                .collect::<HashSet<_>>()
                .len()
                != bridge.selected_frame_indices.len()
        {
            return Err(SplatError::Process(
                "Invalid manual bridge checkpoint".into(),
            ));
        }
    }
    Ok(())
}

fn resolve_bridge_frames(
    work: &Path,
    panorama: bool,
    plan: &FramePlan,
    indices: &[u64],
    fps: Option<f64>,
) -> Result<Vec<crate::video::PlannedFrame>> {
    if let Some(total) = plan.bridge_source_frames {
        if indices.iter().any(|index| *index >= total) {
            return Err(SplatError::Process(
                "Bridge frame index is out of range".into(),
            ));
        }
        if panorama {
            return crate::video::panorama::bridge_source_frames(work, total, indices);
        }
        let fps = fps
            .filter(|fps| fps.is_finite() && *fps > 0.)
            .ok_or_else(|| SplatError::Process("Invalid bridge source frame rate".into()))?;
        return Ok(indices
            .iter()
            .map(|index| crate::video::PlannedFrame {
                source_frame_index: *index,
                timestamp_seconds: *index as f64 / fps,
            })
            .collect());
    }
    Ok(indices
        .iter()
        .filter_map(|index| {
            plan.candidate_frames
                .iter()
                .find(|frame| frame.source_frame_index == *index)
                .cloned()
        })
        .collect())
}

fn oriented_video_dimensions(video: &VideoInfo) -> (u32, u32) {
    if video.rotation.rem_euclid(180) == 90 {
        (video.height, video.width)
    } else {
        (video.width, video.height)
    }
}

#[cfg(test)]
async fn remove_bridge_outputs(
    frames: &Path,
    colmap_frames: &Path,
    masks: &Path,
    additional: &[PlannedFrame],
    has_alpha: bool,
) -> Result<()> {
    let extension = if has_alpha { "png" } else { "jpg" };
    for frame in additional {
        let name = format!("frame_{:010}.{extension}", frame.source_frame_index);
        let mut files = vec![frames.join(&name), colmap_frames.join(&name)];
        if has_alpha {
            files.push(masks.join(format!("{name}.png")));
        }
        for path in files {
            match tokio::fs::remove_file(path).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(())
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
    fn legacy_v1_keeps_saved_none_and_refuses_to_relabel_mixed_source_pixels() {
        let mut state = PipelineStateFile::created_for(Quality::Fast, ProjectInputType::Images);
        state.brush_training.oom_retry_used = true;
        let before = serde_json::to_value(&state).unwrap();
        validate_legacy_v1_preparation_policy(state.ordinary_image_preparation_version).unwrap();
        assert_eq!(serde_json::to_value(&state).unwrap(), before);
        state.ordinary_image_preparation_version =
            Some(super::super::inputs::ORDINARY_IMAGE_PREPARATION_VERSION);
        let before = serde_json::to_value(&state).unwrap();
        let error = validate_legacy_v1_preparation_policy(state.ordinary_image_preparation_version)
            .unwrap_err();
        assert!(error
            .to_string()
            .contains(legacy_work_order::UNRESTORABLE_RECIPE));
        assert_eq!(serde_json::to_value(&state).unwrap(), before);
    }

    #[tokio::test]
    async fn a_new_unversioned_v1_request_stops_before_source_access_or_project_creation() {
        let root = tempfile::tempdir().unwrap();
        let projects = root.path().join("projects-not-created");
        let runner = PipelineRunner::new(
            EnginePaths::from_root(root.path().join("missing-engines")),
            |_| {},
        );
        let error = match runner
            .generate_incremental_reshoot(
                uuid::Uuid::new_v4(),
                &root.path().join("missing-input"),
                ReshootInputType::Images,
                &projects,
                Quality::Fast,
            )
            .await
        {
            Ok(_) => panic!("V1 must not adopt a global derivative version for mixed inputs"),
            Err(error) => error,
        };
        assert!(error
            .to_string()
            .contains(legacy_work_order::UNRESTORABLE_RECIPE));
        assert!(!projects.exists());
    }

    #[test]
    fn a_reserved_camera_policy_overrides_new_runner_defaults_without_upgrading_old_creation() {
        use uuid::Uuid;
        for version in [
            None,
            Some(super::super::inputs::CAMERA_GROUPING_POLICY_VERSION),
        ] {
            let root = tempfile::tempdir().unwrap();
            let intent = crate::project::creation::ProjectCreationIntent {
                ordinary_image_preparation_version: version,
                camera_grouping_policy_version: version,
                ba_policy_version: None,
                brush_execution_options: None,
                intent_id: Uuid::new_v4(),
                project_id: Uuid::new_v4(),
                task_id: Uuid::new_v4(),
                creating_run_id: Uuid::new_v4(),
                project_path: root.path().join("reserved"),
                request_identity: "reserved".into(),
            };
            let runner =
                PipelineRunner::new(EnginePaths::from_root(root.path().join("engines")), |_| {})
                    .with_creation_intent(Some(intent));
            assert_eq!(runner.camera_grouping_policy_version(), version);
            assert_eq!(runner.ordinary_image_preparation_version(), version);
        }
    }

    fn write_valid_brush_candidate(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            b"ply\nformat binary_little_endian 1.0\nelement vertex 1\nproperty float x\nproperty float y\nproperty float z\nproperty float f_dc_0\nproperty float opacity\nproperty float scale_0\nproperty float rot_0\nend_header\n",
        )
        .unwrap();
    }

    #[tokio::test]
    async fn fast_balanced_budget_is_saved_once_and_resolved_recipes_remain_authoritative() {
        let root = tempfile::tempdir().unwrap();
        let runner =
            PipelineRunner::new(EnginePaths::from_root(root.path().join("engines")), |_| {});
        let model = root.path().join("model");
        let images = root.path().join("images");
        std::fs::create_dir_all(&model).unwrap();
        std::fs::create_dir_all(&images).unwrap();
        std::fs::write(model.join("cameras.txt"), "1 SIMPLE_RADIAL 4 2 4 2 1 0\n").unwrap();
        std::fs::write(
            model.join("images.txt"),
            "1 1 0 0 0 0 0 0 1 a.png\n1 1 1\n2 1 0 0 0 1 0 0 1 b.png\n1 1 1\n",
        )
        .unwrap();
        std::fs::write(
            model.join("points3D.txt"),
            "1 0 0 1 255 255 255 0 1 0 2 0\n",
        )
        .unwrap();
        for name in ["a.png", "b.png"] {
            image::RgbImage::from_pixel(4, 2, image::Rgb([0, 0, 0]))
                .save(images.join(name))
                .unwrap();
        }
        let missing = root.path().join("missing");
        for quality in [Quality::Fast, Quality::Balanced] {
            let mut state = PipelineStateFile::created(quality);
            state.generation_policy = Some(Default::default());
            runner
                .prepare_fine_splat_budget(&mut state, quality, &model, &images, false, 2)
                .await
                .unwrap();
            let saved = state.brush_training.fine_splat_budget.unwrap();
            assert_eq!(
                saved.coefficient,
                super::super::splat_budget::coefficient(quality)
            );
            assert_eq!(saved.effective_pixels, 4);
            let mut resumed: PipelineStateFile =
                serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
            runner
                .prepare_fine_splat_budget(&mut resumed, quality, &missing, &missing, false, 2)
                .await
                .unwrap();
            assert_eq!(resumed.brush_training.fine_splat_budget, Some(saved));
            assert!(runner
                .prepare_fine_splat_budget(&mut resumed, quality, &missing, &missing, false, 3)
                .await
                .is_err());

            // An already resolved saved attempt is not retrofitted, rescanned
            // or rejected just because this executable supports a new budget.
            let old = crate::presets::resolve_brush_training_preset(quality, true, None, 4, 1);
            resumed.brush_training.resolved = Some(old);
            runner
                .prepare_fine_splat_budget(&mut resumed, quality, &missing, &missing, false, 3)
                .await
                .unwrap();
            assert_eq!(resumed.brush_training.resolved, Some(old));
        }
    }

    #[tokio::test]
    async fn fine_budget_snapshot_resumes_without_rescanning_and_other_paths_are_untouched() {
        let root = tempfile::tempdir().unwrap();
        let runner =
            PipelineRunner::new(EnginePaths::from_root(root.path().join("engines")), |_| {});
        let missing = root.path().join("missing");
        for (quality, version, panorama) in [
            (Quality::Fast, 2, false),
            (Quality::Balanced, 2, false),
            (Quality::Fast, 6, false),
            (Quality::Balanced, 6, false),
            (Quality::High, 1, false),
            (Quality::High, 2, true),
            (Quality::Fast, 7, true),
            (Quality::Balanced, 7, true),
        ] {
            let mut state = PipelineStateFile::created(quality);
            state.generation_policy = Some(super::super::generation_policy::GenerationPolicy {
                version,
                ..Default::default()
            });
            runner
                .prepare_fine_splat_budget(&mut state, quality, &missing, &missing, panorama, 1600)
                .await
                .unwrap();
            assert!(state.brush_training.fine_splat_budget.is_none());
        }
        let mut state = PipelineStateFile::created(Quality::High);
        state.generation_policy = Some(Default::default());
        assert!(runner
            .prepare_fine_splat_budget(&mut state, Quality::High, &missing, &missing, false, 3840)
            .await
            .is_err());
        let budget = super::super::splat_budget::FineSplatBudget {
            version: 1,
            coefficient: 0.5,
            registered_images: 90,
            effective_pixels: 364_993_345,
            sparse_points: 66_218,
            tracked_points: 66_218,
            observations: 191_175,
            mean_track_length: 191_175. / 66_218.,
            requested_max_splats: 2_220_679,
            quality: None,
            training_max_resolution: None,
        };
        state.brush_training.fine_splat_budget = Some(budget);
        let mut resumed: PipelineStateFile =
            serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        runner
            .prepare_fine_splat_budget(&mut resumed, Quality::High, &missing, &missing, false, 3840)
            .await
            .unwrap();
        let mut restored = resumed.brush_training.fine_splat_budget.unwrap();
        assert!((restored.mean_track_length - budget.mean_track_length).abs() < 1e-12);
        // JSON float parsing may differ by one ULP; integer evidence and the
        // recomputed ceiling, rather than exact float bits, must remain equal.
        restored.mean_track_length = budget.mean_track_length;
        assert_eq!(restored, budget);
        let resolution =
            resumed
                .generation_policy
                .unwrap()
                .resolution(Quality::High, None, 2560, 3840, false);
        let training = resumed
            .generation_policy
            .unwrap()
            .training(
                Quality::High,
                None,
                &resolution,
                66_218,
                3_000_000,
                resumed.brush_training.fine_splat_budget.as_ref(),
            )
            .unwrap();
        assert_eq!(training.configured_max_splats, Some(2_220_679));
        assert_eq!(training.preset.max_splats, Some(3_000_000));
    }

    #[tokio::test]
    async fn continuation_keeps_saved_bridge_policy_even_when_legacy_retry_requests_an_upgrade() {
        for manual in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("project"));
            for dir in [
                &paths.frames,
                &paths.colmap_frames,
                &paths.masks,
                &paths.colmap,
                &paths.brush,
            ] {
                tokio::fs::create_dir_all(dir).await.unwrap();
                tokio::fs::write(dir.join("sentinel"), b"old")
                    .await
                    .unwrap();
            }
            tokio::fs::write(paths.project.join("final.ply"), b"published")
                .await
                .unwrap();
            tokio::fs::write(paths.project.join("edit.ply"), b"edited")
                .await
                .unwrap();
            let manager = ProjectManager::with_root(root.path().into());
            let runner =
                PipelineRunner::new(EnginePaths::from_root(root.path().join("engines")), |_| {});
            let old = super::super::generation_policy::GenerationPolicy {
                version: 2,
                frame_sampling_ratio: manual.then_some(0.9),
                ..Default::default()
            };
            let new = super::super::generation_policy::GenerationPolicy { version: 3, ..old };
            let video = VideoInfo {
                duration: 60.,
                fps: 30.,
                total_frames: 1800,
                width: 1920,
                height: 1080,
                codec: "h264".into(),
                rotation: 0,
                pixel_format: "yuv420p".into(),
                has_alpha: false,
            };
            let mut state = PipelineStateFile::created(Quality::Fast);
            state.generation_policy = Some(old);
            state.planner_enabled = true;
            state.configuration = Some((*crate::presets::pipeline_optimization_config()).clone());
            state.resolution_plan = Some(old.resolution(Quality::Fast, None, 1920, 1080, true));
            state.frames = Some(FrameState::from(&old.frame_plan(&video, Quality::Fast)));
            state.video = Some(video);
            state.features_complete = true;
            state.matching_complete = true;
            state.reconstruction_complete = true;
            let saved = serde_json::to_value(&state).unwrap();
            runner
                .prepare_failed_retry(&manager, &paths, &mut state, Some(new))
                .await
                .unwrap();
            assert_eq!(serde_json::to_value(&state).unwrap(), saved);
            assert_eq!(state.generation_policy, Some(old));
            assert!(paths.colmap.join("sentinel").exists());
            assert!(paths.frames.join("sentinel").exists());
            assert!(state.reconstruction_complete);
            assert!(!paths.work.join("cache-archive").exists());
            assert!(state.retry_cache_pending.is_empty());
            assert_eq!(
                std::fs::read(paths.project.join("final.ply")).unwrap(),
                b"published"
            );
            assert_eq!(
                std::fs::read(paths.project.join("edit.ply")).unwrap(),
                b"edited"
            );
        }
    }

    #[test]
    fn manual_bridge_checkpoint_freezes_original_budget_across_partial_extraction() {
        let mut plan = FramePlan {
            estimated_frames: 10,
            selected_frames: (0..100)
                .step_by(10)
                .map(|index| PlannedFrame {
                    source_frame_index: index,
                    timestamp_seconds: index as f64 / 30.,
                })
                .collect(),
            ..Default::default()
        };
        plan.use_manual_bridge_budget(1000);
        let bridge = plan_bridge_backfill(&plan, &[0, 90].into_iter().collect(), 2);
        validate_manual_bridge_plan(&plan, &bridge).unwrap();
        let added = resolve_bridge_frames(
            Path::new("unused"),
            false,
            &plan,
            &bridge.selected_frame_indices,
            Some(30.),
        )
        .unwrap();
        assert_eq!(added.len(), 2);
        assert!(
            resolve_bridge_frames(Path::new("unused"), false, &plan, &[1000], Some(30.)).is_err()
        );
        let mut saved = FrameState::from(&plan);
        saved.selected_frames.extend(added);
        saved.extracted_frames = Some(12);
        let saved: FrameState =
            serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        assert_eq!(saved.initial_extracted_frames, 10);
        let resumed = FramePlan {
            selected_frames: saved.selected_frames,
            bridge_source_frames: saved.bridge_source_frames,
            rescue_max_frames: saved.rescue_max_frames,
            ..Default::default()
        };
        validate_manual_bridge_plan(&resumed, &bridge).unwrap();
        assert_eq!(resumed.rescue_max_frames, 12);
        let mut invalid = bridge.clone();
        invalid.available_budget = 3;
        assert!(validate_manual_bridge_plan(&resumed, &invalid).is_err());
        invalid = bridge.clone();
        invalid.selected_frame_indices[1] = invalid.selected_frame_indices[0];
        assert!(validate_manual_bridge_plan(&resumed, &invalid).is_err());
        let old: FrameState =
            serde_json::from_str(r#"{"retentionRatio":0.5,"samplingFps":15,"estimatedFrames":10}"#)
                .unwrap();
        assert_eq!(old.bridge_source_frames, None);
    }

    #[test]
    fn manual_bridge_metrics_distinguish_source_candidates_from_extracted_frames() {
        let tracker = PlannerEffectivenessTracker::default();
        let runner = PipelineRunner::new(default_engine_paths(None), |_| {})
            .with_effectiveness_tracker(tracker.clone());
        let video = VideoInfo {
            duration: 60.,
            fps: 30.,
            total_frames: 1800,
            width: 4096,
            height: 2048,
            codec: "h264".into(),
            rotation: 0,
            pixel_format: "yuv420p".into(),
            has_alpha: false,
        };
        let policy = super::super::generation_policy::GenerationPolicy {
            frame_sampling_ratio: Some(0.2),
            ..Default::default()
        };
        let plan = policy.frame_plan(&video, Quality::High);
        let mut state = PipelineStateFile::created(Quality::High);
        state.video = Some(video);
        state.generation_policy = Some(policy);
        state.frames = Some(FrameState::from(&plan));
        runner.record_frame_metrics(&state, 360, Some(&plan));
        let metrics = tracker.snapshot().frame_plan.unwrap();
        assert_eq!(metrics.initial_selected_count, 360);
        assert_eq!(metrics.candidate_count, 1800);
        assert_eq!(metrics.configured_candidate_fps, None);
        assert_eq!(plan.rescue_max_frames, 432);
    }

    #[tokio::test]
    #[ignore = "Runs bundled FFmpeg/FFprobe on isolated synthetic video; no SfM or training"]
    async fn native_manual_bridge_v3_extraction_and_rollback() {
        use crate::process::ProcessSpec;
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let engines = EnginePaths::from_root(workspace.join("engines"));
        assert!(engines.ffmpeg.is_file() && engines.ffprobe.is_file());
        let root = workspace
            .join(".cache/manual-bridge-v3")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&root).unwrap();
        let mut records = Vec::new();
        for (name, panorama, alpha) in [
            ("ordinary", false, false),
            ("ordinary-alpha", false, true),
            ("panorama-vfr", true, false),
            ("panorama-vfr-alpha", true, true),
        ] {
            let work = root.join(name);
            std::fs::create_dir_all(&work).unwrap();
            let input = work.join("source.mov");
            let mut filter = String::from("format=rgba");
            if alpha {
                filter.push_str(",colorchannelmixer=aa=0.5");
            }
            if panorama {
                filter.push_str(",setpts=PTS+5/TB+0.2*floor(N/15)/TB");
            }
            let manager = ProcessManager::new();
            let result = manager
                .run(ProcessSpec {
                    executable: engines.ffmpeg.clone(),
                    args: vec![
                        "-v".into(),
                        "error".into(),
                        "-f".into(),
                        "lavfi".into(),
                        "-i".into(),
                        "testsrc=size=128x64:rate=30:duration=4".into(),
                        "-vf".into(),
                        filter.into(),
                        "-fps_mode".into(),
                        "vfr".into(),
                        "-c:v".into(),
                        "qtrle".into(),
                        "-pix_fmt".into(),
                        "argb".into(),
                        "-threads".into(),
                        "1".into(),
                        "-y".into(),
                        input.as_os_str().to_owned(),
                    ],
                    working_directory: Some(work.clone()),
                    log_path: Some(work.join("generate.log")),
                    observer: None,
                })
                .await
                .unwrap();
            assert!(result.success, "{}", result.failure_detail());
            let source_before = std::fs::read(&input).unwrap();
            let mut video = probe_video(&engines.ffprobe, &input, None, &manager)
                .await
                .unwrap();
            // Opaque case explicitly exercises the RGB/JPEG extraction path;
            // Alpha cases exercise aligned RGBA, COLMAP images, and masks.
            video.has_alpha = alpha;
            let policy = super::super::generation_policy::GenerationPolicy {
                frame_sampling_ratio: Some(if panorama { 0.3 } else { 0.5 }),
                ..Default::default()
            };
            let runner =
                PipelineRunner::new(engines.clone(), |_| {}).with_generation_policy(Some(policy));
            let frames = work.join("frames");
            let colmap = work.join("colmap_frames");
            let masks = work.join("masks");
            let prepared = runner
                .prepare_frames(
                    &input,
                    Quality::Fast,
                    &frames,
                    &colmap,
                    &masks,
                    None,
                    true,
                    Some(video),
                    panorama.then_some((128, 64)),
                    panorama,
                    None,
                )
                .await
                .unwrap();
            let base: Vec<_> = prepared
                .plan
                .selected_frames
                .iter()
                .map(|frame| {
                    let file = frames.join(format!(
                        "frame_{:010}.{}",
                        frame.source_frame_index,
                        if alpha { "png" } else { "jpg" }
                    ));
                    (file.clone(), std::fs::read(file).unwrap())
                })
                .collect();
            let registered = [
                prepared
                    .plan
                    .selected_frames
                    .first()
                    .unwrap()
                    .source_frame_index,
                prepared
                    .plan
                    .selected_frames
                    .last()
                    .unwrap()
                    .source_frame_index,
            ]
            .into_iter()
            .collect();
            let bridge = plan_bridge_backfill(&prepared.plan, &registered, 2);
            validate_manual_bridge_plan(&prepared.plan, &bridge).unwrap();
            let added = resolve_bridge_frames(
                &work,
                panorama,
                &prepared.plan,
                &bridge.selected_frame_indices,
                prepared.video.as_ref().map(|v| v.fps),
            )
            .unwrap();
            assert!(!added.is_empty());
            assert!(added.len() as u64 <= prepared.extracted_frames / 5);
            if panorama {
                let times: Vec<crate::video::panorama::FrameTime> = serde_json::from_slice(
                    &std::fs::read(work.join("panorama-frame-times.json")).unwrap(),
                )
                .unwrap();
                assert!(times[0].timestamp_seconds >= 5.);
                assert!(times
                    .windows(2)
                    .any(|p| p[1].timestamp_seconds - p[0].timestamp_seconds > 0.1));
                for frame in &added {
                    assert_eq!(
                        frame.timestamp_seconds,
                        times[frame.source_frame_index as usize].timestamp_seconds
                    );
                }
            }
            let cancelled = ProcessManager::new();
            cancelled.cancel();
            assert!(extract_additional_frames(
                &engines.ffmpeg,
                &input,
                &frames,
                &colmap,
                &masks,
                &added,
                alpha,
                Some((128, 64)),
                None,
                &cancelled,
                None
            )
            .await
            .is_err());
            assert_eq!(
                validate_extraction(&frames, &colmap, &masks, alpha)
                    .await
                    .unwrap()
                    .frame_count,
                prepared.extracted_frames
            );
            let output = extract_additional_frames(
                &engines.ffmpeg,
                &input,
                &frames,
                &colmap,
                &masks,
                &added,
                alpha,
                Some((128, 64)),
                Some(work.join("bridge.log")),
                &manager,
                None,
            )
            .await
            .unwrap();
            assert_eq!(
                output.frame_count,
                prepared.extracted_frames + added.len() as u64
            );
            for (file, bytes) in &base {
                assert_eq!(&std::fs::read(file).unwrap(), bytes);
            }
            let count = output.frame_count;
            remove_bridge_outputs(&frames, &colmap, &masks, &added, alpha)
                .await
                .unwrap();
            assert_eq!(
                validate_extraction(&frames, &colmap, &masks, alpha)
                    .await
                    .unwrap()
                    .frame_count,
                prepared.extracted_frames
            );
            assert_eq!(std::fs::read(&input).unwrap(), source_before);
            records.push(
                serde_json::json!({"case":name,"initial":prepared.extracted_frames,
                "budget":prepared.plan.rescue_max_frames,"added":added.len(),"combined":count,
                "sourceUnchanged":true,"cancelAndRollbackProtected":true}),
            );
        }
        std::fs::write(
            root.join("result.json"),
            serde_json::to_vec_pretty(&records).unwrap(),
        )
        .unwrap();
        eprintln!("Native manual bridge evidence: {}", root.display());
    }

    #[tokio::test]
    async fn continuation_preserves_legacy_policy_none_and_does_not_archive_compatible_work() {
        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("project"));
        for dir in [
            &paths.frames,
            &paths.colmap_frames,
            &paths.masks,
            &paths.colmap,
            &paths.brush,
        ] {
            tokio::fs::create_dir_all(dir).await.unwrap();
            tokio::fs::write(dir.join("sentinel"), b"old-cache")
                .await
                .unwrap();
        }
        tokio::fs::write(paths.project.join("final.ply"), b"published")
            .await
            .unwrap();
        tokio::fs::write(paths.project.join("edit.ply"), b"edited")
            .await
            .unwrap();
        let manager = ProjectManager::with_root(root.path().into());
        let runner =
            PipelineRunner::new(EnginePaths::from_root(root.path().join("engines")), |_| {});
        let mut state = PipelineStateFile::created(Quality::High);
        state.features_complete = true;
        state.matching_complete = true;
        state.reconstruction_complete = true;
        state.brush_complete = true;
        runner
            .prepare_failed_retry(&manager, &paths, &mut state, Some(Default::default()))
            .await
            .unwrap();
        assert_eq!(state.generation_policy, None);
        assert!(state.features_complete && state.matching_complete && state.brush_complete);
        assert!(state.reconstruction_complete);
        for dir in [
            &paths.frames,
            &paths.colmap_frames,
            &paths.masks,
            &paths.colmap,
            &paths.brush,
        ] {
            assert_eq!(
                tokio::fs::read(dir.join("sentinel")).await.unwrap(),
                b"old-cache"
            );
        }
        assert!(!paths.work.join("cache-archive").exists());
        assert_eq!(
            tokio::fs::read(paths.project.join("final.ply"))
                .await
                .unwrap(),
            b"published"
        );
        assert_eq!(
            tokio::fs::read(paths.project.join("edit.ply"))
                .await
                .unwrap(),
            b"edited"
        );
        assert!(state.retry_cache_pending.is_empty());
        // A repeated continuation keeps both legacy policy and already validated work.
        tokio::fs::create_dir_all(&paths.colmap).await.unwrap();
        tokio::fs::write(paths.colmap.join("database.db"), b"compatible")
            .await
            .unwrap();
        state.features_complete = true;
        state.matching_complete = true;
        state.reconstruction_complete = true;
        runner
            .prepare_failed_retry(&manager, &paths, &mut state, Some(Default::default()))
            .await
            .unwrap();
        assert!(
            state.features_complete && state.matching_complete && state.reconstruction_complete
        );
        assert_eq!(state.generation_policy, None);
        assert!(state.brush_complete);
        assert_eq!(
            tokio::fs::read(paths.colmap.join("database.db"))
                .await
                .unwrap(),
            b"compatible"
        );
    }

    #[tokio::test]
    async fn v5_continuation_preserves_frames_sfm_actual_brush_and_consumed_oom_allowance() {
        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("project"));
        for path in [&paths.project, &paths.colmap, &paths.brush] {
            tokio::fs::create_dir_all(path).await.unwrap();
        }
        tokio::fs::write(paths.colmap.join("database.db"), b"compatible")
            .await
            .unwrap();
        tokio::fs::write(paths.brush.join("partial"), b"failed training")
            .await
            .unwrap();
        let manager = ProjectManager::with_root(root.path().into());
        let runner =
            PipelineRunner::new(EnginePaths::from_root(root.path().join("engines")), |_| {});
        let policy = super::super::generation_policy::GenerationPolicy {
            version: 5,
            auto_retry_enabled: true,
            frame_sampling_ratio: None,
            frame_sampling_fps: None,
        };
        let video = VideoInfo {
            duration: 60.,
            fps: 30.,
            total_frames: 1_800,
            width: 1920,
            height: 1080,
            codec: "h264".into(),
            rotation: 0,
            pixel_format: "yuv420p".into(),
            has_alpha: false,
        };
        let frame_plan = policy.frame_plan(&video, Quality::Balanced);
        let resolution = policy.resolution(Quality::Balanced, None, 1920, 1080, true);
        let initial =
            resolve_brush_training_preset(Quality::Balanced, true, Some(12_288), 1920, 10_000);
        let mut downgraded = initial;
        downgraded.preset.max_splats = Some(500_000);
        let mut configuration = (*crate::presets::pipeline_optimization_config()).clone();
        configuration.shared.minimum_selected_frames = 777;
        let mut state = PipelineStateFile::created(Quality::Balanced);
        state.generation_policy = Some(policy);
        state.configuration = Some(configuration);
        state.planner_enabled = true;
        state.video = Some(video);
        state.frames = Some(FrameState::from(&frame_plan));
        state.resolution_plan = Some(resolution);
        state.features_complete = true;
        state.matching_complete = true;
        state.reconstruction_complete = true;
        state.brush_training.initial_resolved = Some(initial);
        state.brush_training.resolved = Some(downgraded);
        state.brush_training.oom_retry_used = true;
        state.brush_training.oom_retry_started = true;
        state.brush_training.oom_retry_source_count = Some(625_000);

        runner
            .prepare_failed_retry(&manager, &paths, &mut state, Some(policy))
            .await
            .unwrap();

        assert_eq!(state.generation_policy, Some(policy));
        assert_eq!(
            state
                .configuration
                .as_ref()
                .unwrap()
                .shared
                .minimum_selected_frames,
            777
        );
        assert_eq!(
            state.frames.as_ref().unwrap().sampling_fps,
            frame_plan.sampling_fps
        );
        assert_eq!(state.resolution_plan, Some(resolution));
        assert!(state.features_complete && state.matching_complete);
        assert!(state.reconstruction_complete);
        assert!(!state.brush_complete);
        assert_eq!(state.brush_training.initial_resolved, Some(initial));
        assert_eq!(state.brush_training.resolved, Some(downgraded));
        assert!(state.brush_training.oom_retry_used);
        assert!(state.brush_training.oom_retry_started);
        assert_eq!(state.brush_training.oom_retry_source_count, Some(625_000));
        assert_eq!(
            tokio::fs::read(paths.brush.join("partial")).await.unwrap(),
            b"failed training"
        );
        assert!(paths.colmap.join("database.db").is_file());
    }

    #[tokio::test]
    async fn legacy_continuation_keeps_no_policy_and_actual_reshoot_route_and_consumed_fallbacks() {
        use crate::pipeline::reshoot::{
            ModelCondition, ReconstructionMode, ReshootOptions, ReshootPlan, ReshootRecipe,
            TrainingMode, RECIPE_VERSION,
        };
        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("project"));
        tokio::fs::create_dir_all(&paths.project).await.unwrap();
        let manager = ProjectManager::with_root(root.path().into());
        let runner =
            PipelineRunner::new(EnginePaths::from_root(root.path().join("engines")), |_| {});
        let mut state = PipelineStateFile::created(Quality::Balanced);
        state.planner_enabled = false;
        state.frames = Some(FrameState {
            estimated_frames: 120,
            extracted_frames: Some(120),
            ..Default::default()
        });
        state.features_complete = true;
        state.matching_complete = true;
        state.reconstruction_complete = true;
        state.reshoot = Some(ReshootState {
            recipe: Some(ReshootRecipe {
                version: RECIPE_VERSION,
                normal_parameters_version: None,
                camera_assignment: None,
                options: ReshootOptions {
                    condition: ModelCondition::Good,
                    camera_relation: None,
                    allow_fallback: true,
                    source_identity: "source".into(),
                    input_identity: "input".into(),
                    projection: crate::video::panorama::VideoProjection::Perspective,
                    image_matching_strategy: crate::project::ImageMatchingStrategy::Exhaustive,
                },
                initial: ReshootPlan {
                    reconstruction: ReconstructionMode::Incremental,
                    training: TrainingMode::Warm,
                },
                actual: ReshootPlan {
                    reconstruction: ReconstructionMode::Full,
                    training: TrainingMode::Cold,
                },
                batch_id: "legacy-v2".into(),
                reconstruction_fallbacks: 1,
                training_fallbacks: 1,
                expanded_matching: true,
                source_names: vec!["old.jpg".into()],
                new_names: vec!["new.jpg".into()],
                warm_parameters: None,
                init_sha256: None,
                init_georeference: None,
                stored_input_identity: None,
                panorama_frames: vec![],
            }),
            incremental_reconstruction_complete: true,
            ..Default::default()
        });

        runner
            .prepare_failed_retry(&manager, &paths, &mut state, None)
            .await
            .unwrap();

        assert_eq!(state.generation_policy, None);
        assert!(!state.planner_enabled);
        assert_eq!(state.frames.as_ref().unwrap().extracted_frames, Some(120));
        assert!(state.features_complete && state.matching_complete);
        assert!(state.reconstruction_complete);
        let checkpoint = state.reshoot.as_ref().unwrap();
        assert!(checkpoint.incremental_reconstruction_complete);
        let recipe = checkpoint.recipe.as_ref().unwrap();
        assert_eq!(recipe.actual.reconstruction, ReconstructionMode::Full);
        assert_eq!(recipe.actual.training, TrainingMode::Cold);
        assert_ne!(recipe.actual, recipe.initial);
        assert_eq!(recipe.reconstruction_fallbacks, 1);
        assert_eq!(recipe.training_fallbacks, 1);
        assert!(recipe.expanded_matching);
        assert_eq!(recipe.source_names, ["old.jpg"]);
        assert_eq!(recipe.new_names, ["new.jpg"]);
    }

    #[tokio::test]
    async fn publish_only_continuation_keeps_candidate_and_actual_retry_evidence() {
        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("project"));
        tokio::fs::create_dir_all(&paths.project).await.unwrap();
        let final_ply = paths.project.join("final.ply");
        write_valid_brush_candidate(&final_ply);
        let candidate = paths.brush.join("final.ply.tmp");
        write_valid_brush_candidate(&candidate);
        let candidate_bytes = tokio::fs::read(&candidate).await.unwrap();
        let manager = ProjectManager::with_root(root.path().into());
        let policy = super::super::generation_policy::GenerationPolicy {
            version: 5,
            ..Default::default()
        };
        let runner =
            PipelineRunner::new(EnginePaths::from_root(root.path().join("engines")), |_| {})
                .with_failed_retry_policy(Some(Some(policy)));
        let mut state = PipelineStateFile::created(Quality::High);
        state.generation_policy = Some(policy);
        state.brush_complete = true;
        state.brush_training.oom_retry_used = true;
        state.brush_training.oom_retry_started = true;

        runner
            .prepare_failed_retry(&manager, &paths, &mut state, Some(policy))
            .await
            .unwrap();

        assert!(state.brush_complete);
        assert!(state.brush_training.oom_retry_started);
        assert!(final_ply.is_file());
        assert!(reusable_brush_candidate(&paths.brush).is_some());
        assert!(!paths.work.join("cache-archive").exists());
        assert!(!runner.finalize_manual_retry_markers(&mut state));
        assert!(state.brush_training.oom_retry_started);
        assert!(state.brush_training.oom_retry_used);
        assert!(reusable_brush_candidate(&paths.brush).is_some());
        assert_eq!(tokio::fs::read(&candidate).await.unwrap(), candidate_bytes);
    }

    #[tokio::test]
    async fn failed_retry_archive_is_a_durable_barrier_and_alpha_requires_nonopaque_pixels() {
        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("project"));
        tokio::fs::create_dir_all(&paths.frames).await.unwrap();
        tokio::fs::create_dir_all(&paths.colmap).await.unwrap();
        tokio::fs::write(paths.work.join("cache-archive"), b"blocked-directory")
            .await
            .unwrap();
        tokio::fs::write(paths.colmap.join("database.db"), b"old")
            .await
            .unwrap();
        let manager = ProjectManager::with_root(root.path().into());
        let runner =
            PipelineRunner::new(EnginePaths::from_root(root.path().join("engines")), |_| {});
        let mut state = PipelineStateFile::created(Quality::High);
        // The normalizer has already rejected COLMAP and durably recorded the barrier.
        state.retry_cache_pending.push("colmap".into());
        manager.write_state(&paths.state, &state).await.unwrap();
        let error = runner
            .prepare_failed_retry(&manager, &paths, &mut state, Some(Default::default()))
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("retry_preparation_operation=create_retry_archive"));
        assert!(error
            .to_string()
            .contains("path_role=cache_archive_directory"));
        assert!(error.io_error().is_some());
        let preparation_log = tokio::fs::read_to_string(paths.logs.join("retry-preparation.log"))
            .await
            .unwrap();
        assert!(preparation_log.contains("status=failed operation=create_retry_archive"));
        assert!(preparation_log.contains("path_role=cache_archive_directory"));
        assert!(!preparation_log.contains(&paths.project.display().to_string()));
        let saved = manager.read_state(&paths.state).await.unwrap();
        assert!(
            !saved.features_complete
                && !saved.reconstruction_complete
                && !saved.retry_cache_pending.is_empty()
        );
        assert_eq!(
            tokio::fs::read(paths.colmap.join("database.db"))
                .await
                .unwrap(),
            b"old"
        );
        let mut image = image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]));
        image.save(paths.frames.join("frame.png")).unwrap();
        state.actual_transparency = None;
        assert!(!runner
            .actual_transparency(&paths, &mut state, true)
            .await
            .unwrap());
        image.get_pixel_mut(1, 1).0[3] = 254;
        image.save(paths.frames.join("frame.png")).unwrap();
        state.actual_transparency = None;
        assert!(runner
            .actual_transparency(&paths, &mut state, true)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn retry_state_read_failure_keeps_project_context_without_a_pipeline_stage() {
        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("project"));
        tokio::fs::create_dir_all(&paths.state).await.unwrap();
        let manager = ProjectManager::with_root(root.path().into());
        let updates = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = updates.clone();
        let runner =
            PipelineRunner::new(EnginePaths::from_root(root.path().join("engines")), |_| {})
                .with_lifecycle_observer(move |update| observed.lock().unwrap().push(update));
        let metadata = ProjectMetadata {
            id: paths.id,
            workspace_task_id: Some(uuid::Uuid::new_v4()),
            created_at: Utc::now(),
            source_path: root.path().join("source"),
            quality: Quality::Balanced,
            project_path: paths.project.clone(),
            ..Default::default()
        };
        runner.bind_project_context(&paths, &metadata);
        let error = runner
            .retry_preparation_step(
                &paths,
                RetryPreparationOperation::ReadRetryState,
                "project_state",
                None,
                manager.read_state(&paths.state),
            )
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("retry_preparation_operation=read_retry_state"));
        assert!(error.io_error().is_some());
        let context = runner.failure_context();
        assert_eq!(context.project_id, Some(paths.id));
        assert_eq!(
            context.project_path.as_deref(),
            Some(paths.project.as_path())
        );
        assert_eq!(
            context.logs_directory.as_deref(),
            Some(paths.logs.as_path())
        );
        assert_eq!(context.failed_stage, None);
        let updates = updates.lock().unwrap();
        assert!(updates
            .iter()
            .any(|update| matches!(update, RunnerUpdate::Project(_))));
        assert!(updates
            .iter()
            .any(|update| matches!(update, RunnerUpdate::LogOpened { path, .. } if path.ends_with("retry-preparation.log"))));
    }

    #[tokio::test]
    async fn retry_reuse_barrier_failure_reports_state_write_operation() {
        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("project"));
        tokio::fs::create_dir_all(&paths.state).await.unwrap();
        let manager = ProjectManager::with_root(root.path().into());
        let runner =
            PipelineRunner::new(EnginePaths::from_root(root.path().join("engines")), |_| {});
        let mut state = PipelineStateFile::created(Quality::High);
        state.retry_cache_pending.push("brush".into());
        let error = runner
            .prepare_failed_retry(&manager, &paths, &mut state, Some(Default::default()))
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("retry_preparation_operation=write_retry_reuse_barrier"));
        assert!(error.to_string().contains("path_role=project_state"));
        assert!(error.io_error().is_some());
    }

    #[tokio::test]
    async fn unavailable_retry_log_does_not_replace_the_original_io_error() {
        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("project"));
        tokio::fs::create_dir_all(&paths.project).await.unwrap();
        tokio::fs::write(&paths.logs, b"not-a-directory")
            .await
            .unwrap();
        let runner =
            PipelineRunner::new(EnginePaths::from_root(root.path().join("engines")), |_| {});
        let error = runner
            .retry_preparation_step(
                &paths,
                RetryPreparationOperation::ReadRetryState,
                "project_state",
                None,
                async { Err::<(), _>(SplatError::Io(std::io::Error::from_raw_os_error(5))) },
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.io_error().and_then(std::io::Error::raw_os_error),
            Some(5)
        );
        assert!(error
            .to_string()
            .contains("retry_preparation_operation=read_retry_state"));
    }

    #[tokio::test]
    async fn retry_archive_progress_failure_resumes_without_reusing_moved_cache() {
        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("project"));
        tokio::fs::create_dir_all(&paths.brush).await.unwrap();
        tokio::fs::write(paths.brush.join("sentinel"), b"old-cache")
            .await
            .unwrap();
        tokio::fs::write(paths.project.join("final.ply"), b"published")
            .await
            .unwrap();
        let manager = ProjectManager::with_root(root.path().into());
        let runner =
            PipelineRunner::new(EnginePaths::from_root(root.path().join("engines")), |_| {});
        let mut pending = PipelineStateFile::created(Quality::High);
        pending.retry_cache_pending.push("brush".into());
        manager.write_state(&paths.state, &pending).await.unwrap();

        tokio::fs::remove_file(&paths.state).await.unwrap();
        tokio::fs::create_dir(&paths.state).await.unwrap();
        let mut attempt = pending.clone();
        let error = runner
            .archive_retry_cache(&manager, &paths, &mut attempt)
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("retry_preparation_operation=write_retry_archive_progress"));
        assert!(error.to_string().contains("cache=brush"));
        assert!(!paths.brush.exists());
        assert!(attempt.retry_cache_pending.is_empty());

        tokio::fs::remove_dir(&paths.state).await.unwrap();
        manager.write_state(&paths.state, &pending).await.unwrap();
        let mut resumed = manager.read_state(&paths.state).await.unwrap();
        runner
            .archive_retry_cache(&manager, &paths, &mut resumed)
            .await
            .unwrap();
        assert!(resumed.retry_cache_pending.is_empty());
        assert_eq!(
            tokio::fs::read(paths.project.join("final.ply"))
                .await
                .unwrap(),
            b"published"
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn locked_retry_cache_reports_archive_operation_and_preserves_pending_entry() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("project"));
        tokio::fs::create_dir_all(&paths.brush).await.unwrap();
        let sentinel = paths.brush.join("sentinel");
        tokio::fs::write(&sentinel, b"locked").await.unwrap();
        let _locked = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(&sentinel)
            .unwrap();
        let manager = ProjectManager::with_root(root.path().into());
        let runner =
            PipelineRunner::new(EnginePaths::from_root(root.path().join("engines")), |_| {});
        let mut state = PipelineStateFile::created(Quality::High);
        state.retry_cache_pending.push("brush".into());
        let error = runner
            .archive_retry_cache(&manager, &paths, &mut state)
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("retry_preparation_operation=archive_retry_cache"));
        assert!(error.to_string().contains("cache=brush"));
        assert!(error
            .io_error()
            .and_then(std::io::Error::raw_os_error)
            .is_some());
        assert_eq!(state.retry_cache_pending, ["brush"]);
        assert!(paths.brush.exists());
    }

    #[test]
    fn image_matching_dispatch_preserves_video_and_quality_loop_policy() {
        use crate::project::ImageMatchingStrategy::{Exhaustive, Sequential};
        for quality in [Quality::Fast, Quality::Balanced, Quality::High] {
            for planner in [false, true] {
                assert_eq!(
                    image_matching_policy(ProjectInputType::Images, Exhaustive, quality, planner),
                    (false, false)
                );
                assert_eq!(
                    image_matching_policy(ProjectInputType::Images, Sequential, quality, planner),
                    (true, quality.loop_detection_enabled(planner))
                );
                for strategy in [Sequential, Exhaustive] {
                    assert_eq!(
                        image_matching_policy(ProjectInputType::Video, strategy, quality, planner),
                        (true, quality.loop_detection_enabled(planner))
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn resumed_matching_policy_uses_saved_state_not_new_runner_preference() {
        use crate::project::ImageMatchingStrategy::{Exhaustive, Sequential};
        let directory = tempfile::tempdir().unwrap();
        let manager = ProjectManager::for_diagnostics(directory.path().to_path_buf());
        for strategy in [Sequential, Exhaustive] {
            let mut state = PipelineStateFile::created_for(Quality::Fast, ProjectInputType::Images);
            state.image_matching_strategy = strategy;
            state.matching_complete = true;
            let path = directory.path().join("state.json");
            manager.write_state(&path, &state).await.unwrap();
            let runner = PipelineRunner::new(default_engine_paths(None), |_| {})
                .with_image_matching_strategy(if strategy == Sequential {
                    Exhaustive
                } else {
                    Sequential
                });
            let restored = manager.read_state(&path).await.unwrap();
            assert_ne!(
                restored.image_matching_strategy,
                runner.image_matching_strategy
            );
            assert_eq!(
                image_matching_policy(
                    restored.input_type,
                    restored.image_matching_strategy,
                    restored.preset,
                    restored.planner_enabled
                )
                .0,
                strategy == Sequential
            );
            assert!(restored.matching_complete);
        }
    }

    #[tokio::test]
    #[ignore = "Requires real ERP photos and bundled engines; isolated cache only"]
    async fn bundled_panorama_images_full_pipeline() {
        let input = PathBuf::from(
            std::env::var_os("OOOSPLAT_PANORAMA_IMAGES").expect("Set ERP photo directory"),
        );
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let cache = workspace.join(".cache/panorama-images-v1");
        std::fs::create_dir_all(&cache).unwrap();
        let root = tempfile::tempdir_in(&cache).unwrap().keep();
        let events = Arc::new(std::sync::Mutex::new(Vec::<PipelineEvent>::new()));
        let captured = events.clone();
        let runner = PipelineRunner::new(
            default_engine_paths(Some(workspace.join("engines"))),
            move |event| captured.lock().unwrap().push(event),
        )
        .with_video_projection(crate::video::panorama::VideoProjection::Equirectangular);
        eprintln!("Real image pipeline outputs: {}", root.display());
        let result = runner
            .generate_for_diagnostics(&input, Quality::Fast, &root)
            .await;
        std::fs::write(
            cache.join("events.json"),
            serde_json::to_vec_pretty(&*events.lock().unwrap()).unwrap(),
        )
        .unwrap();
        let result = result.unwrap();
        std::fs::write(
            cache.join("result.json"),
            serde_json::to_vec_pretty(&result).unwrap(),
        )
        .unwrap();
        assert!(
            result.input_images >= 2 && result.registered_images >= 2 && result.splat_count > 0
        );
        let project = result.final_ply.parent().unwrap();
        let state: PipelineStateFile =
            serde_json::from_slice(&std::fs::read(project.join("state.json")).unwrap()).unwrap();
        assert!(state.video.is_none());
        assert_eq!(state.input_type, ProjectInputType::Images);
        assert_eq!(
            state
                .panorama
                .as_ref()
                .unwrap()
                .image_sources
                .as_ref()
                .unwrap()
                .len() as u64,
            result.input_images
        );
        assert!(events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.unit.as_deref() == Some("panoramaImageFaces")));
        eprintln!(
            "Real ERP photos: {} input, {} registered, {} splats, {} ms; {}",
            result.input_images,
            result.registered_images,
            result.splat_count,
            result.duration_ms,
            result.final_ply.display()
        );
    }

    #[tokio::test]
    #[ignore = "Requires OOOSPLAT_PANORAMA_VIDEO and bundled COLMAP/Brush; full real pipeline in isolated cache"]
    async fn bundled_panorama_progress_full_pipeline() {
        let input = PathBuf::from(
            std::env::var_os("OOOSPLAT_PANORAMA_VIDEO").expect("Set a real stitched ERP video"),
        );
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let root = tempfile::tempdir_in(workspace.join(".cache/panorama-v1"))
            .unwrap()
            .keep();
        let captured = Arc::new(std::sync::Mutex::new(Vec::<PipelineEvent>::new()));
        let output = captured.clone();
        let runner = PipelineRunner::new(
            default_engine_paths(Some(workspace.join("engines"))),
            move |event| output.lock().unwrap().push(event),
        )
        .with_video_projection(crate::video::panorama::VideoProjection::Equirectangular);
        let result = runner
            .generate_for_diagnostics(&input, Quality::Fast, &root)
            .await;
        let events = captured.lock().unwrap();
        std::fs::write(
            workspace.join(".cache/panorama-v1/progress-e2e-events.json"),
            serde_json::to_vec_pretty(&*events).unwrap(),
        )
        .unwrap();
        let result = result.unwrap_or_else(|error| {
            panic!("Real pipeline failed: {error}; outputs: {}", root.display())
        });
        std::fs::write(
            workspace.join(".cache/panorama-v1/progress-e2e-result.json"),
            serde_json::to_vec_pretty(&result).unwrap(),
        )
        .unwrap();
        assert!(events
            .windows(2)
            .all(|pair| pair[1].progress >= pair[0].progress));
        assert!(events
            .iter()
            .filter(|event| event.stage != PipelineStage::Completed)
            .all(|event| event.progress <= 99.9));
        let mut active = None;
        for event in events.iter() {
            if event.kind == EventKind::Stage {
                active = Some(event);
            }
            if event.kind == EventKind::Runtime {
                let active = active.unwrap();
                assert_eq!(event.stage, active.stage);
                assert_eq!(event.substage, active.substage);
                assert_eq!(
                    event.operation.as_ref().unwrap().id,
                    active.operation.as_ref().unwrap().id
                );
            }
        }
        for stage in [
            PipelineStage::ExtractingFeatures,
            PipelineStage::Matching,
            PipelineStage::Reconstructing,
            PipelineStage::TrainingSplats,
        ] {
            assert!(
                events
                    .iter()
                    .any(|event| event.stage == stage && event.kind == EventKind::Runtime),
                "Missing actual engine runtime for {stage:?}"
            );
        }
        assert!(events
            .iter()
            .filter(|event| matches!(
                event.stage,
                PipelineStage::Matching | PipelineStage::Reconstructing
            ) && event.kind == EventKind::Progress)
            .all(|event| event.indeterminate && event.stage_progress.is_none()));
        eprintln!(
            "Real panorama pipeline: {} input frames, {} registered, {} splats, {} ms; PLY: {}",
            result.input_images,
            result.registered_images,
            result.splat_count,
            result.duration_ms,
            result.final_ply.display()
        );
    }

    /// Real engines, no telemetry/catalog writes. The fixture must be a completed
    /// diagnostic project; only isolated copies/derived outputs are modified.
    #[tokio::test]
    #[ignore = "Requires OOOSPLAT_PANORAMA_TEST_PROJECT and the bundled GPU runtime"]
    async fn bundled_panorama_cancel_and_resume() {
        let source = PathBuf::from(
            std::env::var_os("OOOSPLAT_PANORAMA_TEST_PROJECT")
                .expect("Set a completed diagnostic project path"),
        );
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let temporary = tempfile::tempdir_in(workspace.join(".cache/panorama-v1")).unwrap();
        let project = temporary.path().join("resume-project");
        let reuse_faces = std::env::var_os("OOOSPLAT_PANORAMA_RESUME_REUSE_FACES").is_some();
        let original_face_batches = if reuse_faces {
            std::fs::read_dir(source.join("work/panorama-training/images"))
                .unwrap()
                .count()
        } else {
            0
        };
        fn copy_readonly_tree(src: &Path, dst: &Path, reuse_faces: bool) {
            std::fs::create_dir_all(dst).unwrap();
            for e in std::fs::read_dir(src).unwrap() {
                let e = e.unwrap();
                let name = e.file_name();
                if ["panorama-training", "brush", "final.ply", "edit.ply"]
                    .iter()
                    .any(|n| {
                        name == std::ffi::OsStr::new(n)
                            && !(*n == "panorama-training" && reuse_faces)
                    })
                {
                    continue;
                }
                let dest = dst.join(&name);
                if e.file_type().unwrap().is_dir() {
                    copy_readonly_tree(&e.path(), &dest, reuse_faces);
                } else {
                    std::fs::copy(e.path(), dest).unwrap();
                }
            }
        }
        copy_readonly_tree(&source, &project, reuse_faces);
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), project.clone());
        let manager = ProjectManager::for_diagnostics(temporary.path().into());
        let mut metadata: ProjectMetadata =
            serde_json::from_slice(&std::fs::read(&paths.metadata).unwrap()).unwrap();
        metadata.id = paths.id;
        metadata.status = ProjectStatus::Cancelled;
        metadata.output = None;
        let mut state = manager.read_state(&paths.state).await.unwrap();
        state.brush_complete = false;
        state.brush_training = Default::default();
        state.stage = PipelineStage::ValidatingReconstruction;
        manager.write_state(&paths.state, &state).await.unwrap();
        manager
            .write_metadata(&paths.metadata, &metadata)
            .await
            .unwrap();
        let sphere = paths.colmap.join("sparse/0/images.bin");
        let before = crate::video::panorama::fingerprint(&sphere).unwrap();
        let before_time = std::fs::metadata(&sphere).unwrap().modified().unwrap();
        let cancel_handle = Arc::new(std::sync::Mutex::new(None::<ProcessManager>));
        let observer = cancel_handle.clone();
        let runner = PipelineRunner::new(
            default_engine_paths(Some(workspace.join("engines"))),
            move |event| {
                if event.substage.as_deref() == Some("panoramaTrainingPreparation")
                    && event.current == Some(12)
                {
                    observer.lock().unwrap().as_ref().unwrap().cancel();
                }
            },
        );
        *cancel_handle.lock().unwrap() = Some(runner.process_manager.clone());
        let acceleration = runner.verify_pipeline_engines().await.unwrap();
        let cancelled = runner
            .execute_project(
                manager.clone(),
                paths.clone(),
                &mut metadata,
                state,
                &acceleration,
            )
            .await;
        assert!(matches!(cancelled, Err(SplatError::Cancelled)));
        assert_eq!(metadata.status, ProjectStatus::Cancelled);
        let cache = paths.work.join("panorama-training/images");
        let count = std::fs::read_dir(&cache).unwrap().count();
        assert_eq!(
            count,
            if reuse_faces {
                original_face_batches
            } else {
                2
            }
        );
        assert!(!std::fs::read_dir(&cache).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".partial")));
        let first = std::fs::read_dir(&cache)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path()
            .join("front.png");
        let face_time = std::fs::metadata(&first).unwrap().modified().unwrap();
        let restored = manager.read_state(&paths.state).await.unwrap();
        let image_input = restored.input_type == ProjectInputType::Images;
        assert!(restored.reconstruction_complete);
        assert!(!restored.brush_complete);
        let cancelled_recipe = restored
            .brush_training
            .resolved
            .expect("Recipe saved before preparing faces");
        assert_eq!(cancelled_recipe.panorama.unwrap().recipe_version, 1);
        assert_eq!(cancelled_recipe.preset.total_steps, 12_000);
        let messages = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = messages.clone();
        let captured = Arc::new(std::sync::Mutex::new(Vec::<PipelineEvent>::new()));
        let observed = captured.clone();
        let runner = PipelineRunner::new(
            default_engine_paths(Some(workspace.join("engines"))),
            move |event| {
                observed.lock().unwrap().push(event.clone());
                if event.message.contains("复用") {
                    seen.lock().unwrap().push(event.message);
                }
            },
        );
        let result = runner
            .execute_project(
                manager,
                paths.clone(),
                &mut metadata,
                restored,
                &acceleration,
            )
            .await
            .unwrap();
        assert_eq!(result.registered_images, result.input_images);
        assert!(result.splat_count > 0);
        let completed: PipelineStateFile =
            serde_json::from_slice(&std::fs::read(&paths.state).unwrap()).unwrap();
        assert_eq!(
            completed.brush_training.initial_resolved,
            Some(cancelled_recipe)
        );
        assert_eq!(completed.brush_training.resolved, Some(cancelled_recipe));
        assert_eq!(
            crate::video::panorama::fingerprint(&sphere).unwrap(),
            before
        );
        assert_eq!(
            std::fs::metadata(&sphere).unwrap().modified().unwrap(),
            before_time
        );
        assert_eq!(
            std::fs::metadata(first).unwrap().modified().unwrap(),
            face_time
        );
        let seen = messages.lock().unwrap();
        for text in [
            "帧检查点",
            "特征提取检查点",
            if image_input {
                "穷举匹配检查点"
            } else {
                "顺序匹配检查点"
            },
            "相机重建检查点",
        ] {
            assert!(seen.iter().any(|m| m.contains(text)), "{text}: {seen:?}");
        }
        let events = captured.lock().unwrap();
        std::fs::write(
            workspace.join(".cache/panorama-v1/last-resume-events.json"),
            serde_json::to_vec_pretty(&*events).unwrap(),
        )
        .unwrap();
        for pair in events.windows(2) {
            assert!(
                pair[1].progress >= pair[0].progress,
                "Progress regressed: {:?} {} -> {:?} {}",
                pair[0].kind,
                pair[0].progress,
                pair[1].kind,
                pair[1].progress
            );
        }
        let faces: Vec<_> = events
            .iter()
            .filter(|event| {
                event.unit.as_deref()
                    == Some(if image_input {
                        "panoramaImageFaces"
                    } else {
                        "panoramaFaces"
                    })
            })
            .collect();
        assert!(if image_input {
            events.iter().any(|event| {
                event.kind == EventKind::Log && event.message == "已校验并复用训练缓存"
            })
        } else {
            faces.iter().any(|event| event.message.contains("复用"))
        });
        assert_eq!(
            faces.last().unwrap().current,
            Some(result.registered_images * 6)
        );
        assert_eq!(
            faces.last().unwrap().total,
            Some(result.registered_images * 6)
        );
        assert!(faces.iter().all(|event| event.progress < 66.0));
        let validated = events
            .iter()
            .position(|event| {
                event.substage.as_deref() == Some("panoramaTrainingValidation")
                    && event
                        .stage_progress
                        .is_some_and(|progress| (progress - 600.0 / 38.0).abs() < 0.001)
            })
            .unwrap();
        assert_eq!(events[validated].progress, 66.0);
        assert!(events[validated].current.is_none() && events[validated].total.is_none());
        assert!(events[validated + 1..]
            .iter()
            .any(|event| event.engine == Some(PipelineEngine::Brush)
                && event.current.is_some()
                && event.substage.as_deref() == Some("training")));
        assert!(events
            .iter()
            .filter(|event| event.stage != PipelineStage::Completed)
            .all(|event| event.progress < 100.0));
        eprintln!("Real cancellation/resume verified: {} ERP frames, {} splats; sphere model and completed face batches unchanged",result.input_images,result.splat_count);
    }

    #[test]
    fn reshoot_project_name_uses_the_short_suffix() {
        assert_eq!(reshoot_project_name("示例项目"), "示例项目_补拍");
    }

    #[test]
    fn two_view_tracks_require_automatic_optimization_and_transparency_masks() {
        assert!(should_allow_two_view_tracks(true, true, true));
        assert!(!should_allow_two_view_tracks(false, true, true));
        assert!(!should_allow_two_view_tracks(true, true, false));
        assert!(!should_allow_two_view_tracks(true, false, true));
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

    #[test]
    fn missing_checkpoint_metrics_stay_unknown() {
        let tracker = PlannerEffectivenessTracker::default();
        let runner = PipelineRunner::new(default_engine_paths(None), |_| {})
            .with_effectiveness_tracker(tracker.clone());
        let state = PipelineStateFile::created(Quality::High);
        runner.record_frame_metrics(&state, 30, None);
        runner.record_brush_metrics(&state);
        let snapshot = tracker.snapshot();
        assert!(snapshot.frame_plan.is_none());
        assert!(snapshot.initial_reconstruction.is_none());
        assert!(snapshot.bridge.is_none());
        assert!(snapshot.brush.is_none());
        assert!(snapshot.final_result.is_none());
    }

    #[test]
    fn restored_source_metrics_use_raw_input_and_only_the_new_reshoot_batch() {
        let tracker = PlannerEffectivenessTracker::default();
        let runner = PipelineRunner::new(default_engine_paths(None), |_| {})
            .with_effectiveness_tracker(tracker.clone());
        let mut state = PipelineStateFile::created_for(Quality::High, ProjectInputType::Images);
        state.image_sequence = Some(ImageSequenceInfo {
            width: 8192,
            height: 4096,
            image_count: 12,
            has_alpha: false,
            requires_large_sequence_confirmation: false,
        });
        state.frames = Some(FrameState {
            extracted_frames: Some(112),
            initial_extracted_frames: 112,
            ..Default::default()
        });
        state.reshoot = Some(ReshootState {
            source_image_count: 100,
            reshoot_frame_count: 12,
            supplemental_frames_complete: true,
            ..Default::default()
        });
        runner.restore_source_metrics(&state);
        let source = tracker.snapshot().frame_plan.unwrap();
        assert_eq!((source.source_width, source.source_height), (8192, 4096));
        assert_eq!(source.source_item_count, 12);
        assert_eq!(source.initial_selected_count, 12);
        assert_eq!(source.candidate_count, 12);
        // Before preparation, confirmed source dimensions/counts are still available.
        state.reshoot.as_mut().unwrap().supplemental_frames_complete = false;
        runner.restore_source_metrics(&state);
        assert_eq!(
            tracker
                .snapshot()
                .frame_plan
                .unwrap()
                .initial_selected_count,
            12
        );
        // Old snapshots missing source data cannot borrow combined training counts.
        let empty = PlannerEffectivenessTracker::default();
        let runner = PipelineRunner::new(default_engine_paths(None), |_| {})
            .with_effectiveness_tracker(empty.clone());
        state.image_sequence = None;
        runner.restore_source_metrics(&state);
        assert!(empty.snapshot().frame_plan.is_none());
    }

    #[tokio::test]
    async fn image_preparation_emits_monotonic_phase_progress() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source");
        let frames = temporary.path().join("frames");
        let colmap_frames = temporary.path().join("colmap_frames");
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
            .prepare_images(
                &source,
                Quality::Balanced,
                &frames,
                &colmap_frames,
                &masks,
                false,
            )
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
        use crate::reconstruction::direct_input::{Model, ModelImage, Point};
        use crate::video::panorama::PosedImage;
        Model {
            cameras: vec![ColmapCamera {
                id: 1,
                model_id: 2,
                model: "SIMPLE_RADIAL".into(),
                width: 100,
                height: 100,
                params: vec![100., 50., 50., 0.],
            }],
            images: (1..=registered)
                .map(|id| ModelImage {
                    pose: PosedImage {
                        id: id as u32,
                        camera_id: 1,
                        name: format!("frame_{id:010}.jpg"),
                        qvec: [1., 0., 0., 0.],
                        tvec: [id as f64, 0., 0.],
                    },
                    observations: vec![],
                })
                .collect(),
            points: (1..=points)
                .map(|id| Point {
                    id,
                    xyz: [0., 0., 1.],
                    rgb: [128; 3],
                    error: 0.,
                    track: vec![],
                })
                .collect(),
        }
        .write(root)
        .unwrap();
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
                "CREATE TABLE cameras (camera_id INTEGER PRIMARY KEY, model INTEGER NOT NULL, width INTEGER NOT NULL, height INTEGER NOT NULL, params BLOB NOT NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO cameras(camera_id, model, width, height, params) VALUES (1, 2, 1920, 1080, ?1)",
                [vec![0_u8; 32]],
            )
            .unwrap();
        drop(connection);
        let camera = ColmapCamera {
            id: 1,
            model_id: 2,
            model: "SIMPLE_RADIAL".into(),
            width: 1920,
            height: 1080,
            params: vec![0.0; 4],
        };

        validate_database_camera(&database, &camera).unwrap();
        let connection = rusqlite::Connection::open(&database).unwrap();
        for malformed in [vec![0_u8; 33], vec![0_u8; 31], vec![0_u8; 24], Vec::new()] {
            connection
                .execute("UPDATE cameras SET params = ?1", [&malformed])
                .unwrap();
            assert!(validate_database_camera(&database, &camera).is_err());
        }
        connection
            .execute("UPDATE cameras SET params = ?1", [vec![0_u8; 32]])
            .unwrap();
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
        tokio::fs::create_dir_all(&paths.colmap_frames)
            .await
            .unwrap();
        image::RgbImage::new(2, 2)
            .save(paths.frames.join("frame_000001.jpg"))
            .unwrap();
        image::RgbImage::new(2, 2)
            .save(paths.frames.join("frame_000002.jpg"))
            .unwrap();
        for name in ["frame_000001.jpg", "frame_000002.jpg"] {
            tokio::fs::copy(paths.frames.join(name), paths.colmap_frames.join(name))
                .await
                .unwrap();
        }
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
            colmap_input_version: Some(COLMAP_INPUT_VERSION),
            ..FrameState::default()
        });

        assert!(prepared_frames_from_checkpoint(&paths, &state)
            .await
            .unwrap()
            .is_some());
        state.frames.as_mut().unwrap().colmap_input_version = Some(1);
        assert!(prepared_frames_from_checkpoint(&paths, &state)
            .await
            .unwrap()
            .is_none());
        state.frames.as_mut().unwrap().colmap_input_version = Some(COLMAP_INPUT_VERSION);
        tokio::fs::remove_file(paths.frames.join("frame_000002.jpg"))
            .await
            .unwrap();
        assert!(prepared_frames_from_checkpoint(&paths, &state)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn interrupted_publish_copies_a_valid_orphan_to_a_brush_checkpoint() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::nil(), temporary.path().to_path_buf());
        let valid = b"ply\nformat binary_little_endian 1.0\nelement vertex 1\nproperty float x\nproperty float y\nproperty float z\nproperty float f_dc_0\nproperty float opacity\nproperty float scale_0\nproperty float rot_0\nend_header\n";
        tokio::fs::write(paths.project.join("final.ply"), valid)
            .await
            .unwrap();
        let mut state = PipelineStateFile::created(Quality::Balanced);
        state.brush_complete = true;

        recover_interrupted_publish(&paths, &state).await.unwrap();

        assert_eq!(
            tokio::fs::read(paths.project.join("final.ply"))
                .await
                .unwrap(),
            valid
        );
        assert_eq!(
            tokio::fs::read(paths.brush.join("final.ply.tmp"))
                .await
                .unwrap(),
            valid
        );
    }

    fn png_focal_checkpoint_fixture() -> crate::video::temporary_png_focal_workaround::Checkpoint {
        crate::video::temporary_png_focal_workaround::Checkpoint {
            version: 1,
            image_name: "frame_000001.png".into(),
            sha256: "fixture".into(),
            width: 8192,
            height: 5464,
            camera_params: [22728.66, 4096.0, 2732.0, 0.0],
        }
    }

    #[tokio::test]
    async fn temporary_png_focal_cache_barrier_preserves_images_and_published_files() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::nil(), temp.path().to_path_buf());
        let manager = ProjectManager::for_diagnostics(temp.path().to_path_buf());
        for dir in [
            &paths.frames,
            &paths.colmap_frames,
            &paths.masks,
            &paths.colmap,
            &paths.brush,
        ] {
            tokio::fs::create_dir_all(dir).await.unwrap();
            tokio::fs::write(dir.join("sentinel"), b"original")
                .await
                .unwrap();
        }
        for name in ["final.ply", "edit.ply", "editing.json"] {
            tokio::fs::write(paths.project.join(name), b"published")
                .await
                .unwrap();
        }
        let mut state = PipelineStateFile::created_for(Quality::Balanced, ProjectInputType::Images);
        state.frames = Some(FrameState {
            extracted_frames: Some(2),
            ..FrameState::default()
        });
        state.features_complete = true;
        state.matching_complete = true;
        state.reconstruction_complete = true;
        state.brush_complete = true;
        state.brush_training.oom_retry_used = true;
        state.bridge_backfill.status = BridgeBackfillStatus::Completed;
        let saved_recipe = state.brush_training.clone();
        let saved_bridge = serde_json::to_value(&state.bridge_backfill).unwrap();
        let checkpoint = png_focal_checkpoint_fixture();
        assert!(
            apply_temporary_png_focal_checkpoint(&manager, &paths, &mut state, &checkpoint)
                .await
                .unwrap()
        );
        let saved = manager.read_state(&paths.state).await.unwrap();
        assert!(
            !saved.features_complete
                && !saved.matching_complete
                && !saved.reconstruction_complete
                && !saved.brush_complete
        );
        assert_eq!(saved.temporary_png_focal.as_ref(), Some(&checkpoint));
        assert_eq!(saved.brush_training, saved_recipe);
        assert_eq!(
            serde_json::to_value(&saved.bridge_backfill).unwrap(),
            saved_bridge
        );
        assert_eq!(saved.frames.unwrap().extracted_frames, Some(2));
        assert!(!paths.colmap.exists() && !paths.brush.exists());
        for dir in [&paths.frames, &paths.colmap_frames, &paths.masks] {
            assert_eq!(
                tokio::fs::read(dir.join("sentinel")).await.unwrap(),
                b"original"
            );
        }
        for name in ["final.ply", "edit.ply", "editing.json"] {
            assert_eq!(
                tokio::fs::read(paths.project.join(name)).await.unwrap(),
                b"published"
            );
        }
        let mut archives = tokio::fs::read_dir(paths.work.join("cache-archive"))
            .await
            .unwrap();
        let archive = archives.next_entry().await.unwrap().unwrap().path();
        assert!(archives.next_entry().await.unwrap().is_none());
        for dir in ["colmap", "brush"] {
            assert_eq!(
                tokio::fs::read(archive.join(dir).join("sentinel"))
                    .await
                    .unwrap(),
                b"original"
            );
        }
        // Identical policy/input/params is a true no-op, even with new cached features.
        tokio::fs::create_dir_all(&paths.colmap).await.unwrap();
        tokio::fs::write(paths.colmap.join("database.db"), b"new correct database")
            .await
            .unwrap();
        state.features_complete = true;
        assert!(
            !apply_temporary_png_focal_checkpoint(&manager, &paths, &mut state, &checkpoint)
                .await
                .unwrap()
        );
        assert!(state.features_complete && paths.colmap.join("database.db").exists());
        let mut changed = checkpoint.clone();
        changed.sha256 = "changed".into();
        assert!(
            apply_temporary_png_focal_checkpoint(&manager, &paths, &mut state, &changed)
                .await
                .unwrap()
        );
        assert!(!state.features_complete);
    }

    #[tokio::test]
    async fn temporary_png_focal_archive_failure_keeps_a_durable_reuse_barrier() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::nil(), temp.path().to_path_buf());
        let manager = ProjectManager::for_diagnostics(temp.path().to_path_buf());
        tokio::fs::create_dir_all(&paths.colmap).await.unwrap();
        tokio::fs::write(paths.colmap.join("database.db"), b"old wrong database")
            .await
            .unwrap();
        // A file where the archive directory should be forces a safe failure.
        tokio::fs::write(paths.work.join("cache-archive"), b"blocked")
            .await
            .unwrap();
        let mut state = PipelineStateFile::created_for(Quality::High, ProjectInputType::Images);
        state.features_complete = true;
        state.brush_complete = true;
        let checkpoint = png_focal_checkpoint_fixture();
        assert!(
            apply_temporary_png_focal_checkpoint(&manager, &paths, &mut state, &checkpoint)
                .await
                .is_err()
        );
        let mut saved = manager.read_state(&paths.state).await.unwrap();
        assert!(
            !saved.features_complete
                && !saved.brush_complete
                && saved.temporary_png_focal.is_none()
        );
        assert!(paths.colmap.join("database.db").exists());
        tokio::fs::remove_file(paths.work.join("cache-archive"))
            .await
            .unwrap();
        // Retrying after the interruption cannot certify the old database.
        apply_temporary_png_focal_checkpoint(&manager, &paths, &mut saved, &checkpoint)
            .await
            .unwrap();
        assert!(!paths.colmap.join("database.db").exists());
        assert_eq!(saved.temporary_png_focal, Some(checkpoint));
    }

    #[tokio::test]
    async fn legacy_colmap_input_cache_is_archived_without_touching_published_output() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::nil(), temporary.path().to_path_buf());
        tokio::fs::create_dir_all(&paths.work).await.unwrap();
        for directory in [
            &paths.frames,
            &paths.colmap_frames,
            &paths.masks,
            &paths.colmap,
            &paths.brush,
        ] {
            tokio::fs::create_dir_all(directory).await.unwrap();
            tokio::fs::write(directory.join("sentinel"), b"legacy")
                .await
                .unwrap();
        }
        tokio::fs::write(paths.project.join("final.ply"), b"published")
            .await
            .unwrap();
        let mut state = PipelineStateFile::created(Quality::Balanced);
        state.frames = Some(FrameState {
            extracted_frames: Some(2),
            colmap_input_version: Some(1),
            ..FrameState::default()
        });

        archive_incompatible_colmap_input_cache(&paths, &state)
            .await
            .unwrap();

        for directory in [
            &paths.frames,
            &paths.colmap_frames,
            &paths.masks,
            &paths.colmap,
            &paths.brush,
        ] {
            assert!(!directory.exists());
        }
        assert_eq!(
            tokio::fs::read(paths.project.join("final.ply"))
                .await
                .unwrap(),
            b"published"
        );
        let mut archives = tokio::fs::read_dir(paths.work.join("cache-archive"))
            .await
            .unwrap();
        let archive = archives.next_entry().await.unwrap().unwrap().path();
        assert!(archives.next_entry().await.unwrap().is_none());
        for name in ["frames", "colmap_frames", "masks", "colmap", "brush"] {
            assert_eq!(
                tokio::fs::read(archive.join(name).join("sentinel"))
                    .await
                    .unwrap(),
                b"legacy"
            );
        }
    }

    #[tokio::test]
    async fn legacy_shared_frames_are_admitted_only_after_strict_source_validation() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::nil(), temporary.path().to_path_buf());
        tokio::fs::create_dir_all(&paths.work).await.unwrap();
        let fixture = temporary.path().join("fixture");
        std::fs::create_dir_all(&fixture).unwrap();
        let (input, database) =
            crate::reconstruction::direct_input::tests::legacy_shared_frames_fixture(&fixture);
        tokio::fs::rename(&input.images, &paths.frames)
            .await
            .unwrap();
        tokio::fs::create_dir_all(paths.colmap.join("sparse"))
            .await
            .unwrap();
        let model = paths.colmap.join("sparse/0");
        tokio::fs::rename(&input.model, &model).await.unwrap();
        tokio::fs::rename(database, paths.colmap.join("database.db"))
            .await
            .unwrap();

        let metadata = ProjectMetadata {
            app_id: crate::project::PROJECT_APP_ID.into(),
            status: ProjectStatus::Completed,
            output: Some(ProjectOutput {
                final_ply: paths.project.join("final.ply"),
                file_size: 1,
                splat_count: 1,
                input_images: 3,
                registered_images: 2,
                registered_ratio: 2.0 / 3.0,
                points_3d: 1,
            }),
            ..ProjectMetadata::default()
        };
        let mut state = PipelineStateFile::created(Quality::Balanced);
        state.frames = Some(FrameState {
            extracted_frames: Some(3),
            colmap_input_version: None,
            ..FrameState::default()
        });

        let resolved = resolve_reshoot_colmap_input(
            &paths.project,
            &metadata,
            &state,
            &model,
            &paths.colmap.join("database.db"),
        )
        .await
        .unwrap();
        assert_eq!(resolved.mode, ReshootColmapInputMode::LegacySharedFrames);
        assert_eq!(resolved.colmap_images, paths.frames);

        tokio::fs::create_dir_all(&paths.colmap_frames)
            .await
            .unwrap();
        assert!(resolve_reshoot_colmap_input(
            &paths.project,
            &metadata,
            &state,
            &model,
            &paths.colmap.join("database.db"),
        )
        .await
        .is_err());
        tokio::fs::remove_dir(&paths.colmap_frames).await.unwrap();
        state.frames.as_mut().unwrap().colmap_input_version = Some(1);
        assert!(resolve_reshoot_colmap_input(
            &paths.project,
            &metadata,
            &state,
            &model,
            &paths.colmap.join("database.db"),
        )
        .await
        .is_err());
    }

    #[tokio::test]
    #[ignore = "Set OOOSPLAT_LEGACY_RESHOOT_PROJECT to a completed pre-colmap_frames project; source is read-only"]
    async fn native_legacy_shared_frames_create_a_current_derived_snapshot_without_source_changes()
    {
        let source = PathBuf::from(
            std::env::var_os("OOOSPLAT_LEGACY_RESHOOT_PROJECT")
                .expect("OOOSPLAT_LEGACY_RESHOOT_PROJECT is required"),
        );
        let metadata: ProjectMetadata =
            serde_json::from_slice(&std::fs::read(source.join("project.json")).unwrap()).unwrap();
        let state: PipelineStateFile =
            serde_json::from_slice(&std::fs::read(source.join("state.json")).unwrap()).unwrap();
        let frames = source.join("work/frames");
        let database = source.join("work/colmap/database.db");
        let reconstruction = reusable_reconstruction(&source, &frames, metadata.output.as_ref())
            .await
            .unwrap();
        let mut protected = vec![
            source.join("project.json"),
            source.join("state.json"),
            database.clone(),
            metadata.output.as_ref().unwrap().final_ply.clone(),
        ];
        protected.extend(reusable_model_files(&reconstruction.model).unwrap());
        protected.extend(crate::video::list_images(&frames).unwrap());
        let before = protected
            .iter()
            .map(|path| crate::video::panorama::fingerprint(path).unwrap())
            .collect::<Vec<_>>();

        let resolved = resolve_reshoot_colmap_input(
            &source,
            &metadata,
            &state,
            &reconstruction.model,
            &database,
        )
        .await
        .unwrap();
        assert_eq!(resolved.mode, ReshootColmapInputMode::LegacySharedFrames);
        assert_eq!(resolved.colmap_images, frames);

        let target_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
        let derived = tempfile::tempdir_in(target_root).unwrap();
        let derived_work = derived.path().join("work");
        let derived_frames = derived_work.join("frames");
        let derived_colmap_frames = derived_work.join("colmap_frames");
        let derived_masks = derived_work.join("masks");
        let derived_colmap = derived_work.join("colmap");
        let derived_model = derived_colmap.join("base-model");
        for directory in [&derived_frames, &derived_colmap_frames, &derived_masks] {
            tokio::fs::create_dir_all(directory).await.unwrap();
        }
        tokio::fs::create_dir_all(&derived_model).await.unwrap();
        copy_image_files(&frames, &derived_frames).await.unwrap();
        copy_image_files(&frames, &derived_colmap_frames)
            .await
            .unwrap();
        let derived_database = derived_colmap.join("database.db");
        tokio::fs::copy(&database, &derived_database).await.unwrap();
        for source_model_file in reusable_model_files(&reconstruction.model).unwrap() {
            tokio::fs::copy(
                &source_model_file,
                derived_model.join(source_model_file.file_name().unwrap()),
            )
            .await
            .unwrap();
        }
        reshoot_runner::sync_refined_cameras(&derived_database, &derived_model).unwrap();
        validate_database_camera(
            &derived_database,
            &read_single_camera(&derived_model).unwrap(),
        )
        .unwrap();
        let expected = state.frames.as_ref().unwrap().extracted_frames.unwrap();
        let mut derived_state = PipelineStateFile::created(Quality::Balanced);
        derived_state.frames = Some(FrameState {
            extracted_frames: Some(expected),
            colmap_input_version: Some(COLMAP_INPUT_VERSION),
            ..FrameState::default()
        });
        tokio::fs::write(
            derived.path().join("state.json"),
            serde_json::to_vec_pretty(&derived_state).unwrap(),
        )
        .await
        .unwrap();
        let reloaded: PipelineStateFile = serde_json::from_slice(
            &tokio::fs::read(derived.path().join("state.json"))
                .await
                .unwrap(),
        )
        .unwrap();
        validate_current_colmap_input_snapshot(derived.path(), &reloaded)
            .await
            .unwrap();
        assert_eq!(
            reloaded
                .frames
                .as_ref()
                .and_then(|frames| frames.colmap_input_version),
            Some(COLMAP_INPUT_VERSION)
        );

        let after = protected
            .iter()
            .map(|path| crate::video::panorama::fingerprint(path).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            before, after,
            "legacy source files must remain byte-identical"
        );
    }

    #[tokio::test]
    async fn reshoot_baseline_requires_a_complete_current_colmap_image_snapshot() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::nil(), temporary.path().to_path_buf());
        tokio::fs::create_dir_all(&paths.frames).await.unwrap();
        tokio::fs::create_dir_all(&paths.colmap_frames)
            .await
            .unwrap();
        for name in ["frame_000001.png", "frame_000002.png"] {
            image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]))
                .save(paths.frames.join(name))
                .unwrap();
            tokio::fs::copy(paths.frames.join(name), paths.colmap_frames.join(name))
                .await
                .unwrap();
        }
        let mut state = PipelineStateFile::created_for(Quality::Balanced, ProjectInputType::Images);
        state.frames = Some(FrameState {
            extracted_frames: Some(2),
            has_alpha: false,
            colmap_input_version: Some(COLMAP_INPUT_VERSION),
            ..FrameState::default()
        });

        state.frames.as_mut().unwrap().colmap_input_version = Some(1);
        assert!(
            validate_current_colmap_input_snapshot(&paths.project, &state)
                .await
                .is_err()
        );
        state.frames.as_mut().unwrap().colmap_input_version = Some(COLMAP_INPUT_VERSION);
        validate_current_colmap_input_snapshot(&paths.project, &state)
            .await
            .unwrap();
        tokio::fs::remove_file(paths.colmap_frames.join("frame_000002.png"))
            .await
            .unwrap();
        assert!(
            validate_current_colmap_input_snapshot(&paths.project, &state)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn reshoot_three_way_merge_preflights_and_moves_every_image_set() {
        let temporary = tempfile::tempdir().unwrap();
        let extracted_frames = temporary.path().join("extracted-frames");
        let extracted_colmap = temporary.path().join("extracted-colmap");
        let extracted_masks = temporary.path().join("extracted-masks");
        let frames = temporary.path().join("frames");
        let colmap_frames = temporary.path().join("colmap-frames");
        let masks = temporary.path().join("masks");
        for directory in [
            &extracted_frames,
            &extracted_colmap,
            &extracted_masks,
            &frames,
            &colmap_frames,
            &masks,
        ] {
            tokio::fs::create_dir_all(directory).await.unwrap();
        }
        for index in 1..=2 {
            let name = format!("frame_{index:06}.png");
            image::RgbaImage::new(2, 2)
                .save(extracted_frames.join(&name))
                .unwrap();
            tokio::fs::copy(extracted_frames.join(&name), extracted_colmap.join(&name))
                .await
                .unwrap();
            image::GrayImage::new(2, 2)
                .save(extracted_masks.join(format!("{name}.png")))
                .unwrap();
        }
        tokio::fs::write(frames.join("reshoot_000002.png"), b"collision")
            .await
            .unwrap();

        assert!(move_extracted_reshoot(
            &extracted_frames,
            &extracted_colmap,
            &extracted_masks,
            &frames,
            &colmap_frames,
            &masks,
            true,
        )
        .await
        .is_err());
        assert!(extracted_frames.join("frame_000001.png").is_file());
        assert!(!frames.join("reshoot_000001.png").exists());

        tokio::fs::remove_file(frames.join("reshoot_000002.png"))
            .await
            .unwrap();
        move_extracted_reshoot(
            &extracted_frames,
            &extracted_colmap,
            &extracted_masks,
            &frames,
            &colmap_frames,
            &masks,
            true,
        )
        .await
        .unwrap();
        for index in 1..=2 {
            let name = format!("reshoot_{index:06}.png");
            assert!(frames.join(&name).is_file());
            assert!(colmap_frames.join(&name).is_file());
            assert!(masks.join(format!("{name}.png")).is_file());
            assert_eq!(
                tokio::fs::read(frames.join(&name)).await.unwrap(),
                tokio::fs::read(colmap_frames.join(&name)).await.unwrap()
            );
        }
    }

    #[tokio::test]
    async fn image_sequence_checkpoint_requires_every_image_and_mask() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::nil(), temporary.path().to_path_buf());
        tokio::fs::create_dir_all(&paths.frames).await.unwrap();
        tokio::fs::create_dir_all(&paths.colmap_frames)
            .await
            .unwrap();
        tokio::fs::create_dir_all(&paths.masks).await.unwrap();
        for name in ["frame_000001.png", "frame_000002.jpg"] {
            if name.ends_with(".png") {
                image::RgbaImage::new(2, 2)
                    .save(paths.frames.join(name))
                    .unwrap();
            } else {
                image::RgbImage::new(2, 2)
                    .save(paths.frames.join(name))
                    .unwrap();
            }
            tokio::fs::copy(paths.frames.join(name), paths.colmap_frames.join(name))
                .await
                .unwrap();
            image::GrayImage::new(2, 2)
                .save(paths.masks.join(format!("{name}.png")))
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
            colmap_input_version: Some(COLMAP_INPUT_VERSION),
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
        tokio::fs::create_dir_all(&paths.colmap_frames)
            .await
            .unwrap();
        for name in ["frame_000001.png", "frame_000002.png"] {
            image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]))
                .save(paths.frames.join(name))
                .unwrap();
            tokio::fs::copy(paths.frames.join(name), paths.colmap_frames.join(name))
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
            colmap_input_version: Some(COLMAP_INPUT_VERSION),
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
        tokio::fs::create_dir_all(&paths.colmap_frames)
            .await
            .unwrap();
        tokio::fs::create_dir_all(&paths.masks).await.unwrap();
        image::RgbaImage::new(2, 2)
            .save(paths.frames.join("frame_000001.png"))
            .unwrap();
        tokio::fs::copy(
            paths.frames.join("frame_000001.png"),
            paths.colmap_frames.join("frame_000001.png"),
        )
        .await
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
            colmap_input_version: Some(COLMAP_INPUT_VERSION),
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
        tokio::fs::create_dir_all(&paths.colmap_frames)
            .await
            .unwrap();
        tokio::fs::create_dir_all(&paths.colmap).await.unwrap();
        image::RgbImage::new(2, 2)
            .save(paths.frames.join("frame_000001.jpg"))
            .unwrap();
        tokio::fs::copy(
            paths.frames.join("frame_000001.jpg"),
            paths.colmap_frames.join("frame_000001.jpg"),
        )
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
            colmap_input_version: Some(COLMAP_INPUT_VERSION),
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
            parse_bracket_progress("Processed image [4/10]"),
            Some((4, 10))
        );
        assert_eq!(parse_bracket_progress("Processing image [4/10]"), None);
        assert_eq!(parse_bracket_progress("Matching block [4/10]"), None);
        assert_eq!(parse_bracket_progress("bundle iteration [4/10]"), None);
    }

    #[test]
    fn undistortion_progress_is_not_matching_or_training_iterations() {
        assert_eq!(
            parse_undistortion_progress("I123 undistorters.cc:208] Undistorting image [3/251]"),
            Some((3, 251))
        );
        for line in [
            "Indexing image [3/251]",
            "Matching block [3/251]",
            "Processed image [3/251]",
            "Undistorting image [0/251]",
            "Undistorting image [252/251]",
            "Undistorting image [3/0]",
            "Undistorting image [bad/251]",
        ] {
            assert_eq!(parse_undistortion_progress(line), None, "{line}");
        }
        let events = Arc::new(std::sync::Mutex::new(Vec::<PipelineEvent>::new()));
        let capture = events.clone();
        let runner = PipelineRunner::new(default_engine_paths(None), move |e| {
            capture.lock().unwrap().push(e)
        });
        runner.events.training_substage("trainingPreparation");
        let observer = runner.process_observer(
            PipelineStage::TrainingSplats,
            PipelineEngine::Colmap,
            Some(251),
            ObserverMode::TrainingPreparation,
        );
        observer(ProcessUpdate::Started { process_id: 17 });
        for current in [1, 2, 3, 251] {
            observer(ProcessUpdate::Line {
                stream: crate::process::ProcessStream::Stderr,
                line: format!("Undistorting image [{current}/251]"),
            });
        }
        let captured = events.lock().unwrap();
        let progress = captured
            .iter()
            .filter(|e| e.kind == EventKind::Progress)
            .collect::<Vec<_>>();
        assert_eq!(progress.len(), 2, "Only start/end bypass the 1 Hz limit");
        for event in progress {
            assert_eq!(event.substage.as_deref(), Some("trainingPreparation"));
            assert_eq!(event.unit.as_deref(), Some("images"));
            assert!(event.stage_progress.is_none());
            assert!(event.runtime.is_none());
        }
    }

    #[test]
    fn generic_training_preparation_never_uses_panorama_labels_or_reaches_100_percent() {
        let events = Arc::new(std::sync::Mutex::new(Vec::<PipelineEvent>::new()));
        let capture = events.clone();
        let runner = PipelineRunner::new(default_engine_paths(None), move |e| {
            capture.lock().unwrap().push(e)
        });
        for substage in [
            "trainingPreparation",
            "trainingValidation",
            "trainingCheckpoint",
            "training",
        ] {
            runner.events.training_substage(substage);
            runner.events.stage(
                PipelineStage::TrainingSplats,
                if substage == "training" { 0.0 } else { 1.0 },
                substage,
            );
        }
        let captured = events.lock().unwrap();
        assert_eq!(
            captured.iter().map(|e| e.progress).collect::<Vec<_>>(),
            vec![66.0; 4]
        );
        for event in captured.iter() {
            assert!(!event.substage.as_deref().unwrap().contains("panorama"));
            assert!(event.stage_progress.unwrap() < 100.0);
        }
        runner.events.panorama_phase.store(8, Ordering::Relaxed);
        assert_eq!(
            runner
                .events
                .substage(PipelineStage::ExtractingFrames)
                .as_deref(),
            Some("videoTimestamps")
        );
        runner.events.panorama_phase.store(4, Ordering::Relaxed);
        assert_eq!(
            runner
                .events
                .substage(PipelineStage::ExtractingFrames)
                .as_deref(),
            Some("panoramaTimestamps")
        );
    }

    #[test]
    fn diagnostic_log_observer_binds_the_run_operation_and_failed_native_status() {
        use std::io::Write;
        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("project"));
        std::fs::create_dir_all(&paths.logs).unwrap();
        let runner = PipelineRunner::new(default_engine_paths(None), |_| {})
            .with_execution_run_id(Some(uuid::Uuid::new_v4()));
        *runner.active_project.lock().unwrap() = Some(ActiveProjectContext {
            project_id: paths.id,
            workspace_task_id: None,
            project_path: paths.project.clone(),
            logs_directory: paths.logs.clone(),
        });
        let path = paths.logs.join("gps.log");
        std::fs::write(&path, "old visual attempt\n").unwrap();
        let observer = runner.process_observer(
            PipelineStage::Reconstructing,
            PipelineEngine::Colmap,
            None,
            ObserverMode::Mapper,
        );
        observer(ProcessUpdate::Started { process_id: 17 });
        let start = std::fs::metadata(&path).unwrap().len();
        observer(ProcessUpdate::LogOpened {
            path: path.clone(),
            offset: start,
        });
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(b"current GPS failure\nexit_code: Some(7)\n")
            .unwrap();
        let termination = crate::process::ProcessTermination {
            exit_code: Some(7),
            ..Default::default()
        };
        observer(ProcessUpdate::LogClosed {
            path: path.clone(),
            end: file.metadata().unwrap().len(),
            success: false,
            termination: termination.clone(),
        });
        drop(file);
        let output = crate::process::ProcessOutput {
            success: false,
            exit_code: Some(7),
            termination,
            stdout: String::new(),
            stderr: "current GPS failure".into(),
        };
        let failure =
            SplatError::EngineProcess(Box::new(crate::failure::EngineProcessFailure::from_output(
                PipelineEngine::Colmap,
                "pose_prior_mapper",
                &output,
            )));
        let frozen = runner
            .freeze_failure_log(&failure, Some(PipelineEngine::Colmap))
            .unwrap();
        assert!(frozen.text.contains("current GPS failure"));
        assert!(frozen.text.contains("exit_code: Some(7)"));
        assert!(!frozen.text.contains("old visual"));
        std::fs::write(&path, "later overwrite\n".repeat(100)).unwrap();
        assert_eq!(
            runner
                .freeze_failure_log(&failure, Some(PipelineEngine::Colmap))
                .unwrap()
                .text,
            frozen.text
        );
        assert!(runner
            .freeze_failure_log(
                &SplatError::Process("application validation failed".into()),
                Some(PipelineEngine::Colmap)
            )
            .is_none());
        assert!(runner
            .freeze_failure_log(
                &SplatError::Io(std::io::Error::other("write state")),
                Some(PipelineEngine::Colmap)
            )
            .is_none());
        let mut different = output.clone();
        different.termination.exit_code = Some(8);
        let different =
            SplatError::EngineProcess(Box::new(crate::failure::EngineProcessFailure::from_output(
                PipelineEngine::Colmap,
                "pose_prior_mapper",
                &different,
            )));
        assert!(runner
            .freeze_failure_log(&different, Some(PipelineEngine::Colmap))
            .is_none());

        // A new successful process cannot lend its log to a later validation
        // error, nor fall back to this operation's previous failed process.
        observer(ProcessUpdate::Started { process_id: 18 });
        let start = std::fs::metadata(&path).unwrap().len();
        observer(ProcessUpdate::LogOpened {
            path: path.clone(),
            offset: start,
        });
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(b"successful process\nexit_code: Some(0)\n")
            .unwrap();
        observer(ProcessUpdate::LogClosed {
            path,
            end: file.metadata().unwrap().len(),
            success: true,
            termination: crate::process::ProcessTermination {
                exit_code: Some(0),
                ..Default::default()
            },
        });
        assert!(runner
            .freeze_failure_log(&failure, Some(PipelineEngine::Colmap))
            .is_none());

        let ffmpeg = runner.process_observer(
            PipelineStage::ExtractingFrames,
            PipelineEngine::Ffmpeg,
            None,
            ObserverMode::Ffmpeg,
        );
        let path = paths.logs.join("joint-bridge-frames.log");
        std::fs::write(&path, b"").unwrap();
        ffmpeg(ProcessUpdate::Started { process_id: 19 });
        ffmpeg(ProcessUpdate::LogOpened {
            path: path.clone(),
            offset: 0,
        });
        std::fs::write(&path, "failed extraction\nexit_code: Some(7)\n").unwrap();
        ffmpeg(ProcessUpdate::LogClosed {
            path: path.clone(),
            end: std::fs::metadata(&path).unwrap().len(),
            success: false,
            termination: crate::process::ProcessTermination {
                exit_code: Some(7),
                ..Default::default()
            },
        });
        let legacy_error = SplatError::Process("FFmpeg extraction failed".into());
        assert!(runner
            .freeze_failure_log(&legacy_error, Some(PipelineEngine::Ffmpeg))
            .unwrap()
            .text
            .contains("failed extraction"));
        assert!(runner
            .freeze_failure_log(&SplatError::Cancelled, Some(PipelineEngine::Ffmpeg))
            .is_none());
        assert!(runner
            .freeze_failure_log(
                &SplatError::EngineStart {
                    engine: "ffmpeg".into(),
                    detail: "launch failed".into(),
                },
                Some(PipelineEngine::Ffmpeg)
            )
            .is_none());
        // A new process without an opened log cannot inherit the last failed
        // process, even though its stage and engine are unchanged.
        ffmpeg(ProcessUpdate::Started { process_id: 20 });
        assert!(runner
            .freeze_failure_log(&legacy_error, Some(PipelineEngine::Ffmpeg))
            .is_none());
    }

    #[test]
    fn matching_silence_notice_has_a_15_second_threshold_and_30_second_limit() {
        let last = AtomicU64::new(0);
        assert!(!matching_quiet_notice_due(&last, 14_999, 14_999));
        assert!(matching_quiet_notice_due(&last, 15_000, 15_000));
        assert!(!matching_quiet_notice_due(&last, 15_000, 15_000));
        assert!(!matching_quiet_notice_due(&last, 44_999, 44_999));
        assert!(matching_quiet_notice_due(&last, 45_000, 45_000));
        assert!(!matching_quiet_notice_due(&last, 99_000, 100));
        last.store(0, Ordering::Relaxed); // An actual output line or fresh process resets silence.
        assert!(!matching_quiet_notice_due(&last, 100_000, 14_999));
        assert!(matching_quiet_notice_due(&last, 100_001, 15_000));
    }

    #[test]
    fn engine_display_gate_limits_updates_and_keeps_the_last_count_and_final_line() {
        let count = |current| EngineDisplay {
            kind: EventKind::Progress,
            level: EventLevel::Info,
            progress: None,
            message: format!("count {current}"),
            current: Some(current),
            total: Some(100),
            unit: Some("images"),
            deduplicate_count: true,
        };
        let log = |message: &str, level| EngineDisplay {
            kind: EventKind::Log,
            level,
            progress: None,
            message: message.into(),
            current: None,
            total: None,
            unit: None,
            deduplicate_count: false,
        };
        let at = Instant::now();
        let mut gate = EngineDisplayGate::default();
        assert_eq!(gate.submit(count(1), at, true).len(), 1);
        for current in 2..=5 {
            assert!(gate
                .submit(
                    count(current),
                    at + std::time::Duration::from_millis(100),
                    false
                )
                .is_empty());
        }
        assert!(gate
            .submit(log("Elapsed time: 1s", EventLevel::Info), at, false)
            .is_empty());
        assert!(gate
            .flush(at + std::time::Duration::from_millis(999), false)
            .is_empty());
        let ready = gate.flush(at + std::time::Duration::from_secs(1), true);
        assert_eq!(ready.len(), 2);
        assert_eq!(ready[0].current, Some(5));
        assert_eq!(ready[1].message, "Elapsed time: 1s");
        assert_eq!(
            gate.submit(count(100), at + std::time::Duration::from_secs(1), true)
                .len(),
            1
        );
        assert!(gate
            .submit(count(100), at + std::time::Duration::from_secs(1), true)
            .is_empty());
        assert_eq!(
            gate.submit(log("Check failed: camera", EventLevel::Error), at, false)
                .len(),
            1
        );
        let mut fresh = EngineDisplayGate::default();
        assert_eq!(
            fresh
                .submit(log("Training config", EventLevel::Info), at, false)
                .len(),
            1
        );
        // The first native report may already be step 60 (or any count), not
        // step one. Starting/retrying must expose it immediately.
        assert_eq!(
            fresh
                .submit(count(60), at + std::time::Duration::from_millis(10), false)
                .len(),
            1
        );
    }

    #[test]
    fn native_failures_keep_their_level_and_do_not_become_counts_or_duplicate_messages() {
        for metric in [
            "Initial reprojection error 0.12",
            "Final reprojection error 0.08",
            "Mean reprojection error 0.12",
            "Mean error 0.12",
            "Initial error 0.12",
            "I20261111 12:00:00.000000 12 bundle_adjustment.cc:1] Mean reprojection error 0.12",
            "[2026-11-11T04:00:00Z INFO bundle_adjustment] Final reprojection error 0.08",
            "Initial warning count 0",
        ] {
            assert_eq!(engine_line_level(metric), EventLevel::Info, "{metric}");
        }
        for error in [
            "[ERROR] native failure",
            "[ERROR brush_process] native failure",
            "[2026-11-11T04:00:00Z ERROR brush_process] native failure",
            "2026-11-11T04:00:00Z ERROR brush_process: native failure",
            "ERROR native failure",
        ] {
            assert_eq!(engine_line_level(error), EventLevel::Error, "{error}");
        }
        for warning in [
            "[WARN] native warning",
            "[2026-11-11T04:00:00Z WARN brush_process] native warning",
        ] {
            assert_eq!(engine_line_level(warning), EventLevel::Warning, "{warning}");
        }
        let captured = Arc::new(std::sync::Mutex::new(Vec::<PipelineEvent>::new()));
        let output = captured.clone();
        let runner = PipelineRunner::new(default_engine_paths(None), move |event| {
            output.lock().unwrap().push(event)
        });
        let observer = runner.process_observer(
            PipelineStage::Matching,
            PipelineEngine::Colmap,
            Some(10),
            ObserverMode::Matching,
        );
        observer(ProcessUpdate::Started { process_id: 42 });
        for line in [
            "Loop detection: Indexing image [1/10]",
            "Failed to create any sparse model",
            "F20261111 12:00:00.000000 12 mapper.cc:1] Check failed: camera_id > 0",
            "wgpu error: Out of Memory",
            "W20261111 12:00:00.000000 12 mapper.cc:2] warning: Processed image [9/10]",
        ] {
            observer(ProcessUpdate::Line {
                stream: crate::process::ProcessStream::Stderr,
                line: line.into(),
            });
        }
        observer(ProcessUpdate::Line {
            stream: crate::process::ProcessStream::Stderr,
            line: "Loading database".into(),
        });
        observer(ProcessUpdate::Exited {
            executable: PathBuf::from("colmap"),
            exit_code: Some(1),
        });
        let events = captured.lock().unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.message.contains("Loop detection: Indexing"))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.kind == EventKind::Progress)
                .count(),
            1
        );
        for marker in ["Failed to create", "Check failed:", "Out of Memory"] {
            let event = events
                .iter()
                .find(|event| event.message.contains(marker))
                .unwrap();
            assert_eq!(event.kind, EventKind::Log);
            assert_eq!(event.level, EventLevel::Error);
            assert!(event.current.is_none() && event.stage_progress.is_none());
        }
        assert_eq!(
            events
                .iter()
                .find(|event| event.message.contains("warning:"))
                .unwrap()
                .level,
            EventLevel::Warning
        );
        assert_eq!(
            events
                .iter()
                .find(|event| event.message == "Loading database")
                .unwrap()
                .level,
            EventLevel::Info
        );
        assert!(events
            .iter()
            .all(|event| event.stage == PipelineStage::Matching));
    }

    #[test]
    fn real_ordinary_and_alpha_log_excerpts_keep_marker_counts_and_unknown_matching_percentage() {
        // De-identified excerpts from the successful 285-image Balanced and
        // 251-image Alpha Fast runs. No source paths, names or coordinates.
        for (total, feature, indexed, matched) in [
            (285, "I20261111 11:45:34.366609 45884 feature_extraction.cc:270] Processed file [285/285]",
                "I20261111 11:49:51.560301 18164 pairing.cc:372] Indexing image [285/285]",
                "I20261111 11:53:32.660749 18164 pairing.cc:333] Processing image [285/285]"),
            (251, "I20261110 10:17:11.142657 28220 feature_extraction.cc:270] Processed file [251/251]",
                "I20261110 10:18:03.845498 16184 pairing.cc:372] Indexing image [251/251]",
                "I20261110 10:19:07.375516 16184 pairing.cc:333] Processing image [251/251]"),
        ] {
            assert_eq!(parse_bracket_progress(feature), Some((total, total)));
            let captured = Arc::new(std::sync::Mutex::new(Vec::<PipelineEvent>::new()));
            let output = captured.clone();
            let runner = PipelineRunner::new(default_engine_paths(None), move |event| output.lock().unwrap().push(event));
            let observer = runner.process_observer(PipelineStage::Matching, PipelineEngine::Colmap, Some(total), ObserverMode::Matching);
            observer(ProcessUpdate::Started { process_id: 42 });
            for line in ["Generating image pairs with vocabulary tree...", indexed, matched] {
                observer(ProcessUpdate::Line { stream: crate::process::ProcessStream::Stderr, line: line.into() });
            }
            let events = captured.lock().unwrap();
            for unit in ["matchingIndexImages", "matchingImages"] {
                let event = events.iter().find(|event| event.unit.as_deref() == Some(unit)).unwrap();
                assert_eq!((event.current, event.total), (Some(total), Some(total)));
                assert!(event.indeterminate && event.stage_progress.is_none());
            }
            assert_eq!(events.iter().filter(|event| event.kind == EventKind::Progress).count(), 3);
        }
        assert_eq!(
            parse_bracket_progress("[prefix] Processed file [23/90]"),
            Some((23, 90))
        );
        assert_eq!(
            parse_matching_progress("[prefix] Indexing image [7/90]"),
            Some((7, 90))
        );
        assert!(parse_matching_progress("Indexing image [91/90]").is_none());
        assert!(parse_matching_progress("Indexing image [1/0]").is_none());
        assert!(matching_activity("Skipping loop detection").is_none());
    }

    #[test]
    fn engine_runtime_deduplicates_noise_but_preserves_final_state_and_visible_quiet_notices() {
        let captured = Arc::new(std::sync::Mutex::new(Vec::<PipelineEvent>::new()));
        let output = captured.clone();
        let runner = PipelineRunner::new(default_engine_paths(None), move |event| {
            output.lock().unwrap().push(event)
        });
        let observer = runner.process_observer(
            PipelineStage::Reconstructing,
            PipelineEngine::Colmap,
            None,
            ObserverMode::Generic,
        );
        observer(ProcessUpdate::Started { process_id: 42 });
        for _ in 0..100 {
            observer(ProcessUpdate::Line {
                stream: crate::process::ProcessStream::Stdout,
                line: "uninteresting native detail".into(),
            });
        }
        assert_eq!(
            captured
                .lock()
                .unwrap()
                .iter()
                .filter(|event| event.kind == EventKind::Runtime)
                .count(),
            1
        );
        observer(ProcessUpdate::Heartbeat { elapsed_ms: 15_000 });
        observer(ProcessUpdate::Heartbeat { elapsed_ms: 16_000 });
        observer(ProcessUpdate::Heartbeat { elapsed_ms: 45_000 });
        assert_eq!(
            captured
                .lock()
                .unwrap()
                .iter()
                .filter(|event| event.message == "相机重建。此步骤可能耗时较长，请耐心等待")
                .count(),
            2
        );
        observer(ProcessUpdate::Line {
            stream: crate::process::ProcessStream::Stderr,
            line: "Warning: native warning".into(),
        });
        observer(ProcessUpdate::Heartbeat { elapsed_ms: 60_000 });
        assert_eq!(
            captured
                .lock()
                .unwrap()
                .iter()
                .filter(|event| event.message == "相机重建。此步骤可能耗时较长，请耐心等待")
                .count(),
            2
        );
        observer(ProcessUpdate::Exited {
            executable: PathBuf::from("colmap"),
            exit_code: Some(0),
        });
        let brush = runner.process_observer(
            PipelineStage::TrainingSplats,
            PipelineEngine::Brush,
            Some(100),
            ObserverMode::Brush,
        );
        brush(ProcessUpdate::Started { process_id: 43 });
        for _ in 0..100 {
            brush(ProcessUpdate::Line {
                stream: crate::process::ProcessStream::Stdout,
                line: "Training config: total_iters=100 start_iter=0".into(),
            });
        }
        brush(ProcessUpdate::Line {
            stream: crate::process::ProcessStream::Stdout,
            line: "Training progress: iteration=30 total=100 lod=0".into(),
        });
        brush(ProcessUpdate::Exited {
            executable: PathBuf::from("brush"),
            exit_code: Some(0),
        });
        let events = captured.lock().unwrap();
        let brush_runtime = events
            .iter()
            .filter(|event| event.engine == Some(PipelineEngine::Brush))
            .filter_map(|event| event.runtime.as_ref())
            .collect::<Vec<_>>();
        assert!(brush_runtime.len() <= 4);
        assert_eq!(
            brush_runtime
                .last()
                .unwrap()
                .training
                .as_ref()
                .unwrap()
                .iteration,
            Some(30)
        );
        assert!(events.iter().any(|event| event.current == Some(30)));
    }

    #[test]
    fn matching_notices_are_system_logs_without_progress_and_stop_on_exit_or_cancel() {
        let received = Arc::new(std::sync::Mutex::new(Vec::<PipelineEvent>::new()));
        let out = received.clone();
        let runner = PipelineRunner::new(default_engine_paths(None), move |e| {
            out.lock().unwrap().push(e)
        });
        let cancelled = Arc::new(std::sync::Mutex::new(Vec::<PipelineEvent>::new()));
        let out = cancelled.clone();
        let cancelled_runner = PipelineRunner::new(default_engine_paths(None), move |e| {
            out.lock().unwrap().push(e)
        });
        let observer = runner.process_observer(
            PipelineStage::Matching,
            PipelineEngine::Colmap,
            Some(86),
            ObserverMode::Matching,
        );
        let cancelled_observer = cancelled_runner.process_observer(
            PipelineStage::Matching,
            PipelineEngine::Colmap,
            Some(86),
            ObserverMode::Matching,
        );
        observer(ProcessUpdate::Started { process_id: 42 });
        cancelled_observer(ProcessUpdate::Started { process_id: 43 });
        let initial_progress = received.lock().unwrap().last().unwrap().progress;
        // One real quiet window tests the production Instant-based clock, not a mocked age.
        std::thread::sleep(std::time::Duration::from_millis(15_025));
        observer(ProcessUpdate::Heartbeat { elapsed_ms: 15_025 });
        observer(ProcessUpdate::Heartbeat { elapsed_ms: 16_000 });
        observer(ProcessUpdate::Heartbeat { elapsed_ms: 45_025 });
        let notices = || {
            received
                .lock()
                .unwrap()
                .iter()
                .filter(|e| e.message.starts_with("图像匹配进程仍在运行"))
                .count()
        };
        assert_eq!(notices(), 2);
        {
            let events = received.lock().unwrap();
            for event in events
                .iter()
                .filter(|e| e.message.starts_with("图像匹配进程仍在运行"))
            {
                assert_eq!(event.kind, EventKind::Log);
                assert_eq!(event.engine, Some(PipelineEngine::System));
                assert_eq!(event.progress, initial_progress);
                assert!(event.stage_progress.is_none());
                assert!(event.current.is_none() && event.total.is_none());
            }
        }
        observer(ProcessUpdate::Exited {
            executable: PathBuf::from("colmap"),
            exit_code: Some(0),
        });
        observer(ProcessUpdate::Heartbeat { elapsed_ms: 75_025 });
        assert_eq!(notices(), 2);
        cancelled_runner.process_manager.cancel();
        cancelled_observer(ProcessUpdate::Heartbeat { elapsed_ms: 15_025 });
        assert!(!cancelled
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.message.starts_with("图像匹配进程仍在运行")));
        observer(ProcessUpdate::Line {
            stream: crate::process::ProcessStream::Stdout,
            line: "Indexing image [1/86]".into(),
        });
        observer(ProcessUpdate::Heartbeat { elapsed_ms: 90_000 });
        assert_eq!(notices(), 2);
    }

    #[test]
    #[ignore = "Requires OOOSPLAT_MATCHING_LOG, an existing real COLMAP matching log"]
    fn replay_real_matching_log_keeps_index_counts_and_unknown_stage_percentage() {
        let path = std::env::var_os("OOOSPLAT_MATCHING_LOG").expect("Set a real COLMAP log");
        let source = std::fs::read_to_string(path).unwrap();
        let captured = Arc::new(std::sync::Mutex::new(Vec::<PipelineEvent>::new()));
        let out = captured.clone();
        let runner = PipelineRunner::new(default_engine_paths(None), move |event| {
            out.lock().unwrap().push(event)
        });
        let observer = runner.process_observer(
            PipelineStage::Matching,
            PipelineEngine::Colmap,
            None,
            ObserverMode::Matching,
        );
        observer(ProcessUpdate::Started { process_id: 42 });
        let mut expected_index_lines = 0;
        let mut last_index_count = None;
        for line in source.lines().filter(|line| {
            parse_matching_progress(line).is_some() || matching_activity(line).is_some()
        }) {
            expected_index_lines += usize::from(line.contains("Indexing image ["));
            if line.contains("Indexing image [") {
                last_index_count = parse_matching_progress(line);
            }
            observer(ProcessUpdate::Line {
                stream: crate::process::ProcessStream::Stderr,
                line: line.into(),
            });
        }
        observer(ProcessUpdate::Exited {
            executable: PathBuf::from("colmap"),
            exit_code: Some(0),
        });
        let captured = captured.lock().unwrap();
        let indices: Vec<_> = captured
            .iter()
            .filter(|e| e.unit.as_deref() == Some("matchingIndexImages"))
            .collect();
        assert!(expected_index_lines > 0);
        assert!(!indices.is_empty() && indices.len() <= expected_index_lines);
        assert_eq!(
            indices
                .last()
                .map(|event| (event.current.unwrap(), event.total.unwrap())),
            last_index_count
        );
        assert!(indices
            .iter()
            .all(|e| e.current.is_some() && e.stage_progress.is_none() && e.indeterminate));
        println!(
            "Real matching replay: {} indexing lines retained; last count {:?}/{:?}",
            indices.len(),
            indices.last().unwrap().current,
            indices.last().unwrap().total
        );
        if let Some(path) = std::env::var_os("OOOSPLAT_MATCHING_EVENTS") {
            std::fs::write(path, serde_json::to_vec(&*captured).unwrap()).unwrap();
        }
    }

    #[test]
    fn panorama_process_context_is_stable_and_matching_counts_do_not_estimate_completion() {
        let captured = Arc::new(std::sync::Mutex::new(Vec::<PipelineEvent>::new()));
        let output = captured.clone();
        let runner = PipelineRunner::new(default_engine_paths(None), move |event| {
            output.lock().unwrap().push(event)
        });
        runner.events.panorama_phase.store(1, Ordering::Relaxed);
        let old = runner.process_observer(
            PipelineStage::Reconstructing,
            PipelineEngine::Colmap,
            Some(100),
            ObserverMode::Mapper,
        );
        old(ProcessUpdate::Started { process_id: 42 });
        old(ProcessUpdate::Line {
            stream: crate::process::ProcessStream::Stdout,
            line: "Registering image #50 (num_reg_frames=40)".into(),
        });
        old(ProcessUpdate::Heartbeat { elapsed_ms: 1000 });
        {
            let events = captured.lock().unwrap();
            assert!(events
                .iter()
                .all(|event| event.substage.as_deref() == Some("panoramaReconstruction")));
            let progress = events
                .iter()
                .find(|event| event.kind == EventKind::Progress)
                .unwrap();
            assert!(progress.indeterminate && progress.stage_progress.is_none());
            assert_eq!(progress.current, Some(40));
        }
        let retry = runner.process_observer(
            PipelineStage::Reconstructing,
            PipelineEngine::Colmap,
            Some(100),
            ObserverMode::Mapper,
        );
        retry(ProcessUpdate::Started { process_id: 43 });
        let count = captured.lock().unwrap().len();
        old(ProcessUpdate::Heartbeat { elapsed_ms: 2000 });
        assert_eq!(captured.lock().unwrap().len(), count);
        assert_eq!(
            captured
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .operation
                .as_ref()
                .unwrap()
                .attempt,
            2
        );
        let matching = runner.process_observer(
            PipelineStage::Matching,
            PipelineEngine::Colmap,
            Some(100),
            ObserverMode::Matching,
        );
        matching(ProcessUpdate::Started { process_id: 44 });
        for line in [
            "Generating sequential image pairs...",
            "Generating image pairs with vocabulary tree...",
            "Processing image [86/86]",
            "Processing image [1/9]",
        ] {
            matching(ProcessUpdate::Line {
                stream: crate::process::ProcessStream::Stdout,
                line: line.into(),
            });
        }
        let events = captured.lock().unwrap();
        let progress: Vec<_> = events
            .iter()
            .filter(|event| {
                event.stage == PipelineStage::Matching && event.kind == EventKind::Progress
            })
            .collect();
        assert_eq!(progress.len(), 4);
        assert!(progress
            .iter()
            .all(|event| event.stage_progress.is_none() && event.indeterminate));
        assert_eq!(progress[0].unit.as_deref(), Some("matchingSequential"));
        assert_eq!(progress[1].unit.as_deref(), Some("matchingLoop"));
        assert_eq!(progress[3].current, Some(1));
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
    fn mapper_registration_uses_the_current_model_count_not_an_accumulated_maximum() {
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
        assert_eq!(value.0, 89);
        assert!(parse_mapper_progress("Registering image #5", &counter, Some(100)).is_none());
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
    fn panorama_oom_retry_is_all_quality_but_still_once_and_optimization_only() {
        for quality in [Quality::Fast, Quality::Balanced, Quality::High] {
            let r =
                resolve_panorama_training_preset(quality, true, Some(8192), 512, 10_192).unwrap();
            let retry = brush_oom_fallback(quality, true, false, r, 10_192).unwrap();
            assert_eq!(retry.preset.max_splats, r.preset.max_splats.map(|n| n / 2));
            assert!(brush_oom_fallback(quality, true, true, retry, 10_192).is_none());
            assert!(brush_oom_fallback(quality, false, false, r, 10_192).is_none());
            let fixed =
                resolve_panorama_training_preset(quality, false, Some(8192), 512, 10_192).unwrap();
            assert!(brush_oom_fallback(quality, true, false, fixed, 10_192).is_none());
        }
    }

    #[test]
    fn event_sequence_is_strictly_increasing() {
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = events.clone();
        let sink = EventSink {
            context: Default::default(),
            bound_operation: None,
            panorama_phase: Arc::new(AtomicU64::new(0)),
            panorama_images: Arc::new(AtomicBool::new(false)),
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
            panorama_phase: Arc::new(AtomicU64::new(0)),
            panorama_images: Arc::new(AtomicBool::new(false)),
            context: Default::default(),
            bound_operation: None,
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

    #[test]
    fn panorama_preparation_and_training_use_distinct_progress_ranges() {
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = events.clone();
        let sink = EventSink {
            context: Default::default(),
            bound_operation: None,
            panorama_phase: Arc::new(AtomicU64::new(2)),
            panorama_images: Arc::new(AtomicBool::new(false)),
            emit: Arc::new(move |event| captured.lock().unwrap().push(event)),
            sequence: Arc::new(AtomicU64::new(0)),
            last_progress_milli_percent: Arc::new(AtomicU64::new(0)),
            last_stage: Arc::new(std::sync::Mutex::new(None)),
            dispatch: Arc::new(std::sync::Mutex::new(())),
            started: Instant::now(),
        };
        sink.stage(PipelineStage::TrainingSplats, 0.5, "faces");
        sink.panorama_phase.store(3, Ordering::Relaxed);
        sink.stage(PipelineStage::TrainingSplats, 1.0, "validated");
        sink.panorama_phase.store(1, Ordering::Relaxed);
        sink.stage(PipelineStage::TrainingSplats, 0.0, "training");
        sink.stage(PipelineStage::TrainingSplats, 0.5, "training");
        sink.stage(
            PipelineStage::TrainingSplats,
            0.1,
            "late or restarted progress",
        );
        sink.stage(
            PipelineStage::TrainingSplats,
            0.513123,
            "fractional progress",
        );
        sink.send(
            PipelineStage::TrainingSplats,
            Some(PipelineEngine::Brush),
            EventKind::Log,
            EventLevel::Info,
            None,
            true,
            "log without progress",
            None,
            None,
            None,
        );
        sink.stage(PipelineStage::Exporting, 1.0, "published");
        sink.stage(PipelineStage::Completed, 1.0, "completed");
        let events = events.lock().unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| event.progress)
                .collect::<Vec<_>>(),
            [63.0, 66.0, 66.0, 82.0, 82.0, 82.42, 82.42, 99.9, 100.0]
        );
        assert_eq!(
            events[0].substage.as_deref(),
            Some("panoramaTrainingPreparation")
        );
        assert_eq!(
            events[1].substage.as_deref(),
            Some("panoramaTrainingValidation")
        );
        assert_eq!(events[2].substage.as_deref(), Some("training"));
        assert!((events[1].stage_progress.unwrap() - 600.0 / 38.0).abs() < 0.001);
        assert_eq!(events[2].stage_progress, events[1].stage_progress);
    }
}
