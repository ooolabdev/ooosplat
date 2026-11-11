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

impl PlannerFramePlanMetrics {
    pub fn images(width: u32, height: u32, count: u64) -> Self {
        Self {
            source_width: width.into(),
            source_height: height.into(),
            source_item_count: count,
            initial_selected_count: count,
            candidate_count: count,
            ..Default::default()
        }
    }

    pub fn video(
        video: &crate::video::VideoInfo,
        plan: &crate::video::FramePlan,
        quality: crate::presets::Quality,
        policy: Option<crate::pipeline::generation_policy::GenerationPolicy>,
        removal_fps: Option<f64>,
    ) -> Option<Self> {
        if !video.duration.is_finite() || video.duration <= 0.0 {
            return None;
        }
        let count = plan.estimated_frames;
        let candidates = plan.candidate_count();
        let preset = quality.preset();
        let manual = removal_fps.is_some()
            || policy.is_some_and(|policy| {
                policy.frame_sampling_ratio.is_some() || policy.frame_sampling_fps.is_some()
            });
        let configured = if let Some(policy) = policy {
            if let Some(fps) = policy.frame_sampling_fps {
                Some(fps.min(video.fps))
            } else if let Some(ratio) = policy.frame_sampling_ratio {
                Some(video.fps * ratio)
            } else if policy.version >= 5 {
                if let Some(removal) = removal_fps {
                    Some((video.fps - removal).max(0.1))
                } else if policy.version == 5 {
                    preset
                        .initial_fps
                        .map(|removal| (video.fps - removal).max(0.1))
                } else {
                    preset.initial_fps
                }
            } else {
                preset.initial_fps
            }
        } else {
            preset.initial_fps
        };
        // v6 may raise its target before creating FramePlan, so that plan's
        // flag alone no longer proves whether the 30-frame floor added frames.
        // Compare the original requested rate (bounded by the source rate)
        // with the actual selected count, without changing the execution plan.
        let minimum = crate::video::minimum_selected_frames().min(video.total_frames);
        let minimum_override = configured.is_some_and(|fps| {
            let requested = fps.min(video.fps) * video.duration;
            requested.is_finite()
                && minimum > 0
                && count >= minimum
                && requested + 1e-9 < minimum as f64
                && count as f64 > requested + 1e-9
        });
        Some(Self {
            source_width: video.width.into(),
            source_height: video.height.into(),
            source_item_count: video.total_frames,
            configured_target_fps: configured.filter(|fps| fps.is_finite() && *fps > 0.0),
            configured_candidate_fps: (!manual).then_some(preset.rescue_max_fps).flatten(),
            effective_target_fps: Some(count as f64 / video.duration),
            effective_candidate_fps: Some(candidates as f64 / video.duration),
            initial_selected_count: count,
            candidate_count: candidates,
            minimum_frame_override_applied: plan.minimum_frame_override_applied || minimum_override,
            minimum_frame_target_unreachable: video.total_frames
                < crate::video::minimum_selected_frames(),
        })
    }
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

/// Optional local observation, not an execution recipe or a cache identity.
/// The source ID binds the scalar frame plan to the one newly requested input.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionTelemetry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_vendor: Option<PlannerGpuVendor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_plan: Option<PlannerFramePlanMetrics>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_reconstruction: Option<PlannerInitialReconstructionMetrics>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge: Option<PlannerBridgeMetrics>,
}

#[derive(Debug, Clone, Default)]
pub struct PlannerEffectivenessTracker {
    inner: Arc<Mutex<PlannerEffectivenessSnapshot>>,
    pending_brush: Arc<Mutex<Option<(ResolvedBrushTrainingPreset, bool)>>>,
}

impl PlannerEffectivenessTracker {
    /// Keep the plan even if GPU initialization fails; don't report an unstarted
    /// fallback as the final executed configuration.
    pub fn prepare_brush(
        &self,
        initial: Option<ResolvedBrushTrainingPreset>,
        resolved: Option<ResolvedBrushTrainingPreset>,
        oom_retry_used: bool,
    ) {
        *self.pending_brush.lock().unwrap_or_else(|p| p.into_inner()) =
            resolved.map(|v| (v, oom_retry_used));
        self.update(|snapshot| {
            let brush = snapshot.brush.get_or_insert_with(Default::default);
            if brush.initial_profile.is_none() {
                brush.initial_profile = initial.map(|v| v.profile);
            }
            if brush.final_profile.is_none() {
                if let Some(resolved) = resolved {
                    let first = brush.initial_profile;
                    brush.update(resolved, oom_retry_used);
                    brush.initial_profile = first;
                    brush.final_profile = None;
                }
            }
        });
    }

    pub fn brush_started(&self) {
        let pending = *self.pending_brush.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((resolved, oom)) = pending {
            self.update(|snapshot| {
                let brush = snapshot.brush.get_or_insert_with(Default::default);
                let first = brush.initial_profile;
                brush.update(resolved, oom);
                brush.initial_profile = first;
            });
        }
    }

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

    pub fn restore_brush(
        &self,
        initial: Option<ResolvedBrushTrainingPreset>,
        resolved: Option<ResolvedBrushTrainingPreset>,
        oom_retry_used: bool,
    ) {
        if initial.is_none() && resolved.is_none() {
            return;
        }
        self.update(|snapshot| {
            let mut brush = PlannerBrushMetrics::default();
            if let Some(resolved) = resolved.or(initial) {
                brush.update(resolved, oom_retry_used);
            }
            // Old OOM checkpoints can contain only the final profile. Keep the
            // unknown initial profile null instead of reporting the fallback as initial.
            brush.initial_profile = initial.map(|value| value.profile);
            snapshot.brush = Some(brush);
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
    use crate::presets::{BrushTrainingPreset, Quality, ResolvedBrushTrainingPreset};

    #[test]
    fn video_observation_keeps_requested_rates_separate_from_minimum_and_manual_plans() {
        let mut video = crate::video::VideoInfo {
            width: 1920,
            height: 1080,
            fps: 30.0,
            duration: 2.0,
            total_frames: 60,
            codec: "fixture".into(),
            rotation: 0,
            pixel_format: "rgb24".into(),
            has_alpha: false,
        };
        let policy = crate::pipeline::generation_policy::GenerationPolicy::default();
        let plan = policy
            .frame_plan_for_material(&video, Quality::Balanced, None)
            .unwrap();
        let observed =
            PlannerFramePlanMetrics::video(&video, &plan, Quality::Balanced, Some(policy), None)
                .unwrap();
        assert_eq!(observed.configured_target_fps, Some(8.0));
        assert_eq!(observed.configured_candidate_fps, Some(12.0));
        assert_eq!(observed.initial_selected_count, 30);
        assert_eq!(observed.effective_target_fps, Some(15.0));
        assert!(observed.minimum_frame_override_applied);
        let low_source = crate::video::VideoInfo {
            fps: 2.0,
            duration: 10.0,
            total_frames: 20,
            ..video.clone()
        };
        let all = policy
            .frame_plan_for_material(&low_source, Quality::Balanced, None)
            .unwrap();
        let low = PlannerFramePlanMetrics::video(
            &low_source,
            &all,
            Quality::Balanced,
            Some(policy),
            None,
        )
        .unwrap();
        assert_eq!(low.initial_selected_count, 20);
        assert!(!low.minimum_frame_override_applied);
        assert!(low.minimum_frame_target_unreachable);
        video.duration = 60.0;
        video.total_frames = 1800;
        let plan = policy
            .frame_plan_for_material(&video, Quality::Balanced, Some(10.0))
            .unwrap();
        let manual = PlannerFramePlanMetrics::video(
            &video,
            &plan,
            Quality::Balanced,
            Some(policy),
            Some(10.0),
        )
        .unwrap();
        assert_eq!(manual.configured_target_fps, Some(20.0));
        assert_eq!(manual.configured_candidate_fps, None);
        assert_eq!(manual.initial_selected_count, plan.estimated_frames);
        assert_eq!(
            manual.effective_target_fps,
            Some(plan.estimated_frames as f64 / 60.0)
        );
        let old = crate::pipeline::generation_policy::GenerationPolicy {
            version: 5,
            ..policy
        };
        let plan = old
            .frame_plan_for_material(&video, Quality::Balanced, None)
            .unwrap();
        assert_eq!(
            PlannerFramePlanMetrics::video(&video, &plan, Quality::Balanced, Some(old), None)
                .unwrap()
                .configured_target_fps,
            Some(22.0)
        );
    }

    #[test]
    fn a_failed_first_execution_and_its_retry_keep_both_profiles_and_actual_caps() {
        let tracker = PlannerEffectivenessTracker::default();
        let first = crate::presets::resolve_brush_training_preset(
            Quality::High,
            true,
            Some(8192),
            3840,
            1000,
        );
        tracker.prepare_brush(Some(first), Some(first), false);
        assert_eq!(tracker.snapshot().brush.unwrap().final_profile, None);
        tracker.brush_started();
        let mut retry = first;
        retry.profile = BrushTrainingProfile::HighLow;
        retry.preset.max_splats = Some(800_000);
        tracker.prepare_brush(Some(first), Some(retry), true);
        // A fallback that could not start must not replace the last executed config.
        assert_eq!(
            tracker.snapshot().brush.unwrap().final_profile,
            Some(first.profile)
        );
        tracker.brush_started();
        let result = tracker.snapshot().brush.unwrap();
        assert_eq!(result.initial_profile, Some(first.profile));
        assert_eq!(result.final_profile, Some(retry.profile));
        assert_eq!(result.max_splats, Some(800_000));
        assert!(result.oom_retry_used);
        // New policy changes only cap, not profile.
        retry.preset.max_splats = Some(600_000);
        tracker.prepare_brush(Some(first), Some(retry), true);
        tracker.brush_started();
        assert_eq!(tracker.snapshot().brush.unwrap().max_splats, Some(600_000));
    }

    #[test]
    fn tracker_accumulates_stage_durations_and_keeps_initial_brush_profile() {
        let tracker = PlannerEffectivenessTracker::default();
        tracker.record_stage_duration(PipelineStage::Matching, 10);
        tracker.record_stage_duration(PipelineStage::Matching, 25);
        let resolved = |profile| ResolvedBrushTrainingPreset {
            profile,
            detected_total_memory_mb: Some(8_192),
            configured_max_splats: Some(1_500_000),
            panorama: None,
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

    #[test]
    fn old_oom_checkpoint_does_not_invent_an_initial_profile() {
        let tracker = PlannerEffectivenessTracker::default();
        let resolved = crate::presets::resolve_brush_training_preset(
            crate::presets::Quality::High,
            true,
            Some(8192),
            3840,
            1000,
        );
        tracker.restore_brush(None, Some(resolved), true);
        let brush = tracker.snapshot().brush.unwrap();
        assert_eq!(brush.initial_profile, None);
        assert_eq!(brush.final_profile, Some(resolved.profile));
        assert!(brush.oom_retry_used);
        // Absence of an OOM flag does not prove an older reshoot never fell back.
        tracker.restore_brush(None, Some(resolved), false);
        assert_eq!(tracker.snapshot().brush.unwrap().initial_profile, None);
    }
}
