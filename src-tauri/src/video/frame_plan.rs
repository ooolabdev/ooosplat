use serde::{Deserialize, Serialize};

use crate::{presets::QualityPreset, video::VideoInfo};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FramePlan {
    pub retention_ratio: f64,
    pub sampling_fps: f64,
    pub estimated_frames: u64,
}

pub trait FrameSelectionStrategy {
    fn create_plan(&self, video: &VideoInfo, preset: &QualityPreset) -> FramePlan;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct UniformRatioFrameSelection;

impl FrameSelectionStrategy for UniformRatioFrameSelection {
    fn create_plan(&self, video: &VideoInfo, preset: &QualityPreset) -> FramePlan {
        let duration = video.duration.max(0.001);
        let desired_frames = (duration * preset.target_sampling_fps)
            .round()
            .max(preset.minimum_frames as f64)
            .min(preset.maximum_frames as f64)
            .min(video.total_frames as f64)
            .max(1.0) as u64;
        let sampling_fps = (desired_frames as f64 / duration).min(video.fps);
        let retention_ratio = if video.total_frames == 0 {
            0.0
        } else {
            desired_frames as f64 / video.total_frames as f64
        };
        FramePlan {
            retention_ratio,
            sampling_fps,
            estimated_frames: desired_frames,
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
        }
    }

    #[test]
    fn targets_useful_sampling_rates_instead_of_source_percentages() {
        let strategy = UniformRatioFrameSelection;
        let video = thirty_fps_video();
        assert_eq!(
            strategy
                .create_plan(&video, &Quality::Fast.preset())
                .sampling_fps,
            2.0
        );
        assert_eq!(
            strategy
                .create_plan(&video, &Quality::Balanced.preset())
                .sampling_fps,
            4.0
        );
        assert_eq!(
            strategy
                .create_plan(&video, &Quality::High.preset())
                .sampling_fps,
            6.666666666666667
        );
    }

    #[test]
    fn keeps_frame_counts_inside_each_quality_budget() {
        let strategy = UniformRatioFrameSelection;
        let video = thirty_fps_video();
        assert_eq!(
            strategy
                .create_plan(&video, &Quality::Fast.preset())
                .estimated_frames,
            120
        );
        assert_eq!(
            strategy
                .create_plan(&video, &Quality::Balanced.preset())
                .estimated_frames,
            240
        );
        assert_eq!(
            strategy
                .create_plan(&video, &Quality::High.preset())
                .estimated_frames,
            400
        );

        let long_video = VideoInfo {
            total_frames: 180_000,
            ..video
        };
        assert_eq!(
            strategy
                .create_plan(&long_video, &Quality::High.preset())
                .estimated_frames,
            400
        );
    }

    #[test]
    fn short_video_meets_the_minimum_without_duplicating_source_frames() {
        let strategy = UniformRatioFrameSelection;
        let video = VideoInfo {
            duration: 10.0,
            total_frames: 300,
            ..thirty_fps_video()
        };
        let plan = strategy.create_plan(&video, &Quality::Fast.preset());
        assert_eq!(plan.estimated_frames, 48);
        assert_eq!(plan.sampling_fps, 4.8);
        assert_eq!(plan.retention_ratio, 0.16);
    }
}
