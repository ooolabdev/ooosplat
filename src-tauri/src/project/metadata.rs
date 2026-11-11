use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    pipeline::PipelineStage,
    planner::BridgeBackfillCheckpoint,
    presets::{PlannerResolutionPlan, Quality, ResolvedBrushTrainingPreset},
    video::{FramePlan, ImageSequenceInfo, PlannedFrame, VideoInfo},
};

pub const PROJECT_APP_ID: &str = "studio.ooo.splat";
pub const COLMAP_INPUT_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectInputType {
    #[default]
    Video,
    Images,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageMatchingStrategy {
    Sequential,
    #[default]
    Exhaustive,
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
        if !(0.001..=10_000.0).contains(&self.scale) {
            return Err(crate::error::SplatError::Process(
                "Uniform Scale 必须位于 0.001–10000 之间".into(),
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum GaussianCrop {
    Sphere {
        center: [f64; 3],
        radius: f64,
        #[serde(default)]
        inverted: bool,
    },
    Box {
        center: [f64; 3],
        size: [f64; 3],
        #[serde(default)]
        inverted: bool,
    },
}

impl GaussianCrop {
    pub fn validate(self) -> crate::error::Result<Self> {
        let valid_vector = |values: &[f64; 3]| values.iter().all(|value| value.is_finite());
        let valid_extent = |value: f64| value.is_finite() && (0.000_001..=1.0e12).contains(&value);
        let valid = match self {
            Self::Sphere { center, radius, .. } => valid_vector(&center) && valid_extent(radius),
            Self::Box { center, size, .. } => {
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
            Self::Sphere {
                center,
                radius,
                inverted,
            } => {
                let inside = point
                    .iter()
                    .zip(center)
                    .map(|(value, origin)| (value - origin).powi(2))
                    .sum::<f64>()
                    <= radius * radius;
                inside != inverted
            }
            Self::Box {
                center,
                size,
                inverted,
            } => {
                let inside = point
                    .iter()
                    .zip(center)
                    .zip(size)
                    .all(|((value, origin), extent)| (value - origin).abs() <= extent * 0.5);
                inside != inverted
            }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImportedGaussianFormat {
    Ply,
    CompressedPly,
    Sog,
    Spz,
    Splat,
    Ksplat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalFileFingerprint {
    pub sha256: String,
    pub file_size: u64,
    pub modified_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedColmapProvenance {
    pub root_path: PathBuf,
    pub sparse_model_path: PathBuf,
    pub database_path: Option<PathBuf>,
    pub images_path: Option<PathBuf>,
    pub registered_images: u64,
    pub points_3d: u64,
    pub camera_count: u64,
    pub reshoot_eligible: bool,
    pub reshoot_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedProjectProvenance {
    pub source_model_path: PathBuf,
    pub source_format: ImportedGaussianFormat,
    pub source_fingerprint: ExternalFileFingerprint,
    /// Relative to the OOOSplat project root. Present for converted formats.
    pub canonical_model_path: Option<PathBuf>,
    #[serde(default)]
    pub colmap: Option<ImportedColmapProvenance>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMetadata {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub app_id: String,
    pub id: Uuid,
    /// Stable frontend draft identity used to reconcile a running task with
    /// the project created by the pipeline. Older projects do not have it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_task_id: Option<Uuid>,
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
    /// 高清补拍溯源：仅当本项目由补拍派生时存在。源项目始终只读。
    #[serde(default)]
    pub reshoot: Option<ReshootProvenance>,
    /// Provenance for an externally-owned Gaussian model. External files are
    /// references only; OOOSplat deletes only its own project and cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported: Option<ImportedProjectProvenance>,
}

/// Where a derived reshoot project came from, and what was added to it.
///
/// The source project stays read-only: this records the paths the derivation
/// read, so it is auditable, and it never implies the source was modified.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReshootProvenance {
    pub source_project_id: Uuid,
    pub source_project_path: PathBuf,
    /// The source's `final.ply`, kept as a reference for the user.
    pub source_final_ply: PathBuf,
    /// The reshoot media the user supplied.
    pub reshoot_source_path: PathBuf,
    #[serde(default)]
    pub camera_id: u32,
    #[serde(default)]
    pub camera_model: String,
    #[serde(default)]
    pub width: u64,
    #[serde(default)]
    pub height: u64,
    #[serde(default)]
    pub has_alpha: bool,
    #[serde(default)]
    pub mask_count: u64,
    #[serde(default)]
    pub source_image_count: u64,
    #[serde(default)]
    pub reshoot_frame_count: u64,
    #[serde(default)]
    pub registered_reshoot_count: u64,
}

pub const fn schema_version() -> u32 {
    9
}

fn default_model() -> String {
    "final.ply".into()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colmap_input_version: Option<u32>,
    #[serde(default)]
    pub initial_extracted_frames: u64,
    #[serde(default)]
    pub selected_frames: Vec<PlannedFrame>,
    #[serde(default)]
    pub candidate_frames: Vec<PlannedFrame>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge_source_frames: Option<u64>,
    #[serde(default)]
    pub rescue_max_frames: u64,
    #[serde(default)]
    pub minimum_frame_override_applied: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrushTrainingCheckpoint {
    /// Absent in existing projects: keep their historical ordinary/reshoot
    /// command-line semantics rather than silently changing Alpha handling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_options: Option<BrushExecutionOptions>,
    /// Content identity of an already prepared, strictly validated legacy
    /// training set. Kept separate from the modern derivation signature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_dataset_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_initialization_sha256: Option<String>,
    /// Frozen input/coordinate/preparation identity for the saved actual recipe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dataset_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fine_splat_budget: Option<crate::pipeline::splat_budget::FineSplatBudget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_resolved: Option<ResolvedBrushTrainingPreset>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved: Option<ResolvedBrushTrainingPreset>,
    #[serde(default)]
    pub oom_retry_used: bool,
    #[serde(default)]
    pub oom_retry_started: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oom_retry_source_count: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum AlphaInterpretation {
    #[default]
    RuntimeDefault,
    Masked,
    Transparent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrushExecutionOptions {
    pub version: u32,
    pub alpha: AlphaInterpretation,
}

impl Default for BrushExecutionOptions {
    fn default() -> Self {
        Self {
            version: 1,
            alpha: AlphaInterpretation::RuntimeDefault,
        }
    }
}

impl BrushExecutionOptions {
    pub fn alpha_mode(self) -> crate::error::Result<Option<&'static str>> {
        if self.version != 1 {
            return Err(crate::error::SplatError::Process(
                "Unsupported saved Brush execution options".into(),
            ));
        }
        Ok(match self.alpha {
            AlphaInterpretation::RuntimeDefault => None,
            AlphaInterpretation::Masked => Some("masked"),
            AlphaInterpretation::Transparent => Some("transparent"),
        })
    }
}

impl BrushTrainingCheckpoint {
    pub fn alpha_mode(&self, legacy_reshoot: bool) -> crate::error::Result<Option<&'static str>> {
        match self.execution_options {
            Some(options) => options.alpha_mode(),
            None => Ok(legacy_reshoot.then_some("transparent")),
        }
    }
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
            colmap_input_version: Some(COLMAP_INPUT_VERSION),
            initial_extracted_frames: plan.estimated_frames,
            selected_frames: plan.selected_frames.clone(),
            candidate_frames: plan.candidate_frames.clone(),
            bridge_source_frames: plan.bridge_source_frames,
            rescue_max_frames: plan.rescue_max_frames,
            minimum_frame_override_applied: plan.minimum_frame_override_applied,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineStateFile {
    /// Missing on historical tasks; never expand their BA scope on continue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ba_policy_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation: Option<crate::project::creation::ProjectCreationState>,
    /// Validated reconstruction/output facts for a completed Brush candidate.
    /// Allows publication recovery without rebuilding disposable training data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validated_output: Option<ProjectOutput>,
    #[serde(default)]
    pub artifact_manifests: crate::pipeline::artifacts::ArtifactManifestSet,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<crate::pipeline::inputs::InputCheckpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_policy: Option<crate::pipeline::generation_policy::GenerationPolicy>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retry_cache_pending: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_transparency: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gps: Option<crate::reconstruction::gps::Checkpoint>,
    /// Frozen at creation. Legacy image projects retain exhaustive matching.
    #[serde(default)]
    pub image_matching_strategy: ImageMatchingStrategy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) temporary_png_focal:
        Option<crate::video::temporary_png_focal_workaround::Checkpoint>,
    #[serde(default = "legacy_video_projection")]
    pub video_projection: crate::video::panorama::VideoProjection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panorama: Option<crate::video::panorama::PanoramaCheckpoint>,
    #[serde(default)]
    pub configuration: Option<crate::presets::PipelineOptimizationConfig>,
    pub stage: PipelineStage,
    pub preset: Quality,
    pub video: Option<VideoInfo>,
    #[serde(default)]
    pub input_type: ProjectInputType,
    #[serde(default)]
    pub image_sequence: Option<ImageSequenceInfo>,
    pub frames: Option<FrameState>,
    /// Missing on pre-v2 projects, which deliberately resume through the
    /// legacy pipeline rather than silently changing their reconstruction.
    #[serde(default)]
    pub planner_enabled: bool,
    /// Present only on projects created after the staged-resolution policy was
    /// introduced. Its absence deliberately keeps older checkpoints on their
    /// original resolution behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_policy_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_plan: Option<PlannerResolutionPlan>,
    #[serde(default)]
    pub bridge_backfill: BridgeBackfillCheckpoint,
    #[serde(default)]
    pub brush_training: BrushTrainingCheckpoint,
    pub features_complete: bool,
    pub matching_complete: bool,
    pub reconstruction_complete: bool,
    pub brush_complete: bool,
    #[serde(default)]
    pub reshoot: Option<ReshootState>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReshootState {
    /// Local evidence tying registered old/new views to the final model and,
    /// once validated, its immutable Gaussian candidate. Missing on old tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_proof: Option<crate::pipeline::reshoot_connection::ConnectionProof>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe: Option<crate::pipeline::reshoot::ReshootRecipe>,
    pub source_snapshot_complete: bool,
    pub supplemental_frames_complete: bool,
    pub supplemental_features_complete: bool,
    pub incremental_matching_complete: bool,
    pub incremental_reconstruction_complete: bool,
    #[serde(default)]
    pub source_image_count: u64,
    #[serde(default)]
    pub reshoot_frame_count: u64,
    #[serde(default)]
    pub mask_count: u64,
    #[serde(default)]
    pub has_alpha: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colmap_input_version: Option<u32>,
    #[serde(default)]
    pub registered_reshoot_count: u64,
    /// Content identity of the immutable source snapshot owned by this derived
    /// project. Missing on legacy/V2 checkpoints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_v3_identity: Option<String>,
    #[serde(default)]
    pub snapshot_v3_ready: bool,
}

fn legacy_video_projection() -> crate::video::panorama::VideoProjection {
    crate::video::panorama::VideoProjection::Perspective
}

impl PipelineStateFile {
    pub fn automatically_retried(&self) -> bool {
        self.generation_policy.is_some()
            && (self.brush_training.oom_retry_started
                || self
                    .reshoot
                    .as_ref()
                    .and_then(|r| r.recipe.as_ref())
                    .is_some_and(|r| r.reconstruction_fallbacks > 0 || r.training_fallbacks > 0))
    }
    pub fn created(preset: Quality) -> Self {
        Self::created_for(preset, ProjectInputType::Video)
    }

    pub fn created_for(preset: Quality, input_type: ProjectInputType) -> Self {
        Self {
            validated_output: None,
            ba_policy_version: None,
            artifact_manifests: Default::default(),
            inputs: None,
            // Older projects were always perspective. Never reinterpret their caches.
            generation_policy: None,
            retry_cache_pending: vec![],
            actual_transparency: None,
            gps: None,
            creation: None,
            temporary_png_focal: None,
            image_matching_strategy: ImageMatchingStrategy::default(),
            video_projection: crate::video::panorama::VideoProjection::Perspective,
            panorama: None,
            configuration: None,
            stage: PipelineStage::Created,
            preset,
            video: None,
            input_type,
            image_sequence: None,
            frames: None,
            planner_enabled: true,
            resolution_policy_version: None,
            resolution_plan: None,
            bridge_backfill: BridgeBackfillCheckpoint::default(),
            brush_training: BrushTrainingCheckpoint::default(),
            features_complete: false,
            matching_complete: false,
            reconstruction_complete: false,
            brush_complete: false,
            reshoot: None,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn missing_ba_scope_remains_legacy_without_a_schema_migration() {
        let state = super::PipelineStateFile::created(crate::presets::Quality::Balanced);
        let old = serde_json::to_value(&state).unwrap();
        assert!(old.get("baPolicyVersion").is_none());
        let restored: super::PipelineStateFile = serde_json::from_value(old).unwrap();
        assert_eq!(restored.ba_policy_version, None);
        let mut state = restored;
        state.ba_policy_version = Some(crate::pipeline::ba_policy::VERSION);
        let restored: super::PipelineStateFile =
            serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        assert_eq!(
            restored.ba_policy_version,
            Some(crate::pipeline::ba_policy::VERSION)
        );
    }
    use super::*;
    #[test]
    fn execution_options_are_explicit_for_new_tasks_and_absent_for_legacy_checkpoints() {
        let old: BrushTrainingCheckpoint =
            serde_json::from_value(serde_json::json!({"oomRetryUsed":true})).unwrap();
        assert_eq!(old.execution_options, None);
        assert!(old.oom_retry_used);
        assert_eq!(old.alpha_mode(false).unwrap(), None);
        assert_eq!(old.alpha_mode(true).unwrap(), Some("transparent"));
        let new = BrushTrainingCheckpoint {
            execution_options: Some(Default::default()),
            ..Default::default()
        };
        assert_eq!(
            new.alpha_mode(false).unwrap(),
            new.alpha_mode(true).unwrap()
        );
        assert_eq!(new.alpha_mode(true).unwrap(), None);
        assert_eq!(BrushExecutionOptions::default().alpha_mode().unwrap(), None);
        assert_eq!(
            BrushExecutionOptions {
                version: 1,
                alpha: AlphaInterpretation::Masked
            }
            .alpha_mode()
            .unwrap(),
            Some("masked")
        );
        assert_eq!(
            BrushExecutionOptions {
                version: 1,
                alpha: AlphaInterpretation::Transparent
            }
            .alpha_mode()
            .unwrap(),
            Some("transparent")
        );
        assert!(BrushExecutionOptions {
            version: 2,
            ..Default::default()
        }
        .alpha_mode()
        .is_err());
    }
    #[test]
    fn retry_badge_requires_a_started_controlled_retry_and_stays_legacy_safe() {
        let mut state = PipelineStateFile::created(Quality::High);
        state.generation_policy = Some(Default::default());
        assert!(!state.automatically_retried());
        state.brush_training.oom_retry_used = true;
        assert!(!state.automatically_retried());
        state.brush_training.oom_retry_started = true;
        assert!(state.automatically_retried());
        state.generation_policy = None;
        assert!(!state.automatically_retried());
    }
    #[test]
    fn image_matching_strategy_is_frozen_and_legacy_defaults_to_exhaustive() {
        let mut state = PipelineStateFile::created_for(Quality::Balanced, ProjectInputType::Images);
        state.image_matching_strategy = ImageMatchingStrategy::Sequential;
        state.matching_complete = true;
        let mut json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["imageMatchingStrategy"], "sequential");
        let restored: PipelineStateFile = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(
            restored.image_matching_strategy,
            ImageMatchingStrategy::Sequential
        );
        assert!(restored.matching_complete);
        json.as_object_mut()
            .unwrap()
            .remove("imageMatchingStrategy");
        let legacy: PipelineStateFile = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(
            legacy.image_matching_strategy,
            ImageMatchingStrategy::Exhaustive
        );
        json["imageMatchingStrategy"] = serde_json::json!("other");
        assert!(serde_json::from_value::<PipelineStateFile>(json).is_err());
    }

    #[test]
    fn state_uses_frame_strategy_vocabulary_only() {
        let json = serde_json::to_string(&PipelineStateFile::created(Quality::Balanced)).unwrap();
        assert!(json.contains("\"preset\":\"balanced\""));
        assert!(!json.contains("targetFrames"));
    }

    #[test]
    fn legacy_projects_do_not_reinterpret_two_to_one_frames_as_panorama() {
        let mut value = serde_json::to_value(PipelineStateFile::created(Quality::Fast)).unwrap();
        value.as_object_mut().unwrap().remove("videoProjection");
        value.as_object_mut().unwrap().remove("panorama");
        let legacy: PipelineStateFile = serde_json::from_value(value).unwrap();
        assert_eq!(
            legacy.video_projection,
            crate::video::panorama::VideoProjection::Perspective
        );
        assert!(legacy.panorama.is_none());
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
        assert_eq!(frames.colmap_input_version, None);
        assert!(!state.planner_enabled);
        assert!(state.resolution_policy_version.is_none());
        assert!(state.resolution_plan.is_none());
        assert_eq!(
            state.bridge_backfill.status,
            crate::planner::BridgeBackfillStatus::NotEvaluated
        );
        assert!(state.brush_training.resolved.is_none());
        assert!(!state.brush_training.oom_retry_used);
    }

    #[test]
    fn old_reshoot_state_defaults_colmap_input_version() {
        let json = r#"{
          "stage":"extractingFrames","preset":"balanced","video":null,
          "inputType":"images","frames":null,
          "featuresComplete":false,"matchingComplete":false,
          "reconstructionComplete":false,"brushComplete":false,
          "reshoot":{"sourceSnapshotComplete":true,"supplementalFramesComplete":true,
            "supplementalFeaturesComplete":false,"incrementalMatchingComplete":false,
            "incrementalReconstructionComplete":false}
        }"#;

        let state: PipelineStateFile = serde_json::from_str(json).unwrap();

        assert_eq!(
            state.reshoot.unwrap().colmap_input_version,
            None,
            "legacy reshoot checkpoints must not silently opt into the new image semantics"
        );
    }

    #[test]
    fn brush_training_checkpoint_round_trips_resolved_retry_state() {
        let mut state = PipelineStateFile::created(Quality::High);
        let initial = crate::presets::resolve_brush_training_preset(
            Quality::High,
            true,
            Some(12_288),
            7_680,
            350_000,
        );
        state.brush_training.initial_resolved = Some(initial);
        state.brush_training.resolved = initial.downgrade_after_oom(350_000);
        state.brush_training.oom_retry_used = true;

        let json = serde_json::to_string(&state).unwrap();
        let restored: PipelineStateFile = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.brush_training, state.brush_training);
    }

    #[test]
    fn panorama_recipe_snapshot_survives_resume_and_legacy_presets_stay_legacy() {
        let mut state = PipelineStateFile::created(Quality::High);
        let initial = crate::presets::resolve_panorama_training_preset(
            Quality::High,
            true,
            Some(8192),
            1024,
            10_192,
        )
        .unwrap();
        state.brush_training.initial_resolved = Some(initial);
        state.brush_training.resolved = initial.downgrade_after_oom(10_192);
        state.brush_training.oom_retry_used = true;
        let json = serde_json::to_vec(&state).unwrap();
        let restored: PipelineStateFile = serde_json::from_slice(&json).unwrap();
        assert_eq!(restored.brush_training, state.brush_training);
        assert_eq!(
            restored
                .brush_training
                .initial_resolved
                .unwrap()
                .preset
                .max_splats,
            Some(3_000_000)
        );
        assert_eq!(
            restored.brush_training.resolved.unwrap().preset.max_splats,
            Some(1_500_000)
        );

        let mut old = serde_json::to_value(crate::presets::resolve_brush_training_preset(
            Quality::Fast,
            true,
            Some(8192),
            512,
            10_192,
        ))
        .unwrap();
        old.as_object_mut().unwrap().remove("panorama");
        let restored: crate::presets::ResolvedBrushTrainingPreset =
            serde_json::from_value(old).unwrap();
        assert_eq!(restored.preset.total_steps, 8000);
        assert_eq!(restored.preset.max_splats, Some(500_000));
        assert!(restored.panorama.is_none());
    }

    #[test]
    fn staged_resolution_plan_round_trips_without_affecting_old_states() {
        let mut state = PipelineStateFile::created(Quality::High);
        state.resolution_policy_version = Some(crate::presets::PLANNER_RESOLUTION_POLICY_VERSION);
        state.resolution_plan = Some(crate::presets::resolve_planner_resolution_plan(
            Quality::High,
            Some(8_192),
            2_560,
            3_840,
            false,
        ));
        let json = serde_json::to_string(&state).unwrap();
        let restored: PipelineStateFile = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.resolution_policy_version, Some(1));
        assert_eq!(restored.resolution_plan.unwrap().working_long_edge(), 3_840);
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
        assert_eq!(metadata.workspace_task_id, None);
        assert_eq!(metadata.schema_version, 2);
    }

    #[test]
    fn rejects_invalid_transform_values() {
        assert!(GaussianTransform {
            scale: 10_000.0,
            ..GaussianTransform::default()
        }
        .validate()
        .is_ok());
        assert!(GaussianTransform {
            scale: 10_000.1,
            ..GaussianTransform::default()
        }
        .validate()
        .is_err());
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
            inverted: false,
        };
        assert!(crop.validate().is_ok());
        assert!(GaussianCrop::Box {
            center: [0.0; 3],
            size: [1.0, 0.0, 1.0],
            inverted: false,
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

    #[test]
    fn inverted_crops_keep_the_complement_and_old_projects_default_to_inside() {
        for json in [
            r#"{"kind":"sphere","center":[0,0,0],"radius":1}"#,
            r#"{"kind":"box","center":[0,0,0],"size":[2,2,2]}"#,
        ] {
            let normal: GaussianCrop = serde_json::from_str(json).unwrap();
            let inverted: GaussianCrop =
                serde_json::from_str(&json.replace('}', ",\"inverted\":true}")).unwrap();
            assert!(normal.contains([0.0, 0.0, 0.0]));
            assert!(!inverted.contains([0.0, 0.0, 0.0]));
            assert!(!normal.contains([3.0, 0.0, 0.0]));
            assert!(inverted.contains([3.0, 0.0, 0.0]));
            assert!(inverted.validate().is_ok());
        }
    }
}
