use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    pipeline::PipelineStage,
    presets::Quality,
    video::{FramePlan, ImageSequenceInfo, VideoInfo},
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReshootProvenance {
    pub source_project_id: Uuid,
    pub source_project_path: PathBuf,
    pub source_final_ply: PathBuf,
    pub reshoot_source_path: PathBuf,
    pub regions: Vec<GaussianCrop>,
    pub guidance: Vec<String>,
    /// Annotated guide images written into the derived project: the circled
    /// region plus arrows marking where each shot should be taken from.
    #[serde(default)]
    pub guidance_images: Vec<PathBuf>,
    #[serde(default)]
    pub original_frame_count: u64,
    #[serde(default)]
    pub reshoot_frame_count: u64,
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
    #[serde(default)]
    pub reshoot: Option<ReshootProvenance>,
}

pub const fn schema_version() -> u32 {
    6
}

fn default_model() -> String {
    "final.ply".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameState {
    pub retention_ratio: f64,
    pub sampling_fps: f64,
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
            retention_ratio: plan.retention_ratio,
            sampling_fps: plan.sampling_fps,
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
        assert!(metadata.reshoot.is_none());
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
