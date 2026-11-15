//! Versioned reshoot decisions. Legacy tasks have no recipe and keep their old path.
use serde::{Deserialize, Serialize};

use crate::{
    presets::{BrushTrainingPreset, Quality},
    project::ImageMatchingStrategy,
};

pub const RECIPE_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModelCondition {
    Good,
    Appearance,
    Geometry,
    Unsure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReconstructionMode {
    Incremental,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrainingMode {
    Warm,
    Cold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CameraRelation {
    Same,
    Different,
    Unsure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraAssignment {
    pub version: u32,
    pub reused_camera_id: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReshootOptions {
    pub condition: ModelCondition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_relation: Option<CameraRelation>,
    pub allow_fallback: bool,
    pub source_identity: String,
    pub input_identity: String,
    pub projection: crate::video::panorama::VideoProjection,
    #[serde(default)]
    pub image_matching_strategy: ImageMatchingStrategy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReshootPlan {
    pub reconstruction: ReconstructionMode,
    pub training: TrainingMode,
}

impl ReshootPlan {
    pub fn resolve(condition: ModelCondition, incremental: bool, warm: bool) -> Self {
        let reconstruction = if condition == ModelCondition::Geometry || !incremental {
            ReconstructionMode::Full
        } else {
            ReconstructionMode::Incremental
        };
        let training = if reconstruction == ReconstructionMode::Incremental
            && warm
            && condition == ModelCondition::Good
        {
            TrainingMode::Warm
        } else {
            TrainingMode::Cold
        };
        Self {
            reconstruction,
            training,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WarmStartParameters {
    pub growth_start: u32,
    pub sh_degree: u32,
    pub lr_mean: f64,
    pub lr_mean_end: f64,
    pub lr_coeffs_dc: f64,
    pub lr_opac: f64,
    pub lr_scale: f64,
    pub lr_rotation: f64,
    pub mean_noise_weight: f32,
}

pub fn warm_training(
    mut preset: BrushTrainingPreset,
    quality: Quality,
    views: u64,
    initial_points: u64,
    sh_degree: u32,
) -> crate::error::Result<(BrushTrainingPreset, WarmStartParameters)> {
    let initial_points = u32::try_from(initial_points).map_err(|_| {
        crate::error::SplatError::Process("Gaussian point count exceeds the encoder range".into())
    })?;
    if initial_points == 0 || sh_degree > 3 {
        return Err(crate::error::SplatError::Process(
            "Invalid Gaussian initialization".into(),
        ));
    }
    let base = match quality {
        Quality::Fast => 3_000,
        Quality::Balanced => 5_000,
        Quality::High => 8_000,
    };
    preset.total_steps = preset
        .total_steps
        .min(base.max(views.saturating_mul(10).min(usize::MAX as u64) as usize));
    preset.refine_every = 200;
    preset.max_splats = Some(preset.max_splats.unwrap_or(10_000_000).max(initial_points));
    let start = ((preset.total_steps as f64 * 0.15 / 200.).ceil() as u32) * 200;
    let stop = ((preset.total_steps as f64 * 0.8 / 200.).floor() as u32) * 200;
    {
        // A custom full recipe may defer to the pinned Brush CLI defaults.
        let density =
            preset
                .densification
                .get_or_insert(crate::presets::BrushDensificationPreset {
                    growth_grad_threshold: 0.0025,
                    growth_select_fraction: 0.25,
                    growth_stop_iter: stop,
                });
        density.growth_grad_threshold *= 1.5;
        density.growth_select_fraction *= 0.5;
        density.growth_stop_iter = stop;
    }
    Ok((
        preset,
        WarmStartParameters {
            growth_start: start.min(stop),
            sh_degree,
            lr_mean: 2e-5 * 0.25,
            lr_mean_end: 2e-7 * 0.25,
            lr_coeffs_dc: 2e-3 * 0.5,
            lr_opac: 0.012 * 0.5,
            lr_scale: 5e-3 * 0.5,
            lr_rotation: 2e-3 * 0.5,
            mean_noise_weight: 50. * 0.1,
        },
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlgorithmFailure {
    NoNewRegistrations,
    CoordinateMismatch,
    InvalidWarmResult,
}

/// A nonzero exit alone (including panic 101) is never an algorithm failure.
pub fn explicit_numeric_divergence(error: &crate::error::SplatError) -> bool {
    let crate::error::SplatError::EngineProcess(failure) = error else {
        return false;
    };
    failure.engine == crate::pipeline::PipelineEngine::Brush
        && failure.brush_failure_reason
            == Some(crate::failure::BrushFailureReason::NumericDivergence)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReshootRecipe {
    pub version: u32,
    /// Absent in existing tasks: preserve their shortened warm-start recipe.
    #[serde(default)]
    pub normal_parameters_version: Option<u32>,
    #[serde(default)]
    pub camera_assignment: Option<CameraAssignment>,
    pub options: ReshootOptions,
    pub initial: ReshootPlan,
    pub actual: ReshootPlan,
    pub batch_id: String,
    #[serde(default)]
    pub reconstruction_fallbacks: u8,
    #[serde(default)]
    pub training_fallbacks: u8,
    #[serde(default)]
    pub expanded_matching: bool,
    #[serde(default)]
    pub source_names: Vec<String>,
    #[serde(default)]
    pub new_names: Vec<String>,
    #[serde(default)]
    pub warm_parameters: Option<WarmStartParameters>,
    #[serde(default)]
    pub init_sha256: Option<String>,
    #[serde(default)]
    pub init_georeference: Option<String>,
    #[serde(default)]
    pub stored_input_identity: Option<String>,
    #[serde(default)]
    pub panorama_frames: Vec<PanoramaFrameRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanoramaFrameRecord {
    pub prepared_name: String,
    pub batch_id: String,
    pub source_name: String,
    #[serde(default)]
    pub source_sha256: String,
    #[serde(default)]
    pub orientation: Option<u8>,
    pub source_index: u64,
    pub timestamp_seconds: Option<f64>,
    pub pts: Option<i64>,
}

impl ReshootRecipe {
    pub fn uses_normal_parameters(&self) -> bool {
        self.normal_parameters_version == Some(1)
    }

    pub fn fallback(&mut self, failure: AlgorithmFailure) -> bool {
        if !self.options.allow_fallback {
            return false;
        }
        match failure {
            AlgorithmFailure::NoNewRegistrations | AlgorithmFailure::CoordinateMismatch
                if self.actual.reconstruction == ReconstructionMode::Incremental
                    && self.reconstruction_fallbacks == 0 =>
            {
                self.reconstruction_fallbacks = 1;
                self.actual = ReshootPlan {
                    reconstruction: ReconstructionMode::Full,
                    training: TrainingMode::Cold,
                };
            }
            AlgorithmFailure::InvalidWarmResult
                if self.actual.training == TrainingMode::Warm && self.training_fallbacks == 0 =>
            {
                self.training_fallbacks = 1;
                self.actual.training = TrainingMode::Cold;
            }
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn warm_recipe_preserves_initialization_and_uses_view_coverage_not_sixfold_steps() {
        for (quality, base) in [
            (Quality::Fast, 3000),
            (Quality::Balanced, 5000),
            (Quality::High, 8000),
        ] {
            let full = crate::presets::resolve_brush_training_preset(
                quality,
                true,
                Some(8192),
                2048,
                1000,
            )
            .preset;
            let original_density = full.densification;
            let (warm, options) = warm_training(full, quality, 384, 3_500_000, 2).unwrap();
            assert_eq!(warm.total_steps, full.total_steps.min(base.max(3840)));
            assert!(warm.max_splats.unwrap() >= 3_500_000);
            assert_eq!(options.sh_degree, 2);
            assert_eq!(options.growth_start % 200, 0);
            if let (Some(density), Some(original)) = (warm.densification, original_density) {
                assert_eq!(density.growth_stop_iter % 200, 0);
                assert!(density.growth_stop_iter as usize <= warm.total_steps);
                assert_eq!(
                    density.growth_grad_threshold,
                    original.growth_grad_threshold * 1.5
                );
                assert_eq!(
                    density.growth_select_fraction,
                    original.growth_select_fraction * 0.5
                );
            }
            let (capped, _) = warm_training(full, quality, u64::MAX, 1, 3).unwrap();
            assert_eq!(capped.total_steps, full.total_steps);
        }
    }

    #[test]
    fn numeric_failure_requires_explicit_evidence_and_never_reclassifies_resource_or_crash() {
        use crate::{
            error::SplatError,
            pipeline::PipelineEngine,
            process::{ProcessOutput, ProcessTermination},
        };
        for (text, exit, expected) in [
            ("non-finite training loss", 1, true),
            ("training loss became NaN", 1, true),
            ("numerical divergence", 101, false),
            ("data read error", 1, false),
            ("GPU out of memory\nnon-finite training loss", 1, false),
            ("DeviceLost\nnon-finite training loss", 1, false),
            ("BufferTooBig\nnon-finite training loss", 1, false),
            ("permission denied\nnon-finite training loss", 1, false),
            (
                "non-finite training loss\n磁盘空间不足。 (os error 112)",
                1,
                false,
            ),
            (
                "non-finite training loss\nExport at iteration 8000 failed: native write failure",
                1,
                false,
            ),
            ("non-finite training loss", -1073740940, false),
        ] {
            let output = ProcessOutput {
                success: false,
                exit_code: Some(exit),
                termination: ProcessTermination {
                    exit_code: Some(exit),
                    windows_status: Some(format!("0x{:08X}", exit as u32)),
                    ..Default::default()
                },
                stdout: text.into(),
                stderr: String::new(),
            };
            let error = SplatError::EngineProcess(Box::new(
                crate::failure::EngineProcessFailure::from_output(
                    PipelineEngine::Brush,
                    "train",
                    &output,
                ),
            ));
            assert_eq!(
                explicit_numeric_divergence(&error),
                expected,
                "{text} / {exit}"
            );
        }
        assert!(!explicit_numeric_divergence(&SplatError::Cancelled));
        assert!(!explicit_numeric_divergence(&SplatError::Io(
            std::io::Error::from(std::io::ErrorKind::OutOfMemory)
        )));
    }
    #[test]
    fn answers_choose_independent_reconstruction_and_training() {
        for (condition, reconstruction, training) in [
            (
                ModelCondition::Good,
                ReconstructionMode::Incremental,
                TrainingMode::Warm,
            ),
            (
                ModelCondition::Appearance,
                ReconstructionMode::Incremental,
                TrainingMode::Cold,
            ),
            (
                ModelCondition::Geometry,
                ReconstructionMode::Full,
                TrainingMode::Cold,
            ),
            (
                ModelCondition::Unsure,
                ReconstructionMode::Incremental,
                TrainingMode::Cold,
            ),
        ] {
            assert_eq!(
                ReshootPlan::resolve(condition, true, true),
                ReshootPlan {
                    reconstruction,
                    training
                }
            );
        }
        assert_eq!(
            ReshootPlan::resolve(ModelCondition::Good, true, false).training,
            TrainingMode::Cold
        );
        assert_eq!(
            ReshootPlan::resolve(ModelCondition::Good, false, true).training,
            TrainingMode::Cold
        );
    }
    #[test]
    fn malformed_or_missing_answer_cannot_authorize_a_new_recipe() {
        assert!(serde_json::from_str::<ReshootOptions>("{}").is_err());
        assert!(serde_json::from_str::<ReshootOptions>(
            r#"{"condition":"auto","allowFallback":true}"#
        )
        .is_err());
    }
    #[test]
    fn recipe_survives_resume_and_bounds_authorized_fallbacks() {
        let mut recipe = ReshootRecipe {
            version: RECIPE_VERSION,
            normal_parameters_version: None,
            camera_assignment: None,
            options: ReshootOptions {
                condition: ModelCondition::Good,
                camera_relation: None,
                allow_fallback: false,
                source_identity: "source".into(),
                input_identity: "input".into(),
                projection: crate::video::panorama::VideoProjection::Perspective,
                image_matching_strategy: ImageMatchingStrategy::Exhaustive,
            },
            initial: ReshootPlan::resolve(ModelCondition::Good, true, true),
            actual: ReshootPlan::resolve(ModelCondition::Good, true, true),
            batch_id: "batch".into(),
            reconstruction_fallbacks: 0,
            training_fallbacks: 0,
            expanded_matching: false,
            source_names: vec![],
            new_names: vec![],
            warm_parameters: None,
            init_sha256: None,
            init_georeference: None,
            stored_input_identity: None,
            panorama_frames: vec![],
        };
        let mut legacy = serde_json::to_value(&recipe).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("normalParametersVersion");
        legacy.as_object_mut().unwrap().remove("cameraAssignment");
        legacy["options"]
            .as_object_mut()
            .unwrap()
            .remove("cameraRelation");
        let restored: ReshootRecipe = serde_json::from_value(legacy).unwrap();
        assert!(!restored.uses_normal_parameters());
        assert!(restored.camera_assignment.is_none());
        assert!(restored.options.camera_relation.is_none());
        let mut normal = recipe.clone();
        normal.normal_parameters_version = Some(1);
        normal.options.camera_relation = Some(CameraRelation::Same);
        normal.camera_assignment = Some(CameraAssignment {
            version: 1,
            reused_camera_id: Some(7),
        });
        let normal: ReshootRecipe =
            serde_json::from_value(serde_json::to_value(normal).unwrap()).unwrap();
        assert!(normal.uses_normal_parameters());
        assert_eq!(normal.camera_assignment.unwrap().reused_camera_id, Some(7));
        assert!(!recipe.fallback(AlgorithmFailure::NoNewRegistrations));
        recipe.options.allow_fallback = true;
        assert!(recipe.fallback(AlgorithmFailure::InvalidWarmResult));
        assert!(!recipe.fallback(AlgorithmFailure::InvalidWarmResult));
        assert!(recipe.fallback(AlgorithmFailure::NoNewRegistrations));
        let mut restored: ReshootRecipe =
            serde_json::from_value(serde_json::to_value(recipe).unwrap()).unwrap();
        assert!(!restored.fallback(AlgorithmFailure::NoNewRegistrations));
        assert_eq!(restored.actual.reconstruction, ReconstructionMode::Full);
    }
}
