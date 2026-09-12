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

#[derive(Debug, Default, Clone, Copy)]
pub struct SmartFrameSelection;

impl FrameSelectionStrategy for SmartFrameSelection {
    fn create_plan(&self, video: &VideoInfo, preset: &QualityPreset) -> FramePlan {
        // 智能过滤发生在 FFmpeg 抽帧之后；先按 1.5 倍超采样，给质量过滤留下足够候选帧。
        // 过滤可能剔除模糊、曝光异常和时序冗余帧，超采样可避免有效帧数量不足。
        const OVERSAMPLE_FACTOR: f64 = 1.5;
        let sampling_fps =
            (video.fps * preset.frame_retention_ratio * OVERSAMPLE_FACTOR).min(video.fps);
        FramePlan {
            retention_ratio: preset.frame_retention_ratio,
            sampling_fps,
            estimated_frames: (video.total_frames as f64
                * (sampling_fps / video.fps.max(f64::EPSILON)))
            .round() as u64,
        }
    }
}

impl FrameSelectionStrategy for UniformRatioFrameSelection {
    fn create_plan(&self, video: &VideoInfo, preset: &QualityPreset) -> FramePlan {
        FramePlan {
            retention_ratio: preset.frame_retention_ratio,
            sampling_fps: video.fps * preset.frame_retention_ratio,
            estimated_frames: ((video.total_frames as f64) * preset.frame_retention_ratio).round()
                as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets::Quality;

    #[test]
    fn smart_selection_oversamples_for_post_filtering() {
        let plan =
            SmartFrameSelection.create_plan(&thirty_fps_video(), &Quality::Balanced.preset());
        assert_eq!(plan.sampling_fps, 22.5);
        assert_eq!(plan.estimated_frames, 1_350);
    }

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
}
