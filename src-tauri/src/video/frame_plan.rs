use serde::{Deserialize, Serialize};

use crate::{presets::Quality, video::VideoInfo};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FramePlanningMode {
    #[default]
    Legacy,
    Budgeted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedFrame {
    pub source_frame_index: u64,
    pub timestamp_seconds: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MinimumFrameProtection {
    pub short_capture: bool,
    pub minimum_required_fps: f64,
    pub minimum_frame_target: u64,
    pub minimum_frame_override_applied: bool,
    pub minimum_frame_target_unreachable: bool,
    pub selected_frames_before_filter: u64,
    pub selected_frames_after_filter: u64,
    pub backfilled_for_minimum_count: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FramePlan {
    #[serde(default)]
    pub quality: Option<Quality>,
    /// Exact historical ratio. It remains authoritative for legacy projects.
    pub retention_ratio: f64,
    /// Actual legacy FFmpeg fps, or the measured average density of a budgeted plan.
    pub sampling_fps: f64,
    #[serde(default)]
    pub actual_average_fps: f64,
    #[serde(default)]
    pub target_fps: f64,
    #[serde(default)]
    pub candidate_fps: f64,
    pub estimated_frames: u64,
    #[serde(default)]
    pub planning_mode: FramePlanningMode,
    #[serde(default)]
    pub preferred_fps: f64,
    #[serde(default)]
    pub selected_frames: Vec<PlannedFrame>,
    #[serde(default)]
    pub candidate_frames: Vec<PlannedFrame>,
    /// Flattened to keep the public FramePlan JSON shape stable and make the
    /// minimum-frame decisions directly available to logs and telemetry.
    #[serde(flatten)]
    pub minimum_frame_protection: MinimumFrameProtection,
}

pub trait FrameSelectionStrategy {
    fn create_plan(&self, video: &VideoInfo, quality: Quality) -> FramePlan;
}

/// Compatibility strategy used while Planner is disabled.
#[derive(Debug, Default, Clone, Copy)]
pub struct UniformRatioFrameSelection;

impl FrameSelectionStrategy for UniformRatioFrameSelection {
    fn create_plan(&self, video: &VideoInfo, quality: Quality) -> FramePlan {
        let retention_ratio = quality.legacy_frame_retention_ratio();
        let sampling_fps = (video.fps * retention_ratio).max(0.0);
        FramePlan {
            quality: Some(quality),
            retention_ratio,
            sampling_fps,
            actual_average_fps: sampling_fps,
            target_fps: sampling_fps,
            candidate_fps: sampling_fps,
            estimated_frames: ((video.total_frames as f64) * retention_ratio).round() as u64,
            planning_mode: FramePlanningMode::Legacy,
            preferred_fps: 0.0,
            selected_frames: Vec::new(),
            candidate_frames: Vec::new(),
            minimum_frame_protection: MinimumFrameProtection::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thirty_fps_video() -> VideoInfo {
        VideoInfo {
            duration: 60.0,
            width: 1920,
            height: 1080,
            fps: 30.0,
            total_frames: 1800,
            codec: "h264".into(),
            rotation: 0,
            pixel_format: "yuv420p".into(),
            has_alpha: false,
        }
    }

    #[test]
    fn planner_off_keeps_current_main_ratios() {
        let video = thirty_fps_video();
        let strategy = UniformRatioFrameSelection;
        let fast = strategy.create_plan(&video, Quality::Fast);
        let balanced = strategy.create_plan(&video, Quality::Balanced);
        let high = strategy.create_plan(&video, Quality::High);
        assert_eq!((fast.retention_ratio, fast.sampling_fps), (0.30, 9.0));
        assert_eq!(
            (balanced.retention_ratio, balanced.sampling_fps),
            (0.50, 15.0)
        );
        assert_eq!((high.retention_ratio, high.sampling_fps), (1.0, 30.0));
        assert_eq!(fast.planning_mode, FramePlanningMode::Legacy);
        assert!(fast.selected_frames.is_empty());
    }

    #[test]
    fn legacy_plan_does_not_use_preferred_fps() {
        let plan = UniformRatioFrameSelection.create_plan(&thirty_fps_video(), Quality::Fast);
        assert_ne!(
            plan.sampling_fps,
            Quality::Fast.preset().frame.preferred_fps
        );
    }
}
