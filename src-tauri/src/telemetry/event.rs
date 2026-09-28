use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    pipeline::{
        effectiveness::{
            PlannerBridgeMetrics, PlannerBrushMetrics, PlannerEffectivenessSnapshot,
            PlannerFinalResultMetrics, PlannerFramePlanMetrics, PlannerGpuVendor,
            PlannerInitialReconstructionMetrics, PlannerStageDurationMetrics,
        },
        PipelineStage,
    },
    presets::Quality,
};

/// Deliberately small, stable telemetry vocabulary. Paths, names, logs and user-provided
/// strings cannot be represented by these DTOs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryOs {
    Windows,
    Macos,
    Linux,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryArch {
    X86_64,
    Aarch64,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryQuality {
    Fast,
    Balanced,
    High,
}

impl From<Quality> for TelemetryQuality {
    fn from(value: Quality) -> Self {
        match value {
            Quality::Fast => Self::Fast,
            Quality::Balanced => Self::Balanced,
            Quality::High => Self::High,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryInputType {
    Video,
    Images,
    Scan,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryStage {
    ProbingVideo,
    ExtractingFrames,
    ExtractingFeatures,
    Matching,
    Reconstructing,
    ValidatingReconstruction,
    TrainingSplats,
    Exporting,
    Unknown,
}

impl TelemetryStage {
    pub fn from_pipeline(value: PipelineStage) -> Option<Self> {
        match value {
            PipelineStage::ProbingVideo => Some(Self::ProbingVideo),
            PipelineStage::ExtractingFrames => Some(Self::ExtractingFrames),
            PipelineStage::ExtractingFeatures => Some(Self::ExtractingFeatures),
            PipelineStage::Matching => Some(Self::Matching),
            PipelineStage::Reconstructing => Some(Self::Reconstructing),
            PipelineStage::ValidatingReconstruction => Some(Self::ValidatingReconstruction),
            PipelineStage::TrainingSplats => Some(Self::TrainingSplats),
            PipelineStage::Exporting => Some(Self::Exporting),
            PipelineStage::Created
            | PipelineStage::PlanningFrames
            | PipelineStage::Completed
            | PipelineStage::Failed
            | PipelineStage::Cancelled => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryErrorCode {
    EngineUnavailable,
    InvalidInput,
    FfprobeFailed,
    FfmpegFailed,
    ColmapFeatureFailed,
    ColmapMatchingFailed,
    ColmapMapperFailed,
    LowRegisteredImages,
    BrushFailed,
    BrushOutOfMemory,
    BrushDeviceLost,
    DiskSpaceLow,
    IoFailed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryRunKind {
    New,
    Resume,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryPlannerOutcome {
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum FrameCountBucket {
    #[serde(rename = "0-100")]
    UpTo100,
    #[serde(rename = "101-300")]
    From101To300,
    #[serde(rename = "301-500")]
    From301To500,
    #[serde(rename = "501-1000")]
    From501To1000,
    #[serde(rename = "1001-2000")]
    From1001To2000,
    #[serde(rename = "2000+")]
    Over2000,
}

impl FrameCountBucket {
    pub fn from_count(value: u64) -> Self {
        match value {
            0..=100 => Self::UpTo100,
            101..=300 => Self::From101To300,
            301..=500 => Self::From301To500,
            501..=1000 => Self::From501To1000,
            1001..=2000 => Self::From1001To2000,
            _ => Self::Over2000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DurationBucket {
    #[serde(rename = "0-10s")]
    UpTo10Seconds,
    #[serde(rename = "10-30s")]
    UpTo30Seconds,
    #[serde(rename = "30-60s")]
    UpTo60Seconds,
    #[serde(rename = "1-2m")]
    UpTo2Minutes,
    #[serde(rename = "2-5m")]
    UpTo5Minutes,
    #[serde(rename = "5m+")]
    Over5Minutes,
}

impl DurationBucket {
    pub fn from_seconds(value: f64) -> Self {
        if value <= 10.0 {
            Self::UpTo10Seconds
        } else if value <= 30.0 {
            Self::UpTo30Seconds
        } else if value <= 60.0 {
            Self::UpTo60Seconds
        } else if value <= 120.0 {
            Self::UpTo2Minutes
        } else if value <= 300.0 {
            Self::UpTo5Minutes
        } else {
            Self::Over5Minutes
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub enum TelemetryEvent {
    DailyActive,
    GenerationStarted {
        quality_preset: TelemetryQuality,
        input_type: TelemetryInputType,
    },
    GenerationCompleted {
        quality_preset: TelemetryQuality,
        total_duration_ms: u64,
        frame_count_bucket: FrameCountBucket,
        duration_bucket: Option<DurationBucket>,
    },
    GenerationFailed {
        stage: Option<TelemetryStage>,
        error_code: TelemetryErrorCode,
    },
    PipelineStageCompleted {
        stage: TelemetryStage,
        duration_ms: u64,
    },
    PlannerEvaluation {
        planner_schema_version: u32,
        run_id: Uuid,
        run_kind: TelemetryRunKind,
        outcome: TelemetryPlannerOutcome,
        planner_enabled: bool,
        planner_version: Option<&'static str>,
        quality_preset: TelemetryQuality,
        input_type: TelemetryInputType,
        total_duration_ms: u64,
        failure_stage: Option<TelemetryStage>,
        error_code: Option<TelemetryErrorCode>,
        snapshot: Box<PlannerEffectivenessSnapshot>,
    },
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum TelemetryEventName {
    DailyActive,
    GenerationStarted,
    GenerationCompleted,
    GenerationFailed,
    PipelineStageCompleted,
    PlannerEvaluation,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct TelemetryProperties {
    #[serde(skip_serializing_if = "Option::is_none")]
    quality_preset: Option<TelemetryQuality>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_type: Option<TelemetryInputType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frame_count_bucket: Option<FrameCountBucket>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_duration_bucket: Option<DurationBucket>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stage: Option<TelemetryStage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_code: Option<TelemetryErrorCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    planner_schema_version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_kind: Option<TelemetryRunKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<TelemetryPlannerOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    planner_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    planner_version: Option<Option<&'static str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_duration_ms: Option<u64>,
    #[serde(rename = "failureStage", skip_serializing_if = "Option::is_none")]
    failure_stage: Option<Option<TelemetryStage>>,
    #[serde(rename = "errorCode", skip_serializing_if = "Option::is_none")]
    planner_error_code: Option<Option<TelemetryErrorCode>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gpu_vendor: Option<PlannerGpuVendor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frame_plan: Option<Option<PlannerFramePlanMetrics>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    initial_reconstruction: Option<Option<PlannerInitialReconstructionMetrics>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bridge: Option<Option<PlannerBridgeMetrics>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    brush: Option<Option<PlannerBrushMetrics>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    final_result: Option<Option<PlannerFinalResultMetrics>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stage_durations: Option<PlannerStageDurationMetrics>,
}

impl TelemetryEvent {
    fn into_wire_parts(self) -> (TelemetryEventName, TelemetryProperties) {
        match self {
            Self::DailyActive => (
                TelemetryEventName::DailyActive,
                TelemetryProperties::default(),
            ),
            Self::GenerationStarted {
                quality_preset,
                input_type,
            } => (
                TelemetryEventName::GenerationStarted,
                TelemetryProperties {
                    quality_preset: Some(quality_preset),
                    input_type: Some(input_type),
                    ..TelemetryProperties::default()
                },
            ),
            Self::GenerationCompleted {
                quality_preset,
                total_duration_ms,
                frame_count_bucket,
                duration_bucket,
            } => (
                TelemetryEventName::GenerationCompleted,
                TelemetryProperties {
                    quality_preset: Some(quality_preset),
                    duration_ms: Some(total_duration_ms.min(86_400_000)),
                    frame_count_bucket: Some(frame_count_bucket),
                    input_duration_bucket: duration_bucket,
                    ..TelemetryProperties::default()
                },
            ),
            Self::GenerationFailed { stage, error_code } => (
                TelemetryEventName::GenerationFailed,
                TelemetryProperties {
                    stage,
                    error_code: Some(error_code),
                    ..TelemetryProperties::default()
                },
            ),
            Self::PipelineStageCompleted { stage, duration_ms } => (
                TelemetryEventName::PipelineStageCompleted,
                TelemetryProperties {
                    stage: Some(stage),
                    duration_ms: Some(duration_ms.min(86_400_000)),
                    ..TelemetryProperties::default()
                },
            ),
            Self::PlannerEvaluation {
                planner_schema_version,
                run_id,
                run_kind,
                outcome,
                planner_enabled,
                planner_version,
                quality_preset,
                input_type,
                total_duration_ms,
                failure_stage,
                error_code,
                snapshot,
            } => (
                TelemetryEventName::PlannerEvaluation,
                TelemetryProperties {
                    quality_preset: Some(quality_preset),
                    input_type: Some(input_type),
                    planner_schema_version: Some(planner_schema_version),
                    run_id: Some(run_id),
                    run_kind: Some(run_kind),
                    outcome: Some(outcome),
                    planner_enabled: Some(planner_enabled),
                    planner_version: Some(planner_version),
                    total_duration_ms: Some(total_duration_ms),
                    failure_stage: Some(failure_stage),
                    planner_error_code: Some(error_code),
                    gpu_vendor: Some(snapshot.gpu_vendor),
                    frame_plan: Some(snapshot.frame_plan),
                    initial_reconstruction: Some(snapshot.initial_reconstruction),
                    bridge: Some(snapshot.bridge),
                    brush: Some(snapshot.brush),
                    final_result: Some(snapshot.final_result),
                    stage_durations: Some(snapshot.stage_durations),
                    ..TelemetryProperties::default()
                },
            ),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TelemetryPayload {
    install_id: Uuid,
    event: TelemetryEventName,
    timestamp: DateTime<Utc>,
    app_version: &'static str,
    os: TelemetryOs,
    arch: TelemetryArch,
    properties: TelemetryProperties,
}

impl TelemetryPayload {
    pub fn new(install_id: Uuid, event: TelemetryEvent) -> Self {
        let (event, properties) = event.into_wire_parts();
        Self {
            install_id,
            event,
            timestamp: Utc::now(),
            app_version: env!("CARGO_PKG_VERSION"),
            os: current_os(),
            arch: current_arch(),
            properties,
        }
    }
}

fn current_os() -> TelemetryOs {
    if cfg!(target_os = "windows") {
        TelemetryOs::Windows
    } else if cfg!(target_os = "macos") {
        TelemetryOs::Macos
    } else if cfg!(target_os = "linux") {
        TelemetryOs::Linux
    } else {
        TelemetryOs::Unknown
    }
}

fn current_arch() -> TelemetryArch {
    if cfg!(target_arch = "x86_64") {
        TelemetryArch::X86_64
    } else if cfg!(target_arch = "aarch64") {
        TelemetryArch::Aarch64
    } else {
        TelemetryArch::Unknown
    }
}

const FORBIDDEN_KEYS: &[&str] = &[
    "path",
    "file_path",
    "filename",
    "file_name",
    "video_name",
    "project_name",
    "username",
    "hostname",
    "image",
    "video",
    "frame",
    "ply",
    "sog",
    "project_content",
    "stdout",
    "stderr",
];

/// Final defensive check after serialization. Event DTOs are the primary privacy boundary;
/// this guard makes a future accidental sensitive field fail closed.
pub fn validate_privacy(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => map.iter().all(|(key, child)| {
            let mut normalized = String::with_capacity(key.len());
            for character in key.chars() {
                if character == '-' {
                    normalized.push('_');
                } else if character.is_ascii_uppercase() {
                    if !normalized.is_empty() {
                        normalized.push('_');
                    }
                    normalized.push(character.to_ascii_lowercase());
                } else {
                    normalized.extend(character.to_lowercase());
                }
            }
            !FORBIDDEN_KEYS.contains(&normalized.as_str()) && validate_privacy(child)
        }),
        serde_json::Value::Array(values) => values.iter().all(validate_privacy),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::effectiveness::{
        PlannerEffectivenessSnapshot, PlannerFramePlanMetrics, PLANNER_EFFECTIVENESS_SCHEMA_VERSION,
    };

    #[test]
    fn payload_schema_contains_only_approved_common_fields() {
        let payload = TelemetryPayload::new(
            Uuid::nil(),
            TelemetryEvent::GenerationStarted {
                quality_preset: TelemetryQuality::Balanced,
                input_type: TelemetryInputType::Video,
            },
        );
        let value = serde_json::to_value(payload).unwrap();
        assert_eq!(value["event"], "generation_started");
        assert_eq!(value["appVersion"], env!("CARGO_PKG_VERSION"));
        assert_eq!(value["properties"]["qualityPreset"], "balanced");
        assert_eq!(value["properties"]["inputType"], "video");
        assert_eq!(value.as_object().unwrap().len(), 7);
        assert!(validate_privacy(&value));
        assert!(value.get("projectPath").is_none());
        assert!(value.get("filename").is_none());
    }

    #[test]
    fn image_events_use_images_and_omit_a_fake_video_duration() {
        let started = serde_json::to_value(TelemetryPayload::new(
            Uuid::nil(),
            TelemetryEvent::GenerationStarted {
                quality_preset: TelemetryQuality::Balanced,
                input_type: TelemetryInputType::Images,
            },
        ))
        .unwrap();
        assert_eq!(started["properties"]["inputType"], "images");

        let completed = serde_json::to_value(TelemetryPayload::new(
            Uuid::nil(),
            TelemetryEvent::GenerationCompleted {
                quality_preset: TelemetryQuality::Balanced,
                total_duration_ms: 30_000,
                frame_count_bucket: FrameCountBucket::UpTo100,
                duration_bucket: None,
            },
        ))
        .unwrap();
        assert!(completed["properties"].get("inputDurationBucket").is_none());
    }

    #[test]
    fn privacy_guard_rejects_sensitive_keys_recursively() {
        assert!(!validate_privacy(
            &serde_json::json!({"details": {"file_name": "secret"}})
        ));
        assert!(!validate_privacy(
            &serde_json::json!({"details": {"fileName": "secret"}})
        ));
        assert!(!validate_privacy(
            &serde_json::json!({"stdout": "raw engine output"})
        ));
    }

    #[test]
    fn buckets_do_not_expose_exact_input_characteristics() {
        assert_eq!(
            FrameCountBucket::from_count(301),
            FrameCountBucket::From301To500
        );
        assert_eq!(
            DurationBucket::from_seconds(35.5),
            DurationBucket::UpTo60Seconds
        );
    }

    #[test]
    fn production_wire_schema_uses_nested_camel_case_properties() {
        let value = serde_json::to_value(TelemetryPayload::new(
            Uuid::nil(),
            TelemetryEvent::PipelineStageCompleted {
                stage: TelemetryStage::Matching,
                duration_ms: 1200,
            },
        ))
        .unwrap();

        assert_eq!(value["event"], "pipeline_stage_completed");
        assert_eq!(value["properties"]["stage"], "matching");
        assert_eq!(value["properties"]["durationMs"], 1200);
        assert!(value.get("stage").is_none());
        assert!(value.get("duration_ms").is_none());
    }

    #[test]
    fn every_event_variant_matches_the_production_properties_envelope() {
        let daily = serde_json::to_value(TelemetryPayload::new(
            Uuid::nil(),
            TelemetryEvent::DailyActive,
        ))
        .unwrap();
        assert_eq!(daily["properties"], serde_json::json!({}));

        let completed = serde_json::to_value(TelemetryPayload::new(
            Uuid::nil(),
            TelemetryEvent::GenerationCompleted {
                quality_preset: TelemetryQuality::High,
                total_duration_ms: 123_000,
                frame_count_bucket: FrameCountBucket::From301To500,
                duration_bucket: Some(DurationBucket::UpTo60Seconds),
            },
        ))
        .unwrap();
        assert_eq!(completed["properties"]["qualityPreset"], "high");
        assert_eq!(completed["properties"]["durationMs"], 123_000);
        assert_eq!(completed["properties"]["frameCountBucket"], "301-500");
        assert_eq!(completed["properties"]["inputDurationBucket"], "30-60s");

        let failed = serde_json::to_value(TelemetryPayload::new(
            Uuid::nil(),
            TelemetryEvent::GenerationFailed {
                stage: None,
                error_code: TelemetryErrorCode::InvalidInput,
            },
        ))
        .unwrap();
        assert_eq!(failed["properties"]["errorCode"], "invalid_input");
        assert!(failed["properties"].get("stage").is_none());
    }

    #[test]
    fn planner_evaluation_is_exact_anonymous_and_keeps_disabled_version_null() {
        let value = serde_json::to_value(TelemetryPayload::new(
            Uuid::nil(),
            TelemetryEvent::PlannerEvaluation {
                planner_schema_version: PLANNER_EFFECTIVENESS_SCHEMA_VERSION,
                run_id: Uuid::from_u128(7),
                run_kind: TelemetryRunKind::New,
                outcome: TelemetryPlannerOutcome::Completed,
                planner_enabled: false,
                planner_version: None,
                quality_preset: TelemetryQuality::Fast,
                input_type: TelemetryInputType::Video,
                total_duration_ms: 42_000,
                failure_stage: None,
                error_code: None,
                snapshot: Box::new(PlannerEffectivenessSnapshot {
                    frame_plan: Some(PlannerFramePlanMetrics {
                        source_width: 1_920,
                        source_height: 1_080,
                        source_item_count: 300,
                        initial_selected_count: 90,
                        candidate_count: 90,
                        ..PlannerFramePlanMetrics::default()
                    }),
                    ..PlannerEffectivenessSnapshot::default()
                }),
            },
        ))
        .unwrap();

        assert_eq!(value["event"], "planner_evaluation");
        assert_eq!(
            value["properties"]["plannerVersion"],
            serde_json::Value::Null
        );
        assert_eq!(value["properties"]["failureStage"], serde_json::Value::Null);
        assert_eq!(value["properties"]["errorCode"], serde_json::Value::Null);
        assert_eq!(value["properties"]["framePlan"]["sourceWidth"], 1_920);
        assert_eq!(
            value["properties"]["initialReconstruction"],
            serde_json::Value::Null
        );
        assert_eq!(value["properties"]["bridge"], serde_json::Value::Null);
        assert_eq!(value["properties"]["brush"], serde_json::Value::Null);
        assert_eq!(value["properties"]["finalResult"], serde_json::Value::Null);
        assert!(value["properties"].get("projectId").is_none());
        assert!(value["properties"].get("path").is_none());
        assert!(validate_privacy(&value));
    }
}
