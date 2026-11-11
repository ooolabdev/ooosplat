mod event;
mod service;

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::Instant,
};

use crate::{
    error::SplatError,
    pipeline::{
        effectiveness::{
            PlannerEffectivenessTracker, PLANNER_EFFECTIVENESS_SCHEMA_VERSION, PLANNER_VERSION_V1,
        },
        EventKind, PipelineEvent, PipelineStage,
    },
    presets::Quality,
};

pub use event::{
    DurationBucket, FrameCountBucket, TelemetryErrorCode, TelemetryEvent, TelemetryInputType,
    TelemetryPlannerOutcome, TelemetryQuality, TelemetryRunKind, TelemetryStage,
};
pub use service::{TelemetryDeliveryStatus, TelemetryPreferences, TelemetryService};

#[derive(Debug)]
struct StageTiming {
    active: Option<(PipelineStage, Instant, bool)>,
    last_stage: Option<TelemetryStage>,
}

#[cfg(test)]
mod failure_stage_tests {
    use super::*;

    #[tokio::test]
    async fn completed_stage_does_not_erase_the_failure_location() {
        let root = tempfile::tempdir().unwrap();
        let (service, _) = TelemetryService::recording(root.path().join("telemetry.json"));
        service.preferences().await.unwrap();
        let session =
            PipelineTelemetrySession::new(service, Quality::High, TelemetryInputType::Images);
        session.observe(&PipelineEvent::mapped(
            PipelineStage::TrainingSplats,
            0.0,
            "start",
        ));
        session.observe(&PipelineEvent::mapped(
            PipelineStage::TrainingSplats,
            1.0,
            "done",
        ));
        assert!(session.timing.lock().unwrap().active.is_none());
        let mut log = PipelineEvent::mapped(PipelineStage::Matching, 0.0, "late log");
        log.kind = EventKind::Log;
        session.observe(&log);
        session.observe(&PipelineEvent::mapped(PipelineStage::Failed, 1.0, "failed"));
        assert_eq!(
            session.current_stage(),
            Some(TelemetryStage::TrainingSplats)
        );
    }
}

struct PlannerTelemetryRun {
    run_id: uuid::Uuid,
    run_kind: TelemetryRunKind,
    planner_enabled: bool,
    tracker: PlannerEffectivenessTracker,
    emitted: AtomicBool,
}

/// Side-channel observer for one generation run. It consumes the existing public stage events
/// and never influences pipeline control flow or results.
pub struct PipelineTelemetrySession {
    service: TelemetryService,
    quality: TelemetryQuality,
    input_type: TelemetryInputType,
    started: Instant,
    timing: Mutex<StageTiming>,
    finished: AtomicBool,
    planner_run: Option<PlannerTelemetryRun>,
}

impl PipelineTelemetrySession {
    pub fn new(
        service: TelemetryService,
        quality: Quality,
        input_type: TelemetryInputType,
    ) -> Self {
        Self {
            service,
            quality: quality.into(),
            input_type,
            started: Instant::now(),
            timing: Mutex::new(StageTiming {
                active: None,
                last_stage: None,
            }),
            finished: AtomicBool::new(false),
            planner_run: None,
        }
    }

    pub fn new_with_planner_evaluation(
        service: TelemetryService,
        quality: Quality,
        input_type: TelemetryInputType,
        run_kind: TelemetryRunKind,
        planner_enabled: bool,
        tracker: PlannerEffectivenessTracker,
    ) -> Self {
        Self {
            service,
            quality: quality.into(),
            input_type,
            started: Instant::now(),
            timing: Mutex::new(StageTiming {
                active: None,
                last_stage: None,
            }),
            finished: AtomicBool::new(false),
            planner_run: Some(PlannerTelemetryRun {
                run_id: uuid::Uuid::new_v4(),
                run_kind,
                planner_enabled,
                tracker,
                emitted: AtomicBool::new(false),
            }),
        }
    }

    pub fn generation_started(&self) {
        self.service.track(TelemetryEvent::GenerationStarted {
            quality_preset: self.quality,
            input_type: self.input_type,
        });
    }

    pub fn observe(&self, event: &PipelineEvent) {
        if self.finished.load(Ordering::Acquire) || event.kind != EventKind::Stage {
            return;
        }
        let Some(stage) = TelemetryStage::from_pipeline(event.stage) else {
            return;
        };
        let now = Instant::now();
        let mut timing = self
            .timing
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        timing.last_stage = Some(stage);

        if timing
            .active
            .as_ref()
            .is_some_and(|(active, _, _)| *active != event.stage)
        {
            if let Some((completed, started, record_effectiveness)) = timing.active.take() {
                if let Some(completed) = TelemetryStage::from_pipeline(completed) {
                    self.emit_stage_completed(completed, started.elapsed(), record_effectiveness);
                }
            }
        }
        if timing.active.is_none() {
            let record_effectiveness = !event
                .stage_progress
                .is_some_and(|progress| progress >= 99.999);
            timing.active = Some((event.stage, now, record_effectiveness));
        }

        if !matches!(
            event.substage.as_deref(),
            Some("panoramaTrainingPreparation" | "panoramaTrainingValidation")
        ) && event
            .stage_progress
            .is_some_and(|progress| progress >= 99.999)
        {
            if let Some((completed, started, record_effectiveness)) = timing.active.take() {
                if completed == event.stage {
                    self.emit_stage_completed(stage, started.elapsed(), record_effectiveness);
                }
            }
        }
    }

    pub fn generation_completed(
        &self,
        total_duration_ms: u64,
        frame_count: u64,
        source_duration_seconds: Option<f64>,
    ) {
        if self.finished.swap(true, Ordering::AcqRel) {
            return;
        }
        self.flush_active_stage();
        self.service.track(TelemetryEvent::GenerationCompleted {
            quality_preset: self.quality,
            total_duration_ms,
            frame_count_bucket: FrameCountBucket::from_count(frame_count),
            duration_bucket: source_duration_seconds.map(DurationBucket::from_seconds),
        });
        self.emit_planner_evaluation(TelemetryPlannerOutcome::Completed, None, None);
    }

    pub fn generation_failed(&self, error: &SplatError) {
        if self.finished.swap(true, Ordering::AcqRel) {
            return;
        }
        // An unsupported new selection never started a generation attempt.
        // Do not turn its UI/MCP admission rejection into a failed-task event.
        if matches!(error, SplatError::Cancelled | SplatError::InputAdmission(_)) {
            return;
        }
        let stage = self.current_stage();
        self.flush_active_stage();
        let error_code = safe_error_code(error, stage);
        self.service
            .track(TelemetryEvent::GenerationFailed { stage, error_code });
        self.emit_planner_evaluation(TelemetryPlannerOutcome::Failed, stage, Some(error_code));
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    fn current_stage(&self) -> Option<TelemetryStage> {
        self.timing
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .last_stage
    }

    fn emit_stage_completed(
        &self,
        stage: TelemetryStage,
        duration: std::time::Duration,
        record_effectiveness: bool,
    ) {
        if record_effectiveness {
            if let Some(run) = &self.planner_run {
                if let Some(pipeline_stage) = pipeline_stage_from_telemetry(stage) {
                    run.tracker
                        .record_stage_duration(pipeline_stage, duration.as_millis() as u64);
                }
            }
        }
        self.service.track(TelemetryEvent::PipelineStageCompleted {
            stage,
            duration_ms: duration.as_millis() as u64,
        });
    }

    fn flush_active_stage(&self) {
        let active = self
            .timing
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .active
            .take();
        if let Some((stage, started, record_effectiveness)) = active {
            if let Some(stage) = TelemetryStage::from_pipeline(stage) {
                self.emit_stage_completed(stage, started.elapsed(), record_effectiveness);
            }
        }
    }

    fn emit_planner_evaluation(
        &self,
        outcome: TelemetryPlannerOutcome,
        failure_stage: Option<TelemetryStage>,
        error_code: Option<TelemetryErrorCode>,
    ) {
        let Some(run) = &self.planner_run else {
            return;
        };
        if run
            .emitted
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        self.service.track(TelemetryEvent::PlannerEvaluation {
            planner_schema_version: PLANNER_EFFECTIVENESS_SCHEMA_VERSION,
            run_id: run.run_id,
            run_kind: run.run_kind,
            outcome,
            planner_enabled: run.planner_enabled,
            planner_version: run.planner_enabled.then_some(PLANNER_VERSION_V1),
            quality_preset: self.quality,
            input_type: self.input_type,
            total_duration_ms: self.elapsed_ms(),
            failure_stage,
            error_code,
            snapshot: Box::new(run.tracker.snapshot()),
        });
    }
}

fn pipeline_stage_from_telemetry(stage: TelemetryStage) -> Option<PipelineStage> {
    match stage {
        TelemetryStage::ProbingVideo => Some(PipelineStage::ProbingVideo),
        TelemetryStage::ExtractingFrames => Some(PipelineStage::ExtractingFrames),
        TelemetryStage::ExtractingFeatures => Some(PipelineStage::ExtractingFeatures),
        TelemetryStage::Matching => Some(PipelineStage::Matching),
        TelemetryStage::Reconstructing => Some(PipelineStage::Reconstructing),
        TelemetryStage::ValidatingReconstruction => Some(PipelineStage::ValidatingReconstruction),
        TelemetryStage::TrainingSplats => Some(PipelineStage::TrainingSplats),
        TelemetryStage::Exporting => Some(PipelineStage::Exporting),
        TelemetryStage::Unknown => None,
    }
}

fn safe_error_code(error: &SplatError, stage: Option<TelemetryStage>) -> TelemetryErrorCode {
    match error {
        SplatError::Diagnostic { source, .. } => safe_error_code(source, stage),
        SplatError::EngineMissing(_)
        | SplatError::EngineStart { .. }
        | SplatError::UnsupportedEngine(_) => TelemetryErrorCode::EngineUnavailable,
        SplatError::InputAdmission(_)
        | SplatError::InvalidVideo(_)
        | SplatError::InvalidPath(_) => TelemetryErrorCode::InvalidInput,
        SplatError::Io(io_error) => {
            if io_error.raw_os_error() == Some(112) {
                TelemetryErrorCode::DiskSpaceLow
            } else {
                TelemetryErrorCode::IoFailed
            }
        }
        SplatError::Json(_) => TelemetryErrorCode::Unknown,
        SplatError::Cancelled => TelemetryErrorCode::Unknown,
        SplatError::BrushOutOfMemory(_) => TelemetryErrorCode::BrushOutOfMemory,
        SplatError::BrushDeviceLost(_) => TelemetryErrorCode::BrushDeviceLost,
        SplatError::EngineProcess(failure) => match failure.classification.kind {
            Some("brush_out_of_memory") => TelemetryErrorCode::BrushOutOfMemory,
            Some("brush_device_lost") => TelemetryErrorCode::BrushDeviceLost,
            Some("mapper_storage")
                if failure.detail.to_ascii_lowercase().contains("disk full")
                    || failure
                        .detail
                        .to_ascii_lowercase()
                        .contains("no space left") =>
            {
                TelemetryErrorCode::DiskSpaceLow
            }
            _ if failure.engine == crate::pipeline::PipelineEngine::Brush => {
                TelemetryErrorCode::BrushFailed
            }
            _ if failure.engine == crate::pipeline::PipelineEngine::Colmap => {
                match failure.operation.as_str() {
                    "feature_extractor" => TelemetryErrorCode::ColmapFeatureFailed,
                    "sequential_matcher" | "exhaustive_matcher" | "matches_importer" => {
                        TelemetryErrorCode::ColmapMatchingFailed
                    }
                    _ => TelemetryErrorCode::ColmapMapperFailed,
                }
            }
            _ => TelemetryErrorCode::Unknown,
        },
        SplatError::Process(detail) => {
            let normalized = detail.to_ascii_lowercase();
            if stage == Some(TelemetryStage::TrainingSplats) && crate::failure::gpu_oom(detail) {
                return TelemetryErrorCode::BrushOutOfMemory;
            }
            if normalized.contains("no space")
                || normalized.contains("disk full")
                || detail.contains("磁盘空间")
            {
                return TelemetryErrorCode::DiskSpaceLow;
            }
            if detail.contains("低于 50%") || detail.contains("注册率过低") {
                return TelemetryErrorCode::LowRegisteredImages;
            }
            match stage {
                Some(TelemetryStage::ProbingVideo) => TelemetryErrorCode::FfprobeFailed,
                Some(TelemetryStage::ExtractingFrames) => TelemetryErrorCode::FfmpegFailed,
                Some(TelemetryStage::ExtractingFeatures) => TelemetryErrorCode::ColmapFeatureFailed,
                Some(TelemetryStage::Matching) => TelemetryErrorCode::ColmapMatchingFailed,
                Some(TelemetryStage::Reconstructing | TelemetryStage::ValidatingReconstruction) => {
                    TelemetryErrorCode::ColmapMapperFailed
                }
                Some(TelemetryStage::TrainingSplats) => TelemetryErrorCode::BrushFailed,
                Some(TelemetryStage::Exporting | TelemetryStage::Unknown) | None => {
                    TelemetryErrorCode::Unknown
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{EventLevel, PipelineEngine};

    #[tokio::test]
    async fn input_admission_does_not_emit_generation_failure_or_planner_summary() {
        let root = tempfile::tempdir().unwrap();
        let (service, events) = TelemetryService::recording(root.path().join("telemetry.json"));
        service.enable_for_test().await;
        for reason in [
            crate::error::InputAdmissionReason::ColmapDisabled,
            crate::error::InputAdmissionReason::UnsupportedInput,
        ] {
            let session = PipelineTelemetrySession::new_with_planner_evaluation(
                service.clone(),
                Quality::Balanced,
                TelemetryInputType::Images,
                TelemetryRunKind::New,
                true,
                PlannerEffectivenessTracker::default(),
            );
            let error = SplatError::InputAdmission(reason);
            assert_eq!(
                safe_error_code(&error, None),
                TelemetryErrorCode::InvalidInput
            );
            session.generation_failed(&error);
            assert!(session.finished.load(Ordering::Acquire));
            assert!(!session
                .planner_run
                .as_ref()
                .unwrap()
                .emitted
                .load(Ordering::Acquire));
            session.generation_failed(&SplatError::Process("late failure".into()));
            session.generation_completed(1, 30, None);
        }
        tokio::task::yield_now().await;
        assert!(events.lock().unwrap().is_empty());
    }

    #[test]
    fn engine_failure_telemetry_keeps_existing_codes_without_cross_engine_oom_guesses() {
        for (engine, text, expected) in [
            (
                PipelineEngine::Brush,
                "wgpu error: Out of Memory\nAn IO error happened",
                TelemetryErrorCode::BrushOutOfMemory,
            ),
            (
                PipelineEngine::Brush,
                "BufferTooBig(2290420416)",
                TelemetryErrorCode::BrushFailed,
            ),
            (
                PipelineEngine::Brush,
                "VK_ERROR_DEVICE_LOST",
                TelemetryErrorCode::BrushDeviceLost,
            ),
            (
                PipelineEngine::Colmap,
                "wgpu error: Out of Memory",
                TelemetryErrorCode::ColmapMapperFailed,
            ),
        ] {
            let output = crate::process::ProcessOutput {
                success: false,
                exit_code: Some(101),
                termination: Default::default(),
                stdout: String::new(),
                stderr: text.into(),
            };
            let failure =
                crate::failure::EngineProcessFailure::from_output(engine, "mapper", &output);
            let error = SplatError::EngineProcess(Box::new(failure));
            assert_eq!(
                safe_error_code(&error, Some(TelemetryStage::TrainingSplats)),
                expected
            );
        }
    }

    fn stage_event(stage: PipelineStage, stage_progress: f32) -> PipelineEvent {
        PipelineEvent {
            operation: None,
            task_id: None,
            run_id: None,
            revision: 0,
            sequence: 1,
            timestamp: chrono::Utc::now(),
            kind: EventKind::Stage,
            level: EventLevel::Info,
            stage,
            engine: Some(PipelineEngine::System),
            substage: None,
            progress: stage_progress,
            stage_progress: Some(stage_progress),
            indeterminate: false,
            message: "local-only UI message".into(),
            current: None,
            total: None,
            unit: None,
            elapsed_ms: 0,
            acceleration: None,
            runtime: None,
        }
    }

    #[test]
    fn safe_error_mapping_never_returns_raw_details() {
        let error =
            SplatError::Process(r#"COLMAP failed for C:\Users\someone\private\video.mp4"#.into());
        assert_eq!(
            safe_error_code(&error, Some(TelemetryStage::ExtractingFeatures)),
            TelemetryErrorCode::ColmapFeatureFailed
        );
    }

    #[test]
    fn retry_preparation_context_preserves_io_telemetry_codes() {
        let denied = SplatError::Io(std::io::Error::from_raw_os_error(5))
            .with_io_diagnostic("retry_preparation_operation=archive_retry_cache");
        assert_eq!(safe_error_code(&denied, None), TelemetryErrorCode::IoFailed);

        let full = SplatError::Io(std::io::Error::from_raw_os_error(112))
            .with_io_diagnostic("retry_preparation_operation=write_retry_archive_progress");
        assert_eq!(
            safe_error_code(&full, None),
            TelemetryErrorCode::DiskSpaceLow
        );
    }

    #[test]
    fn cancellation_is_not_a_failure_code() {
        assert_eq!(
            safe_error_code(&SplatError::Cancelled, None),
            TelemetryErrorCode::Unknown
        );
    }

    #[test]
    fn panorama_preparation_does_not_complete_the_training_telemetry_stage() {
        let directory = tempfile::tempdir().unwrap();
        let (service, _) = TelemetryService::recording(directory.path().join("telemetry.json"));
        let session =
            PipelineTelemetrySession::new(service, Quality::Fast, TelemetryInputType::Video);
        let mut event = stage_event(PipelineStage::TrainingSplats, 0.0);
        event.substage = Some("panoramaTrainingPreparation".into());
        session.observe(&event);
        event.stage_progress = Some(100.0);
        event.substage = Some("panoramaTrainingValidation".into());
        session.observe(&event);
        assert!(session.timing.lock().unwrap().active.is_some());
        event.substage = Some("training".into());
        event.stage_progress = Some(0.0);
        session.observe(&event);
        assert!(session.timing.lock().unwrap().active.is_some());
        event.stage_progress = Some(100.0);
        session.observe(&event);
        assert!(session.timing.lock().unwrap().active.is_none());
    }

    #[test]
    fn brush_device_loss_has_a_dedicated_privacy_safe_code() {
        assert_eq!(
            safe_error_code(
                &SplatError::BrushDeviceLost("private driver detail".into()),
                Some(TelemetryStage::TrainingSplats),
            ),
            TelemetryErrorCode::BrushDeviceLost
        );
        assert_eq!(
            serde_json::to_value(TelemetryErrorCode::BrushDeviceLost).unwrap(),
            "brush_device_lost"
        );
    }

    #[tokio::test]
    async fn pipeline_observer_emits_structured_stage_duration_without_message() {
        let directory = tempfile::tempdir().unwrap();
        let (service, events) =
            TelemetryService::recording(directory.path().join("telemetry.json"));
        service.enable_for_test().await;
        let session =
            PipelineTelemetrySession::new(service, Quality::Balanced, TelemetryInputType::Video);
        session.observe(&stage_event(PipelineStage::ExtractingFrames, 0.0));
        session.observe(&stage_event(PipelineStage::ExtractingFrames, 100.0));
        for _ in 0..50 {
            if events
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .any(|value| value["event"] == "pipeline_stage_completed")
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let recorded = events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let stage = recorded
            .iter()
            .find(|value| value["event"] == "pipeline_stage_completed")
            .unwrap();
        assert_eq!(stage["properties"]["stage"], "extracting_frames");
        assert!(stage.get("message").is_none());
        assert!(stage.get("path").is_none());
    }

    #[tokio::test]
    async fn planner_evaluation_is_terminal_idempotent_and_cancellation_is_excluded() {
        let directory = tempfile::tempdir().unwrap();
        let (service, events) =
            TelemetryService::recording(directory.path().join("telemetry.json"));
        service.enable_for_test().await;
        let session = PipelineTelemetrySession::new_with_planner_evaluation(
            service,
            Quality::High,
            TelemetryInputType::Images,
            TelemetryRunKind::Resume,
            true,
            PlannerEffectivenessTracker::default(),
        );
        session.generation_completed(100, 30, None);
        session.generation_completed(100, 30, None);
        session.generation_failed(&SplatError::Process("late failure".into()));

        for _ in 0..50 {
            if events
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .any(|value| value["event"] == "planner_evaluation")
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        {
            let recorded = events
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let evaluations = recorded
                .iter()
                .filter(|value| value["event"] == "planner_evaluation")
                .collect::<Vec<_>>();
            assert_eq!(evaluations.len(), 1);
            assert_eq!(evaluations[0]["properties"]["runKind"], "resume");
            assert_eq!(evaluations[0]["properties"]["outcome"], "completed");
            assert_eq!(
                recorded
                    .iter()
                    .filter(|event| event["event"] == "generation_completed")
                    .count(),
                1
            );
            assert!(!recorded
                .iter()
                .any(|event| event["event"] == "generation_failed"));
        }

        let cancelled = PipelineTelemetrySession::new_with_planner_evaluation(
            session.service.clone(),
            Quality::High,
            TelemetryInputType::Images,
            TelemetryRunKind::New,
            true,
            PlannerEffectivenessTracker::default(),
        );
        cancelled.generation_failed(&SplatError::Cancelled);
        cancelled.generation_completed(100, 30, None);
        assert!(cancelled.finished.load(Ordering::Acquire));

        let failed = PipelineTelemetrySession::new_with_planner_evaluation(
            session.service.clone(),
            Quality::High,
            TelemetryInputType::Images,
            TelemetryRunKind::New,
            true,
            PlannerEffectivenessTracker::default(),
        );
        failed.generation_failed(&SplatError::BrushDeviceLost("fixture".into()));
        failed.generation_failed(&SplatError::BrushDeviceLost("duplicate".into()));
        failed.generation_completed(100, 30, None);
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let ready = {
                    let events = events.lock().unwrap();
                    events
                        .iter()
                        .filter(|event| event["event"] == "planner_evaluation")
                        .count()
                        == 2
                        && events
                            .iter()
                            .filter(|event| event["event"] == "generation_failed")
                            .count()
                            == 1
                };
                if ready {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let recorded = events.lock().unwrap();
        assert_eq!(
            recorded
                .iter()
                .filter(|event| event["event"] == "generation_completed")
                .count(),
            1
        );
        assert_eq!(
            recorded
                .iter()
                .filter(|event| event["event"] == "generation_failed")
                .count(),
            1
        );
    }

    #[test]
    fn resume_checkpoint_events_do_not_invent_stage_duration() {
        let directory = tempfile::tempdir().unwrap();
        let (service, _) = TelemetryService::recording(directory.path().join("telemetry.json"));
        let tracker = PlannerEffectivenessTracker::default();
        let session = PipelineTelemetrySession::new_with_planner_evaluation(
            service,
            Quality::Balanced,
            TelemetryInputType::Video,
            TelemetryRunKind::Resume,
            true,
            tracker.clone(),
        );
        session.observe(&stage_event(PipelineStage::ExtractingFeatures, 100.0));

        assert_eq!(
            tracker.snapshot().stage_durations.extracting_features_ms,
            None
        );
    }
}
