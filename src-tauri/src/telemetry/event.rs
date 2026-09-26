use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::{pipeline::PipelineStage, presets::Quality};

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
    DiskSpaceLow,
    IoFailed,
    Unknown,
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
    QualityMetricsRecorded {
        actual_frame_count: u64,
        actual_sfm_resolution: u32,
        actual_feature_count: Option<u64>,
        actual_brush_resolution: u32,
        actual_brush_iterations: usize,
        registered_images: u64,
        reprojection_error: Option<f64>,
        splat_count: u64,
        peak_gpu_memory_mb: Option<u64>,
    },
    PlannerMetricsRecorded {
        planner_version: u32,
        capture_type: Option<String>,
        pairing_planned: Option<String>,
        pairing_actual: Option<String>,
        mapper_planned: Option<String>,
        mapper_actual: Option<String>,
        largest_component_ratio: Option<f32>,
        two_core_ratio: Option<f32>,
        bridge_ratio: Option<f32>,
        normal_rescue_rounds: u32,
        success_recovery_rounds: u32,
        normal_budget_exhausted: bool,
        success_recovery_entered: bool,
        budget_overridden_for_success: bool,
        normal_duration_ms: u64,
        recovery_duration_ms: u64,
        reconstruction_quality: Option<String>,
    },
    GeometryScreeningRecorded {
        geometry_threshold_profile: String,
        points_3d: u64,
        observations: u64,
        mean_track_length: f64,
        point_diversity_ratio: f64,
        median_triangulation_ratio: f64,
        p25_triangulation_ratio: f64,
        minimum_triangulation_ratio: f64,
        weak_geometry_interval_count: usize,
        weak_geometry_image_count: usize,
        triangulation_underfilled: bool,
        track_redundancy_high: bool,
        continuous_weak_region: bool,
        geometry_screening_decision: String,
    },
    GeometryProbeRecorded {
        geometry_probe_reasons: Vec<String>,
        requested_additional_frames: usize,
        actual_additional_frames: usize,
        baseline_points: u64,
        probe_points: Option<u64>,
        point_gain_ratio: Option<f64>,
        baseline_observations: u64,
        probe_observations: Option<u64>,
        observation_gain_ratio: Option<f64>,
        baseline_track_length: Option<f64>,
        probe_track_length: Option<f64>,
        baseline_reprojection_error: Option<f64>,
        probe_reprojection_error: Option<f64>,
        geometry_probe_duration_ms: u64,
    },
    GenerationFailed {
        stage: Option<TelemetryStage>,
        error_code: TelemetryErrorCode,
    },
    PipelineStageCompleted {
        stage: TelemetryStage,
        duration_ms: u64,
    },
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum TelemetryEventName {
    DailyActive,
    GenerationStarted,
    GenerationCompleted,
    QualityMetricsRecorded,
    PlannerMetricsRecorded,
    GeometryScreeningRecorded,
    GeometryProbeRecorded,
    GenerationFailed,
    PipelineStageCompleted,
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
    actual_frame_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    actual_sfm_resolution: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    actual_feature_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    actual_brush_resolution: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    actual_brush_iterations: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    registered_images: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reprojection_error: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    splat_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    peak_gpu_memory_mb: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    planner_version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    capture_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pairing_planned: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pairing_actual: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mapper_planned: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mapper_actual: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    largest_component_ratio: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    two_core_ratio: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bridge_ratio: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    normal_rescue_rounds: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    success_recovery_rounds: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    normal_budget_exhausted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    success_recovery_entered: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    budget_overridden_for_success: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    normal_duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recovery_duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reconstruction_quality: Option<String>,
    #[serde(rename = "points3D", skip_serializing_if = "Option::is_none")]
    points_3d: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    observations: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mean_track_length: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    point_diversity_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    median_triangulation_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    p25_triangulation_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    minimum_triangulation_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    weak_geometry_interval_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    weak_geometry_image_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    triangulation_underfilled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    track_redundancy_high: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    continuous_weak_region: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    geometry_screening_decision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    geometry_threshold_profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    geometry_probe_reasons: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    requested_additional_frames: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    actual_additional_frames: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    baseline_points: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    probe_points: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    point_gain_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    baseline_observations: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    probe_observations: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    observation_gain_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    baseline_track_length: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    probe_track_length: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    baseline_reprojection_error: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    probe_reprojection_error: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    geometry_probe_duration_ms: Option<u64>,
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
            Self::QualityMetricsRecorded {
                actual_frame_count,
                actual_sfm_resolution,
                actual_feature_count,
                actual_brush_resolution,
                actual_brush_iterations,
                registered_images,
                reprojection_error,
                splat_count,
                peak_gpu_memory_mb,
            } => (
                TelemetryEventName::QualityMetricsRecorded,
                TelemetryProperties {
                    actual_frame_count: Some(actual_frame_count),
                    actual_sfm_resolution: Some(actual_sfm_resolution),
                    actual_feature_count,
                    actual_brush_resolution: Some(actual_brush_resolution),
                    actual_brush_iterations: Some(actual_brush_iterations),
                    registered_images: Some(registered_images),
                    reprojection_error,
                    splat_count: Some(splat_count),
                    peak_gpu_memory_mb,
                    ..TelemetryProperties::default()
                },
            ),
            Self::PlannerMetricsRecorded {
                planner_version,
                capture_type,
                pairing_planned,
                pairing_actual,
                mapper_planned,
                mapper_actual,
                largest_component_ratio,
                two_core_ratio,
                bridge_ratio,
                normal_rescue_rounds,
                success_recovery_rounds,
                normal_budget_exhausted,
                success_recovery_entered,
                budget_overridden_for_success,
                normal_duration_ms,
                recovery_duration_ms,
                reconstruction_quality,
            } => (
                TelemetryEventName::PlannerMetricsRecorded,
                TelemetryProperties {
                    planner_version: Some(planner_version),
                    capture_type,
                    pairing_planned,
                    pairing_actual,
                    mapper_planned,
                    mapper_actual,
                    largest_component_ratio,
                    two_core_ratio,
                    bridge_ratio,
                    normal_rescue_rounds: Some(normal_rescue_rounds),
                    success_recovery_rounds: Some(success_recovery_rounds),
                    normal_budget_exhausted: Some(normal_budget_exhausted),
                    success_recovery_entered: Some(success_recovery_entered),
                    budget_overridden_for_success: Some(budget_overridden_for_success),
                    normal_duration_ms: Some(normal_duration_ms.min(86_400_000)),
                    recovery_duration_ms: Some(recovery_duration_ms.min(86_400_000)),
                    reconstruction_quality,
                    ..TelemetryProperties::default()
                },
            ),
            Self::GeometryScreeningRecorded {
                geometry_threshold_profile,
                points_3d,
                observations,
                mean_track_length,
                point_diversity_ratio,
                median_triangulation_ratio,
                p25_triangulation_ratio,
                minimum_triangulation_ratio,
                weak_geometry_interval_count,
                weak_geometry_image_count,
                triangulation_underfilled,
                track_redundancy_high,
                continuous_weak_region,
                geometry_screening_decision,
            } => (
                TelemetryEventName::GeometryScreeningRecorded,
                TelemetryProperties {
                    geometry_threshold_profile: Some(geometry_threshold_profile),
                    points_3d: Some(points_3d),
                    observations: Some(observations),
                    mean_track_length: Some(mean_track_length),
                    point_diversity_ratio: Some(point_diversity_ratio),
                    median_triangulation_ratio: Some(median_triangulation_ratio),
                    p25_triangulation_ratio: Some(p25_triangulation_ratio),
                    minimum_triangulation_ratio: Some(minimum_triangulation_ratio),
                    weak_geometry_interval_count: Some(weak_geometry_interval_count),
                    weak_geometry_image_count: Some(weak_geometry_image_count),
                    triangulation_underfilled: Some(triangulation_underfilled),
                    track_redundancy_high: Some(track_redundancy_high),
                    continuous_weak_region: Some(continuous_weak_region),
                    geometry_screening_decision: Some(geometry_screening_decision),
                    ..TelemetryProperties::default()
                },
            ),
            Self::GeometryProbeRecorded {
                geometry_probe_reasons,
                requested_additional_frames,
                actual_additional_frames,
                baseline_points,
                probe_points,
                point_gain_ratio,
                baseline_observations,
                probe_observations,
                observation_gain_ratio,
                baseline_track_length,
                probe_track_length,
                baseline_reprojection_error,
                probe_reprojection_error,
                geometry_probe_duration_ms,
            } => (
                TelemetryEventName::GeometryProbeRecorded,
                TelemetryProperties {
                    geometry_probe_reasons: Some(geometry_probe_reasons),
                    requested_additional_frames: Some(requested_additional_frames),
                    actual_additional_frames: Some(actual_additional_frames),
                    baseline_points: Some(baseline_points),
                    probe_points,
                    point_gain_ratio,
                    baseline_observations: Some(baseline_observations),
                    probe_observations,
                    observation_gain_ratio,
                    baseline_track_length,
                    probe_track_length,
                    baseline_reprojection_error,
                    probe_reprojection_error,
                    geometry_probe_duration_ms: Some(geometry_probe_duration_ms.min(86_400_000)),
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

        let metrics = serde_json::to_value(TelemetryPayload::new(
            Uuid::nil(),
            TelemetryEvent::QualityMetricsRecorded {
                actual_frame_count: 240,
                actual_sfm_resolution: 1_600,
                actual_feature_count: None,
                actual_brush_resolution: 2_400,
                actual_brush_iterations: 30_000,
                registered_images: 220,
                reprojection_error: Some(0.42),
                splat_count: 1_500_000,
                peak_gpu_memory_mb: None,
            },
        ))
        .unwrap();
        assert_eq!(metrics["event"], "quality_metrics_recorded");
        assert_eq!(metrics["properties"]["actualFrameCount"], 240);
        assert_eq!(metrics["properties"]["actualBrushIterations"], 30_000);
        assert!(metrics["properties"].get("actualFeatureCount").is_none());
        assert!(metrics["properties"].get("peakGpuMemoryMb").is_none());
        assert!(validate_privacy(&metrics));

        let geometry = serde_json::to_value(TelemetryPayload::new(
            Uuid::nil(),
            TelemetryEvent::GeometryScreeningRecorded {
                geometry_threshold_profile: "provisional_video002_v1".into(),
                points_3d: 48_689,
                observations: 980_482,
                mean_track_length: 20.138,
                point_diversity_ratio: 0.04966,
                median_triangulation_ratio: 0.32,
                p25_triangulation_ratio: 0.18,
                minimum_triangulation_ratio: 0.01,
                weak_geometry_interval_count: 1,
                weak_geometry_image_count: 5,
                triangulation_underfilled: false,
                track_redundancy_high: true,
                continuous_weak_region: true,
                geometry_screening_decision: "probe_recommended".into(),
            },
        ))
        .unwrap();
        assert_eq!(geometry["event"], "geometry_screening_recorded");
        assert_eq!(geometry["properties"]["points3D"], 48_689);
        assert_eq!(
            geometry["properties"]["geometryThresholdProfile"],
            "provisional_video002_v1"
        );
        assert!(validate_privacy(&geometry));

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
}
