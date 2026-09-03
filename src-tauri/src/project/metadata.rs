use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    pipeline::PipelineStage,
    presets::Quality,
    video::{FramePlan, VideoInfo},
};

pub const PROJECT_APP_ID: &str = "studio.ooo.splat";

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
}

pub const fn schema_version() -> u32 {
    3
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
}

impl From<&FramePlan> for FrameState {
    fn from(plan: &FramePlan) -> Self {
        Self {
            retention_ratio: plan.retention_ratio,
            sampling_fps: plan.sampling_fps,
            estimated_frames: plan.estimated_frames,
            extracted_frames: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineStateFile {
    pub stage: PipelineStage,
    pub preset: Quality,
    pub video: Option<VideoInfo>,
    pub frames: Option<FrameState>,
    pub features_complete: bool,
    pub matching_complete: bool,
    #[serde(default)]
    pub matching_strategy_version: u32,
    pub reconstruction_complete: bool,
    pub brush_complete: bool,
}

impl PipelineStateFile {
    pub fn created(preset: Quality) -> Self {
        Self {
            stage: PipelineStage::Created,
            preset,
            video: None,
            frames: None,
            features_complete: false,
            matching_complete: false,
            matching_strategy_version: 1,
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
    fn schema_two_metadata_defaults_to_identity_transform() {
        let json = r#"{
          "schemaVersion":2,"appId":"studio.ooo.splat","id":"00000000-0000-0000-0000-000000000001",
          "name":"legacy","createdAt":"2026-01-01T00:00:00Z","sourcePath":"input.mp4","quality":"balanced",
          "projectPath":"C:/legacy"
        }"#;
        let metadata: ProjectMetadata = serde_json::from_str(json).unwrap();
        assert_eq!(metadata.model, "final.ply");
        assert_eq!(metadata.transform, GaussianTransform::default());
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
}
