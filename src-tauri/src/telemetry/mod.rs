mod event;
mod service;

use std::{sync::Mutex, time::Instant};

use crate::{
    error::SplatError,
    pipeline::{EventKind, PipelineEvent, PipelineStage},
    presets::Quality,
    project::QualityRunMetrics,
};

pub use event::{
    DurationBucket, FrameCountBucket, TelemetryErrorCode, TelemetryEvent, TelemetryInputType,
    TelemetryQuality, TelemetryStage,
};
pub use service::{TelemetryDeliveryStatus, TelemetryPreferences, TelemetryService};

#[derive(Debug)]
struct StageTiming {
    active: Option<(PipelineStage, Instant)>,
}

/// Side-channel observer for one generation run. It consumes the existing public stage events
/// and never influences pipeline control flow or results.
pub struct PipelineTelemetrySession {
    service: TelemetryService,
    quality: TelemetryQuality,
    input_type: TelemetryInputType,
    started: Instant,
    timing: Mutex<StageTiming>,
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
            timing: Mutex::new(StageTiming { active: None }),
        }
    }

    pub fn generation_started(&self) {
        self.service.track(TelemetryEvent::GenerationStarted {
            quality_preset: self.quality,
            input_type: self.input_type,
        });
    }

    pub fn observe(&self, event: &PipelineEvent) {
        if event.kind != EventKind::Stage {
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

        if timing
            .active
            .as_ref()
            .is_some_and(|(active, _)| *active != event.stage)
        {
            if let Some((completed, started)) = timing.active.take() {
                if let Some(completed) = TelemetryStage::from_pipeline(completed) {
                    self.emit_stage_completed(completed, started.elapsed());
                }
            }
        }
        if timing.active.is_none() {
            timing.active = Some((event.stage, now));
        }

        if event
            .stage_progress
            .is_some_and(|progress| progress >= 99.999)
        {
            if let Some((completed, started)) = timing.active.take() {
                if completed == event.stage {
                    self.emit_stage_completed(stage, started.elapsed());
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
        self.service.track(TelemetryEvent::GenerationCompleted {
            quality_preset: self.quality,
            total_duration_ms,
            frame_count_bucket: FrameCountBucket::from_count(frame_count),
            duration_bucket: source_duration_seconds.map(DurationBucket::from_seconds),
        });
    }

    pub fn generation_failed(&self, error: &SplatError) {
        if matches!(error, SplatError::Cancelled) {
            return;
        }
        let stage = self.current_stage();
        self.service.track(TelemetryEvent::GenerationFailed {
            stage,
            error_code: safe_error_code(error, stage),
        });
    }

    pub fn quality_metrics_recorded(&self, metrics: &QualityRunMetrics) {
        self.service.track(TelemetryEvent::QualityMetricsRecorded {
            actual_frame_count: metrics.actual_frame_count,
            actual_sfm_resolution: metrics.actual_sfm_resolution,
            actual_feature_count: metrics.actual_feature_count,
            actual_brush_resolution: metrics.actual_brush_resolution,
            actual_brush_iterations: metrics.actual_brush_iterations,
            registered_images: metrics.registered_images,
            reprojection_error: metrics.reprojection_error,
            splat_count: metrics.splat_count,
            peak_gpu_memory_mb: metrics.peak_gpu_memory_mb,
        });
        if metrics.planner_enabled {
            self.service.track(TelemetryEvent::PlannerMetricsRecorded {
                planner_version: metrics.planner_version.unwrap_or(1),
                capture_type: metrics.capture_type.clone(),
                pairing_planned: metrics.pairing_planned.clone(),
                pairing_actual: metrics.pairing_actual.clone(),
                mapper_planned: metrics.mapper_planned.clone(),
                mapper_actual: metrics.mapper_actual.clone(),
                largest_component_ratio: metrics.largest_component_ratio,
                two_core_ratio: metrics.two_core_ratio,
                bridge_ratio: metrics.bridge_ratio,
                normal_rescue_rounds: metrics.normal_rescue_rounds,
                success_recovery_rounds: metrics.success_recovery_rounds,
                normal_budget_exhausted: metrics.normal_budget_exhausted,
                success_recovery_entered: metrics.success_recovery_entered,
                budget_overridden_for_success: metrics.budget_overridden_for_success,
                normal_duration_ms: metrics.normal_duration_ms,
                recovery_duration_ms: metrics.recovery_duration_ms,
                reconstruction_quality: metrics.reconstruction_quality.clone(),
            });
        }
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    fn current_stage(&self) -> Option<TelemetryStage> {
        self.timing
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .active
            .and_then(|(stage, _)| TelemetryStage::from_pipeline(stage))
    }

    fn emit_stage_completed(&self, stage: TelemetryStage, duration: std::time::Duration) {
        self.service.track(TelemetryEvent::PipelineStageCompleted {
            stage,
            duration_ms: duration.as_millis() as u64,
        });
    }
}

fn safe_error_code(error: &SplatError, stage: Option<TelemetryStage>) -> TelemetryErrorCode {
    match error {
        SplatError::EngineMissing(_)
        | SplatError::EngineStart { .. }
        | SplatError::UnsupportedEngine(_) => TelemetryErrorCode::EngineUnavailable,
        SplatError::InvalidVideo(_) | SplatError::InvalidPath(_) => {
            TelemetryErrorCode::InvalidInput
        }
        SplatError::Io(io_error) => {
            if io_error.raw_os_error() == Some(112) {
                TelemetryErrorCode::DiskSpaceLow
            } else {
                TelemetryErrorCode::IoFailed
            }
        }
        SplatError::Json(_) => TelemetryErrorCode::Unknown,
        SplatError::Cancelled => TelemetryErrorCode::Unknown,
        SplatError::Process(detail) => {
            let normalized = detail.to_ascii_lowercase();
            if normalized.contains("out of memory")
                || normalized.contains("outofmemory")
                || detail.contains("显存不足")
            {
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

    fn stage_event(stage: PipelineStage, stage_progress: f32) -> PipelineEvent {
        PipelineEvent {
            sequence: 1,
            timestamp: chrono::Utc::now(),
            kind: EventKind::Stage,
            level: EventLevel::Info,
            stage,
            engine: Some(PipelineEngine::System),
            progress: stage_progress,
            stage_progress: Some(stage_progress),
            indeterminate: false,
            message: "local-only UI message".into(),
            current: None,
            total: None,
            unit: None,
            elapsed_ms: 0,
            acceleration: None,
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
    fn cancellation_is_not_a_failure_code() {
        assert_eq!(
            safe_error_code(&SplatError::Cancelled, None),
            TelemetryErrorCode::Unknown
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
}
