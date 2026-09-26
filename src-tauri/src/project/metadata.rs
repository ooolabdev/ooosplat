use std::{collections::BTreeMap, path::PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    pipeline::PipelineStage,
    planner::{GeometryProbeMetrics, GeometryScreeningReport, PlannerCheckpoint},
    presets::Quality,
    video::{
        FramePlan, FramePlanningMode, ImageSequenceInfo, MinimumFrameProtection, PlannedFrame,
        VideoInfo,
    },
};

pub const PROJECT_APP_ID: &str = "studio.ooo.splat";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectInputType {
    #[default]
    Video,
    Images,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GaussianTransform {
    pub position: [f64; 3],
    pub rotation: [f64; 3],
    pub scale: f64,
}

impl Default for GaussianTransform {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            rotation: [0.0; 3],
            scale: 1.0,
        }
    }
}

impl GaussianTransform {
    pub fn validate(self) -> crate::error::Result<Self> {
        if self
            .position
            .iter()
            .chain(self.rotation.iter())
            .any(|value| !value.is_finite())
            || !self.scale.is_finite()
        {
            return Err(crate::error::SplatError::Process(
                "Transform 包含无效数值".into(),
            ));
        }
        if !(0.001..=1000.0).contains(&self.scale) {
            return Err(crate::error::SplatError::Process(
                "Uniform Scale 必须位于 0.001–1000 之间".into(),
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum GaussianCrop {
    Sphere { center: [f64; 3], radius: f64 },
    Box { center: [f64; 3], size: [f64; 3] },
}

impl GaussianCrop {
    pub fn validate(self) -> crate::error::Result<Self> {
        let valid_vector = |values: &[f64; 3]| values.iter().all(|value| value.is_finite());
        let valid_extent = |value: f64| value.is_finite() && (0.000_001..=1.0e12).contains(&value);
        let valid = match self {
            Self::Sphere { center, radius } => valid_vector(&center) && valid_extent(radius),
            Self::Box { center, size } => {
                valid_vector(&center) && size.iter().all(|value| valid_extent(*value))
            }
        };
        if !valid {
            return Err(crate::error::SplatError::Process(
                "Gaussian 裁切区域包含无效的位置或尺寸".into(),
            ));
        }
        Ok(self)
    }

    pub fn contains(self, point: [f64; 3]) -> bool {
        match self {
            Self::Sphere { center, radius } => {
                point
                    .iter()
                    .zip(center)
                    .map(|(value, origin)| (value - origin).powi(2))
                    .sum::<f64>()
                    <= radius * radius
            }
            Self::Box { center, size } => point
                .iter()
                .zip(center)
                .zip(size)
                .all(|((value, origin), extent)| (value - origin).abs() <= extent * 0.5),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GaussianEditing {
    #[serde(default)]
    pub crop: Option<GaussianCrop>,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub source_splat_count: u64,
    #[serde(default)]
    pub deleted_count: u64,
}

impl GaussianEditing {
    pub fn validate(self, expected_splats: u64) -> crate::error::Result<Self> {
        if self.source_splat_count != 0 && self.source_splat_count != expected_splats {
            return Err(crate::error::SplatError::Process(
                "编辑状态与当前 Gaussian 文件的 Splat 数量不一致".into(),
            ));
        }
        if self.deleted_count > expected_splats {
            return Err(crate::error::SplatError::Process(
                "编辑状态中的删除数量无效".into(),
            ));
        }
        if let Some(crop) = self.crop {
            crop.validate()?;
        }
        Ok(Self {
            source_splat_count: expected_splats,
            ..self
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectStatus {
    #[default]
    Running,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectOutput {
    pub final_ply: PathBuf,
    pub file_size: u64,
    pub splat_count: u64,
    pub input_images: u64,
    pub registered_images: u64,
    pub registered_ratio: f64,
    pub points_3d: u64,
    #[serde(default)]
    pub quality_metrics: QualityRunMetrics,
}

/// Non-sensitive benchmark facts produced by a run. This intentionally has no
/// image data, source names, or filesystem paths.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct QualityRunMetrics {
    pub actual_frame_count: u64,
    pub actual_sfm_resolution: u32,
    pub actual_feature_count: Option<u64>,
    pub mean_features_per_image: Option<f64>,
    pub median_features_per_image: Option<f64>,
    pub raw_matches: Option<u64>,
    pub geometrically_verified_pairs: Option<u64>,
    pub verified_correspondences: Option<u64>,
    pub colmap_high_quality_experiment: bool,
    pub actual_brush_resolution: u32,
    pub actual_brush_iterations: usize,
    pub registered_images: u64,
    pub reprojection_error: Option<f64>,
    pub splat_count: u64,
    pub stage_durations_ms: BTreeMap<String, u64>,
    pub peak_gpu_memory_mb: Option<u64>,
    pub planner_enabled: bool,
    pub planner_version: Option<u32>,
    pub capture_type: Option<String>,
    pub pairing_planned: Option<String>,
    pub pairing_actual: Option<String>,
    pub mapper_planned: Option<String>,
    pub mapper_actual: Option<String>,
    pub largest_component_ratio: Option<f32>,
    pub two_core_ratio: Option<f32>,
    pub bridge_ratio: Option<f32>,
    pub normal_rescue_rounds: u32,
    pub success_recovery_rounds: u32,
    pub normal_budget_exhausted: bool,
    pub success_recovery_entered: bool,
    pub budget_overridden_for_success: bool,
    pub normal_duration_ms: u64,
    pub recovery_duration_ms: u64,
    pub reconstruction_quality: Option<String>,
    pub geometry_screening: Option<GeometryScreeningReport>,
    pub geometry_probe: Option<GeometryProbeMetrics>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMetadata {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub app_id: String,
    pub id: Uuid,
    #[serde(default)]
    pub name: String,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub status: ProjectStatus,
    pub source_path: PathBuf,
    #[serde(default)]
    pub input_type: ProjectInputType,
    pub quality: Quality,
    #[serde(default)]
    pub project_path: PathBuf,
    #[serde(default)]
    pub output_path: Option<PathBuf>,
    #[serde(default)]
    pub output: Option<ProjectOutput>,
    #[serde(default)]
    pub failure_message: Option<String>,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub transform: GaussianTransform,
    #[serde(default)]
    pub editing: GaussianEditing,
}

pub const fn schema_version() -> u32 {
    8
}

fn default_model() -> String {
    "final.ply".into()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FrameState {
    #[serde(default)]
    pub quality: Option<Quality>,
    pub retention_ratio: f64,
    pub sampling_fps: f64,
    #[serde(default)]
    pub actual_average_fps: f64,
    #[serde(default)]
    pub target_fps: f64,
    #[serde(default)]
    pub candidate_fps: f64,
    #[serde(default)]
    pub planning_mode: FramePlanningMode,
    #[serde(default)]
    pub preferred_fps: f64,
    #[serde(default)]
    pub selected_frames: Vec<PlannedFrame>,
    #[serde(default)]
    pub candidate_frames: Vec<PlannedFrame>,
    #[serde(flatten)]
    pub minimum_frame_protection: MinimumFrameProtection,
    pub estimated_frames: u64,
    pub extracted_frames: Option<u64>,
    #[serde(default)]
    pub image_format: Option<String>,
    #[serde(default)]
    pub mask_count: Option<u64>,
    #[serde(default)]
    pub has_alpha: bool,
}

impl From<&FramePlan> for FrameState {
    fn from(plan: &FramePlan) -> Self {
        Self {
            quality: plan.quality,
            retention_ratio: plan.retention_ratio,
            sampling_fps: plan.sampling_fps,
            actual_average_fps: plan.actual_average_fps,
            target_fps: plan.target_fps,
            candidate_fps: plan.candidate_fps,
            planning_mode: plan.planning_mode,
            preferred_fps: plan.preferred_fps,
            selected_frames: plan.selected_frames.clone(),
            candidate_frames: plan.candidate_frames.clone(),
            minimum_frame_protection: plan.minimum_frame_protection.clone(),
            estimated_frames: plan.estimated_frames,
            extracted_frames: None,
            image_format: None,
            mask_count: None,
            has_alpha: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineStateFile {
    pub stage: PipelineStage,
    pub preset: Quality,
    pub video: Option<VideoInfo>,
    #[serde(default)]
    pub input_type: ProjectInputType,
    #[serde(default)]
    pub image_sequence: Option<ImageSequenceInfo>,
    pub frames: Option<FrameState>,
    /// Snapshotted at project creation so resume never changes extraction mode.
    #[serde(default)]
    pub planner_enabled: bool,
    /// Versioned, authoritative Planner decisions and actual execution state.
    /// Old checkpoints migrate to `None` and continue through the legacy path.
    #[serde(default)]
    pub planner: Option<PlannerCheckpoint>,
    /// Snapshotted experimental COLMAP tuning so resumed work cannot mix A/B
    /// parameters within one database or sparse reconstruction.
    #[serde(default)]
    pub colmap_high_quality_experiment: Option<bool>,
    pub features_complete: bool,
    pub matching_complete: bool,
    pub reconstruction_complete: bool,
    pub brush_complete: bool,
}

impl PipelineStateFile {
    pub fn created(preset: Quality) -> Self {
        Self::created_for(preset, ProjectInputType::Video)
    }

    pub fn created_for(preset: Quality, input_type: ProjectInputType) -> Self {
        Self {
            stage: PipelineStage::Created,
            preset,
            video: None,
            input_type,
            image_sequence: None,
            frames: None,
            planner_enabled: false,
            planner: None,
            colmap_high_quality_experiment: None,
            features_complete: false,
            matching_complete: false,
            reconstruction_complete: false,
            brush_complete: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn state_uses_frame_strategy_vocabulary_only() {
        let json = serde_json::to_string(&PipelineStateFile::created(Quality::Balanced)).unwrap();
        assert!(json.contains("\"preset\":\"balanced\""));
        assert!(!json.contains("targetFrames"));
    }

    #[test]
    fn old_pipeline_state_defaults_transparency_fields() {
        let json = r#"{
          "stage":"extractingFrames",
          "preset":"balanced",
          "video":{
            "duration":10.0,"width":1920,"height":1080,"fps":30.0,
            "totalFrames":300,"codec":"h264","rotation":0
          },
          "frames":{
            "retentionRatio":0.5,"samplingFps":15.0,"estimatedFrames":150,
            "extractedFrames":150
          },
          "featuresComplete":false,"matchingComplete":false,
          "reconstructionComplete":false,"brushComplete":false
        }"#;
        let state: PipelineStateFile = serde_json::from_str(json).unwrap();
        let video = state.video.unwrap();
        assert_eq!(video.pixel_format, "");
        assert!(!video.has_alpha);
        let frames = state.frames.unwrap();
        assert_eq!(frames.image_format, None);
        assert_eq!(frames.mask_count, None);
        assert!(!frames.has_alpha);
        assert!(!state.planner_enabled);
        assert!(state.planner.is_none());
        assert_eq!(frames.planning_mode, FramePlanningMode::Legacy);
    }

    #[test]
    fn schema_two_metadata_defaults_to_identity_transform() {
        let json = r#"{
          "schemaVersion":2,"appId":"studio.ooo.splat","id":"00000000-0000-0000-0000-000000000001",
          "name":"legacy","createdAt":"2026-01-01T00:00:00Z","sourcePath":"input.mp4","quality":"balanced",
          "projectPath":"C:/legacy"
        }"#;
        let metadata: ProjectMetadata = serde_json::from_str(json).unwrap();
        assert_eq!(metadata.model, "final.ply");
        assert_eq!(metadata.transform, GaussianTransform::default());
        assert_eq!(metadata.editing, GaussianEditing::default());
        assert_eq!(metadata.input_type, ProjectInputType::Video);
        assert_eq!(metadata.schema_version, 2);
    }

    #[test]
    fn rejects_invalid_transform_values() {
        assert!(GaussianTransform {
            scale: 0.0,
            ..GaussianTransform::default()
        }
        .validate()
        .is_err());
        assert!(GaussianTransform {
            position: [f64::NAN, 0.0, 0.0],
            ..GaussianTransform::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn validates_crop_and_edit_counts() {
        let crop = GaussianCrop::Sphere {
            center: [0.0, 1.0, 2.0],
            radius: 4.0,
        };
        assert!(crop.validate().is_ok());
        assert!(GaussianCrop::Box {
            center: [0.0; 3],
            size: [1.0, 0.0, 1.0],
        }
        .validate()
        .is_err());
        assert!(GaussianEditing {
            crop: Some(crop),
            revision: 1,
            source_splat_count: 9,
            deleted_count: 10,
        }
        .validate(9)
        .is_err());
    }
}
