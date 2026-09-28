use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::{
    engines::ColmapBackend,
    pipeline::PipelineStage,
    planner::BridgeBackfillStatus,
    presets::{BrushTrainingProfile, ResolvedBrushTrainingPreset},
};

pub const PLANNER_EFFECTIVENESS_SCHEMA_VERSION: u32 = 1;
pub const PLANNER_VERSION_V1: &str = "quality_v2_planner_v1";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlannerGpuVendor {
    Nvidia,
    Amd,
    Intel,
    Apple,
    Other,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannerFramePlanMetrics {
    pub source_width: u64,
    pub source_height: u64,
    pub source_item_count: u64,
    pub configured_target_fps: Option<f64>,
    pub configured_candidate_fps: Option<f64>,
    pub effective_target_fps: Option<f64>,
    pub effective_candidate_fps: Option<f64>,
    pub initial_selected_count: u64,
    pub candidate_count: u64,
    pub minimum_frame_override_applied: bool,
    pub minimum_frame_target_unreachable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannerInitialReconstructionMetrics {
    pub input_images: u64,
    pub registered_images: u64,
    pub points_3d: u64,
    pub backend: ColmapBackend,
    pub allow_two_view_tracks: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlannerBridgeStatus {
    #[default]
    NotApplicable,
    NotEvaluated,
    NotNeeded,
    NoBudget,
    Running,
    Completed,
    FailedRolledBack,
}

impl From<BridgeBackfillStatus> for PlannerBridgeStatus {
    fn from(value: BridgeBackfillStatus) -> Self {
        match value {
            BridgeBackfillStatus::NotEvaluated => Self::NotEvaluated,
            BridgeBackfillStatus::NotNeeded => Self::NotNeeded,
            BridgeBackfillStatus::NoBudget => Self::NoBudget,
            BridgeBackfillStatus::Running => Self::Running,
            BridgeBackfillStatus::Completed => Self::Completed,
            BridgeBackfillStatus::FailedRolledBack => Self::FailedRolledBack,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannerBridgeMetrics {
    pub status: PlannerBridgeStatus,
    pub trigger_ratio: f64,
    pub available_budget: u64,
    pub requested_frames: u64,
    pub added_frames: u64,
    pub internal_bridge_count: u64,
    pub edge_extension_count: u64,
    pub duration_ms: Option<u64>,
    pub initial_input_images: u64,
    pub initial_registered_images: u64,
    pub initial_points_3d: u64,
    pub final_input_images: u64,
    pub final_registered_images: u64,
    pub final_points_3d: u64,
    pub adopted: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannerBrushMetrics {
    pub initial_profile: Option<BrushTrainingProfile>,
    pub final_profile: Option<BrushTrainingProfile>,
    pub detected_total_memory_mb: Option<u64>,
    pub max_resolution: Option<u32>,
    pub total_steps: Option<u64>,
    pub growth_grad_threshold: Option<f32>,
    pub growth_select_fraction: Option<f32>,
    pub growth_stop_iter: Option<u32>,
    pub refine_every: Option<u32>,
    pub max_splats: Option<u32>,
    pub oom_retry_used: bool,
}

impl PlannerBrushMetrics {
    fn update(&mut self, resolved: ResolvedBrushTrainingPreset, oom_retry_used: bool) {
        self.initial_profile.get_or_insert(resolved.profile);
        self.final_profile = Some(resolved.profile);
        self.detected_total_memory_mb = resolved.detected_total_memory_mb;
        self.max_resolution = Some(resolved.preset.max_resolution);
        self.total_steps = Some(resolved.preset.total_steps as u64);
        self.refine_every = Some(resolved.preset.refine_every);
        self.max_splats = resolved.preset.max_splats;
        if let Some(densification) = resolved.preset.densification {
            self.growth_grad_threshold = Some(densification.growth_grad_threshold);
            self.growth_select_fraction = Some(densification.growth_select_fraction);
            self.growth_stop_iter = Some(densification.growth_stop_iter);
        } else {
            self.growth_grad_threshold = None;
            self.growth_select_fraction = None;
            self.growth_stop_iter = None;
        }
        self.oom_retry_used |= oom_retry_used;
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannerFinalResultMetrics {
    pub input_images: u64,
    pub registered_images: u64,
    pub points_3d: u64,
    pub splat_count: u64,
    pub ply_size_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannerStageDurationMetrics {
    pub probing_video_ms: Option<u64>,
    pub extracting_frames_ms: Option<u64>,
    pub extracting_features_ms: Option<u64>,
    pub matching_ms: Option<u64>,
    pub reconstructing_ms: Option<u64>,
    pub validating_reconstruction_ms: Option<u64>,
    pub training_splats_ms: Option<u64>,
    pub exporting_ms: Option<u64>,
}

impl PlannerStageDurationMetrics {
    fn add(slot: &mut Option<u64>, duration_ms: u64) {
        *slot = Some(slot.unwrap_or(0).saturating_add(duration_ms));
    }

    pub fn record(&mut self, stage: PipelineStage, duration_ms: u64) {
        match stage {
            PipelineStage::ProbingVideo => Self::add(&mut self.probing_video_ms, duration_ms),
            PipelineStage::ExtractingFrames => {
                Self::add(&mut self.extracting_frames_ms, duration_ms)
            }
            PipelineStage::ExtractingFeatures => {
                Self::add(&mut self.extracting_features_ms, duration_ms)
            }
            PipelineStage::Matching => Self::add(&mut self.matching_ms, duration_ms),
            PipelineStage::Reconstructing => Self::add(&mut self.reconstructing_ms, duration_ms),
            PipelineStage::ValidatingReconstruction => {
                Self::add(&mut self.validating_reconstruction_ms, duration_ms)
            }
            PipelineStage::TrainingSplats => Self::add(&mut self.training_splats_ms, duration_ms),
            PipelineStage::Exporting => Self::add(&mut self.exporting_ms, duration_ms),
            PipelineStage::Created
            | PipelineStage::PlanningFrames
            | PipelineStage::Completed
            | PipelineStage::Failed
            | PipelineStage::Cancelled => {}
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannerEffectivenessSnapshot {
    pub gpu_vendor: PlannerGpuVendor,
    pub frame_plan: Option<PlannerFramePlanMetrics>,
    pub initial_reconstruction: Option<PlannerInitialReconstructionMetrics>,
    pub bridge: Option<PlannerBridgeMetrics>,
    pub brush: Option<PlannerBrushMetrics>,
    pub final_result: Option<PlannerFinalResultMetrics>,
    pub stage_durations: PlannerStageDurationMetrics,
}

#[derive(Debug, Clone, Default)]
pub struct PlannerEffectivenessTracker {
    inner: Arc<Mutex<PlannerEffectivenessSnapshot>>,
}

impl PlannerEffectivenessTracker {
    fn update(&self, apply: impl FnOnce(&mut PlannerEffectivenessSnapshot)) {
        let mut snapshot = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        apply(&mut snapshot);
    }

    pub fn snapshot(&self) -> PlannerEffectivenessSnapshot {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn record_frame_plan(&self, metrics: PlannerFramePlanMetrics) {
        self.update(|snapshot| snapshot.frame_plan = Some(metrics));
    }

    pub fn record_initial_reconstruction(&self, metrics: PlannerInitialReconstructionMetrics) {
        self.update(|snapshot| snapshot.initial_reconstruction = Some(metrics));
    }

    pub fn record_gpu_vendor(&self, vendor: PlannerGpuVendor) {
        self.update(|snapshot| snapshot.gpu_vendor = vendor);
    }

    pub fn record_bridge(&self, metrics: PlannerBridgeMetrics) {
        self.update(|snapshot| snapshot.bridge = Some(metrics));
    }

    pub fn record_brush(&self, resolved: ResolvedBrushTrainingPreset, oom_retry_used: bool) {
        self.update(|snapshot| {
            snapshot
                .brush
                .get_or_insert_with(PlannerBrushMetrics::default)
                .update(resolved, oom_retry_used);
        });
    }

    pub fn record_final_result(&self, metrics: PlannerFinalResultMetrics) {
        self.update(|snapshot| snapshot.final_result = Some(metrics));
    }

    pub fn record_stage_duration(&self, stage: PipelineStage, duration_ms: u64) {
        self.update(|snapshot| snapshot.stage_durations.record(stage, duration_ms));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets::{BrushTrainingPreset, ResolvedBrushTrainingPreset};

    #[test]
    fn tracker_accumulates_stage_durations_and_keeps_initial_brush_profile() {
        let tracker = PlannerEffectivenessTracker::default();
        tracker.record_stage_duration(PipelineStage::Matching, 10);
        tracker.record_stage_duration(PipelineStage::Matching, 25);
        let resolved = |profile| ResolvedBrushTrainingPreset {
            profile,
            detected_total_memory_mb: Some(8_192),
            configured_max_splats: Some(1_500_000),
            preset: BrushTrainingPreset {
                total_steps: 30_000,
                max_resolution: 3_840,
                refine_every: 200,
                max_splats: Some(1_500_000),
                densification: None,
            },
        };
        tracker.record_brush(resolved(BrushTrainingProfile::HighStandard), false);
        tracker.record_brush(resolved(BrushTrainingProfile::HighLow), true);

        let snapshot = tracker.snapshot();
        assert_eq!(snapshot.stage_durations.matching_ms, Some(35));
        let brush = snapshot.brush.unwrap();
        assert_eq!(
            brush.initial_profile,
            Some(BrushTrainingProfile::HighStandard)
        );
        assert_eq!(brush.final_profile, Some(BrushTrainingProfile::HighLow));
        assert!(brush.oom_retry_used);
    }
}
