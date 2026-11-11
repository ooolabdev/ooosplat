//! Transport-independent execution decisions shared by normal generation,
//! direct COLMAP training and reshoots.
//!
//! This module deliberately contains no filesystem operations.  Source
//! projects are first frozen into a reshoot snapshot and only snapshot-relative
//! paths may be placed in a plan.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::reshoot::{ReconstructionMode, ReshootRecipe, TrainingMode};

pub const EXECUTION_PLAN_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotModel {
    /// Relative to the derived project's work directory.
    pub relative_path: PathBuf,
    pub sha256: String,
    pub coordinate_frame_id: String,
    #[serde(default)]
    pub has_erp: bool,
    #[serde(default)]
    pub fixed_images: Vec<u32>,
    #[serde(default)]
    pub fixed_cameras: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ReconstructionPlan {
    Full,
    Incremental { seed_model: SnapshotModel },
    Imported { model: SnapshotModel },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TrainingSeed {
    SparsePoints,
    GaussianSnapshot {
        relative_path: PathBuf,
        sha256: String,
        coordinate_frame_id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MatchPlan {
    Normal,
    ReshootCrossSource,
    Imported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgePolicy {
    pub enabled: bool,
    /// A reshoot may backfill newly supplied videos, never the frozen source.
    pub new_video_sources_only: bool,
    pub max_joint_rounds: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuccessPolicy {
    /// Ordinary generation accepts the best connected component. A reshoot
    /// requires at least one newly supplied source to contribute a camera.
    pub require_any_new_source: bool,
    pub allow_partial_new_sources: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FallbackPolicy {
    pub allow_algorithm_fallback: bool,
    pub reconstruction_fallbacks_used: u8,
    pub training_fallbacks_used: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionPlan {
    pub version: u32,
    /// Projection capability of the authoritative seed, retained even when a
    /// fallback switches reconstruction from Incremental to Full.
    #[serde(default)]
    pub source_has_erp: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_model: Option<SnapshotModel>,
    pub reconstruction: ReconstructionPlan,
    pub training_seed: TrainingSeed,
    pub matching: MatchPlan,
    pub bridge: BridgePolicy,
    pub success: SuccessPolicy,
    pub fallback: FallbackPolicy,
}

impl ExecutionPlan {
    pub fn normal() -> Self {
        Self {
            version: EXECUTION_PLAN_VERSION,
            source_has_erp: false,
            source_model: None,
            reconstruction: ReconstructionPlan::Full,
            training_seed: TrainingSeed::SparsePoints,
            matching: MatchPlan::Normal,
            bridge: BridgePolicy {
                enabled: true,
                new_video_sources_only: false,
                max_joint_rounds: 1,
            },
            success: SuccessPolicy {
                require_any_new_source: false,
                allow_partial_new_sources: true,
            },
            fallback: FallbackPolicy {
                allow_algorithm_fallback: false,
                reconstruction_fallbacks_used: 0,
                training_fallbacks_used: 0,
            },
        }
    }

    pub fn imported(model: SnapshotModel) -> Self {
        let source_has_erp = model.has_erp;
        Self {
            source_has_erp,
            source_model: Some(model.clone()),
            reconstruction: ReconstructionPlan::Imported { model },
            matching: MatchPlan::Imported,
            bridge: BridgePolicy {
                enabled: false,
                new_video_sources_only: false,
                max_joint_rounds: 0,
            },
            ..Self::normal()
        }
    }

    pub fn reshoot(recipe: &ReshootRecipe, seed_model: SnapshotModel) -> Self {
        let source_has_erp = seed_model.has_erp;
        let reconstruction = match recipe.actual.reconstruction {
            ReconstructionMode::Full => ReconstructionPlan::Full,
            ReconstructionMode::Incremental => ReconstructionPlan::Incremental {
                seed_model: seed_model.clone(),
            },
        };
        let training_seed = match recipe.actual.training {
            TrainingMode::Cold => TrainingSeed::SparsePoints,
            TrainingMode::Warm => TrainingSeed::GaussianSnapshot {
                relative_path: PathBuf::from("reshoot-snapshot-v3/training/init-source.ply"),
                sha256: recipe.init_sha256.clone().unwrap_or_default(),
                coordinate_frame_id: recipe
                    .init_georeference
                    .clone()
                    .unwrap_or_else(|| "source-colmap".into()),
            },
        };
        Self {
            version: EXECUTION_PLAN_VERSION,
            source_has_erp,
            source_model: Some(seed_model.clone()),
            reconstruction,
            training_seed,
            matching: MatchPlan::ReshootCrossSource,
            bridge: BridgePolicy {
                enabled: true,
                new_video_sources_only: true,
                max_joint_rounds: 1,
            },
            success: SuccessPolicy {
                require_any_new_source: true,
                allow_partial_new_sources: true,
            },
            fallback: FallbackPolicy {
                allow_algorithm_fallback: recipe.options.allow_fallback,
                reconstruction_fallbacks_used: recipe.reconstruction_fallbacks,
                training_fallbacks_used: recipe.training_fallbacks,
            },
        }
    }

    pub fn incremental_seed(&self) -> Option<&SnapshotModel> {
        match &self.reconstruction {
            ReconstructionPlan::Incremental { seed_model } => Some(seed_model),
            _ => None,
        }
    }

    pub fn gaussian_seed(&self) -> Option<(&PathBuf, &str, &str)> {
        match &self.training_seed {
            TrainingSeed::GaussianSnapshot {
                relative_path,
                sha256,
                coordinate_frame_id,
            } => Some((relative_path, sha256, coordinate_frame_id)),
            TrainingSeed::SparsePoints => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        pipeline::reshoot::{
            CameraRelation, ModelCondition, ReshootOptions, ReshootPlan, RECIPE_VERSION,
        },
        project::ImageMatchingStrategy,
        video::panorama::VideoProjection,
    };

    fn recipe(condition: ModelCondition, allow_fallback: bool) -> ReshootRecipe {
        let plan = ReshootPlan::resolve(condition, true, true);
        ReshootRecipe {
            version: RECIPE_VERSION,
            normal_parameters_version: Some(1),
            camera_assignment: None,
            options: ReshootOptions {
                condition,
                camera_relation: Some(CameraRelation::Same),
                allow_fallback,
                source_identity: "source".into(),
                input_identity: "input".into(),
                projection: VideoProjection::Perspective,
                image_matching_strategy: ImageMatchingStrategy::Exhaustive,
            },
            initial: plan,
            actual: plan,
            batch_id: "batch".into(),
            reconstruction_fallbacks: 0,
            training_fallbacks: 0,
            expanded_matching: false,
            source_names: vec![],
            new_names: vec![],
            warm_parameters: None,
            init_sha256: Some("ply".into()),
            init_georeference: Some("frame".into()),
            stored_input_identity: None,
            panorama_frames: vec![],
        }
    }

    fn model() -> SnapshotModel {
        SnapshotModel {
            relative_path: "reshoot-snapshot-v3/sparse".into(),
            sha256: "model".into(),
            coordinate_frame_id: "frame".into(),
            has_erp: false,
            fixed_images: vec![1],
            fixed_cameras: vec![2],
        }
    }

    #[test]
    fn user_answers_only_change_reconstruction_and_training_seed() {
        let good = ExecutionPlan::reshoot(&recipe(ModelCondition::Good, true), model());
        assert!(matches!(
            good.reconstruction,
            ReconstructionPlan::Incremental { .. }
        ));
        assert!(matches!(
            good.training_seed,
            TrainingSeed::GaussianSnapshot { .. }
        ));
        assert!(good.bridge.new_video_sources_only);
        assert!(good.success.require_any_new_source);

        let appearance =
            ExecutionPlan::reshoot(&recipe(ModelCondition::Appearance, false), model());
        assert!(matches!(
            appearance.reconstruction,
            ReconstructionPlan::Incremental { .. }
        ));
        assert_eq!(appearance.training_seed, TrainingSeed::SparsePoints);

        let geometry = ExecutionPlan::reshoot(&recipe(ModelCondition::Geometry, false), model());
        assert_eq!(geometry.reconstruction, ReconstructionPlan::Full);
        assert_eq!(geometry.training_seed, TrainingSeed::SparsePoints);
    }

    #[test]
    fn imported_plan_cannot_run_matching_or_bridge() {
        let plan = ExecutionPlan::imported(model());
        assert!(matches!(
            plan.reconstruction,
            ReconstructionPlan::Imported { .. }
        ));
        assert_eq!(plan.matching, MatchPlan::Imported);
        assert!(!plan.bridge.enabled);
    }
}
