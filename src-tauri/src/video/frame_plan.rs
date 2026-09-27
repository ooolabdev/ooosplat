use serde::{Deserialize, Serialize};

use crate::{presets::QualityPreset, video::VideoInfo};

pub const MINIMUM_SELECTED_FRAMES: u64 = 30;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedFrame {
    pub source_frame_index: u64,
    pub timestamp_seconds: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FramePlan {
    pub retention_ratio: f64,
    pub sampling_fps: f64,
    pub estimated_frames: u64,
    #[serde(default, skip_serializing)]
    pub selected_frames: Vec<PlannedFrame>,
    #[serde(default, skip_serializing)]
    pub candidate_frames: Vec<PlannedFrame>,
    #[serde(default)]
    pub rescue_max_frames: u64,
    #[serde(default)]
    pub minimum_frame_override_applied: bool,
}

impl Default for FramePlan {
    fn default() -> Self {
        Self {
            retention_ratio: 0.0,
            sampling_fps: 0.0,
            estimated_frames: 0,
            selected_frames: Vec::new(),
            candidate_frames: Vec::new(),
            rescue_max_frames: 0,
            minimum_frame_override_applied: false,
        }
    }
}

pub trait FrameSelectionStrategy {
    fn create_plan(&self, video: &VideoInfo, preset: &QualityPreset) -> FramePlan;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct UniformRatioFrameSelection;

impl FrameSelectionStrategy for UniformRatioFrameSelection {
    fn create_plan(&self, video: &VideoInfo, preset: &QualityPreset) -> FramePlan {
        FramePlan {
            retention_ratio: preset.frame_retention_ratio,
            sampling_fps: video.fps * preset.frame_retention_ratio,
            estimated_frames: ((video.total_frames as f64) * preset.frame_retention_ratio).round()
                as u64,
            ..FramePlan::default()
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct QualityV2FrameSelection;

impl QualityV2FrameSelection {
    fn uniform_frames(video: &VideoInfo, count: u64) -> Vec<PlannedFrame> {
        let count = count.min(video.total_frames).max(1);
        (0..count)
            .map(|position| {
                let source_frame_index = ((position as f64 * video.total_frames as f64)
                    / count as f64)
                    .floor()
                    .min(video.total_frames.saturating_sub(1) as f64)
                    as u64;
                PlannedFrame {
                    source_frame_index,
                    timestamp_seconds: if video.fps > 0.0 {
                        source_frame_index as f64 / video.fps
                    } else {
                        0.0
                    },
                }
            })
            .fold(Vec::new(), |mut frames, frame| {
                if frames
                    .last()
                    .map(|last: &PlannedFrame| last.source_frame_index)
                    != Some(frame.source_frame_index)
                {
                    frames.push(frame);
                }
                frames
            })
    }

    fn subset(candidate_frames: &[PlannedFrame], count: u64) -> Vec<PlannedFrame> {
        if count as usize >= candidate_frames.len() {
            return candidate_frames.to_vec();
        }
        let positions = Self::uniform_positions(candidate_frames.len(), count as usize);
        positions
            .into_iter()
            .map(|position| candidate_frames[position].clone())
            .collect()
    }

    fn uniform_positions(total: usize, count: usize) -> Vec<usize> {
        if count >= total {
            return (0..total).collect();
        }
        (0..count).map(|position| position * total / count).fold(
            Vec::new(),
            |mut output, position| {
                if output.last().copied() != Some(position) {
                    output.push(position);
                }
                output
            },
        )
    }
}

impl FrameSelectionStrategy for QualityV2FrameSelection {
    fn create_plan(&self, video: &VideoInfo, preset: &QualityPreset) -> FramePlan {
        if video.total_frames == 0 || video.fps <= 0.0 || video.duration <= 0.0 {
            return FramePlan::default();
        }
        let minimum_required_fps = MINIMUM_SELECTED_FRAMES as f64 / video.duration;
        let initial_fps = preset
            .initial_fps
            .map(|fps| fps.max(minimum_required_fps).min(video.fps))
            .unwrap_or(video.fps);
        let rescue_fps = preset
            .rescue_max_fps
            .map(|fps| fps.max(minimum_required_fps).min(video.fps))
            .unwrap_or(initial_fps);
        let initial_count =
            ((video.duration * initial_fps).round() as u64).clamp(1, video.total_frames);
        let rescue_count =
            ((video.duration * rescue_fps).round() as u64).clamp(initial_count, video.total_frames);
        let candidate_frames = Self::uniform_frames(video, rescue_count);
        let selected_frames = Self::subset(&candidate_frames, initial_count);
        let selected_count = selected_frames.len() as u64;
        let actual_fps = selected_count as f64 / video.duration;
        FramePlan {
            retention_ratio: selected_count as f64 / video.total_frames as f64,
            sampling_fps: actual_fps,
            estimated_frames: selected_count,
            selected_frames,
            rescue_max_frames: candidate_frames.len() as u64,
            candidate_frames,
            minimum_frame_override_applied: preset
                .initial_fps
                .is_some_and(|preferred| minimum_required_fps > preferred),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets::Quality;

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
    fn calculates_required_sampling_rates() {
        let strategy = UniformRatioFrameSelection;
        let video = thirty_fps_video();
        assert_eq!(
            strategy
                .create_plan(&video, &Quality::Fast.preset())
                .sampling_fps,
            9.0
        );
        assert_eq!(
            strategy
                .create_plan(&video, &Quality::Balanced.preset())
                .sampling_fps,
            15.0
        );
        assert_eq!(
            strategy
                .create_plan(&video, &Quality::High.preset())
                .sampling_fps,
            30.0
        );
    }

    #[test]
    fn estimates_frames_without_a_cap() {
        let strategy = UniformRatioFrameSelection;
        let video = thirty_fps_video();
        assert_eq!(
            strategy
                .create_plan(&video, &Quality::Fast.preset())
                .estimated_frames,
            540
        );
        assert_eq!(
            strategy
                .create_plan(&video, &Quality::Balanced.preset())
                .estimated_frames,
            900
        );
        assert_eq!(
            strategy
                .create_plan(&video, &Quality::High.preset())
                .estimated_frames,
            1800
        );

        let long_video = VideoInfo {
            total_frames: 180_000,
            ..video
        };
        assert_eq!(
            strategy
                .create_plan(&long_video, &Quality::High.preset())
                .estimated_frames,
            180_000
        );
    }

    #[test]
    fn quality_v2_uses_fixed_target_and_candidate_rates() {
        let video = thirty_fps_video();
        let fast = QualityV2FrameSelection.create_plan(&video, &Quality::Fast.preset());
        let balanced = QualityV2FrameSelection.create_plan(&video, &Quality::Balanced.preset());
        let high = QualityV2FrameSelection.create_plan(&video, &Quality::High.preset());
        assert_eq!(fast.estimated_frames, 360);
        assert_eq!(fast.rescue_max_frames, 540);
        assert_eq!(balanced.estimated_frames, 480);
        assert_eq!(balanced.rescue_max_frames, 720);
        assert_eq!(high.estimated_frames, 720);
        assert_eq!(high.rescue_max_frames, 900);
    }

    #[test]
    fn short_video_applies_thirty_frame_minimum_without_exceeding_source() {
        let video = VideoInfo {
            duration: 3.0,
            total_frames: 90,
            ..thirty_fps_video()
        };
        let fast = QualityV2FrameSelection.create_plan(&video, &Quality::Fast.preset());
        assert_eq!(fast.estimated_frames, 30);
        assert_eq!(fast.rescue_max_frames, 30);
        assert!(fast.minimum_frame_override_applied);

        let short = VideoInfo {
            duration: 1.0,
            fps: 20.0,
            total_frames: 20,
            ..thirty_fps_video()
        };
        let plan = QualityV2FrameSelection.create_plan(&short, &Quality::Fast.preset());
        assert_eq!(plan.estimated_frames, 20);
        assert_eq!(plan.selected_frames.len(), 20);
    }

    #[test]
    fn high_no_longer_keeps_every_video_frame() {
        let video = thirty_fps_video();
        let plan = QualityV2FrameSelection.create_plan(&video, &Quality::High.preset());
        assert_eq!(plan.sampling_fps, 12.0);
        assert_eq!(plan.selected_frames.len(), 720);
        assert_eq!(plan.candidate_frames.len(), 900);
        assert!(plan.selected_frames.len() < video.total_frames as usize);
    }

    #[test]
    fn source_fps_below_targets_keeps_every_available_frame() {
        let video = VideoInfo {
            fps: 8.0,
            duration: 10.0,
            total_frames: 80,
            ..thirty_fps_video()
        };
        let plan = QualityV2FrameSelection.create_plan(&video, &Quality::High.preset());
        assert_eq!(plan.estimated_frames, 80);
        assert_eq!(plan.rescue_max_frames, 80);
    }
}
