use serde::Serialize;

use crate::{
    presets::Quality,
    video::{FramePlan, VideoInfo},
};

#[derive(Debug, Clone)]
pub struct RuntimeSample {
    pub video: VideoInfo,
    pub quality: Quality,
    pub extracted_frames: u64,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EstimateConfidence {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeEstimate {
    pub estimated_ms: u64,
    pub lower_bound_ms: u64,
    pub upper_bound_ms: u64,
    pub confidence: EstimateConfidence,
    pub sample_count: usize,
    pub basis: String,
}

pub fn estimate_runtime(
    video: &VideoInfo,
    plan: &FramePlan,
    quality: Quality,
    total_vram_mb: Option<u64>,
    samples: &[RuntimeSample],
) -> RuntimeEstimate {
    let base = base_estimate_ms(video, plan.estimated_frames, quality, total_vram_mb);
    let mut calibration = samples
        .iter()
        .filter(|sample| sample.duration_ms >= 10_000 && sample.extracted_frames > 0)
        .map(|sample| {
            let expected = base_estimate_ms(
                &sample.video,
                sample.extracted_frames,
                sample.quality,
                total_vram_mb,
            );
            (sample.duration_ms as f64 / expected.max(1) as f64).clamp(0.60, 2.50)
        })
        .collect::<Vec<_>>();
    calibration.sort_by(f64::total_cmp);
    let sample_count = calibration.len();
    let factor = median(&calibration).unwrap_or(1.0);
    let estimated_ms = (base as f64 * factor).round().max(1_000.0) as u64;
    let (confidence, lower_factor, upper_factor) = match sample_count {
        0 => (EstimateConfidence::Low, 0.55, 1.75),
        1..=2 => (EstimateConfidence::Low, 0.60, 1.60),
        3..=5 => (EstimateConfidence::Medium, 0.72, 1.38),
        _ => (EstimateConfidence::High, 0.82, 1.22),
    };
    RuntimeEstimate {
        estimated_ms,
        lower_bound_ms: (estimated_ms as f64 * lower_factor).round() as u64,
        upper_bound_ms: (estimated_ms as f64 * upper_factor).round() as u64,
        confidence,
        sample_count,
        basis: if sample_count == 0 {
            "根据视频分辨率、抽帧数、质量档位和显存预算估算；完成任务后会自动校准".into()
        } else {
            format!(
                "根据视频分辨率、抽帧数、质量档位、显存预算和本机 {sample_count} 个已完成任务校准"
            )
        },
    }
}

fn base_estimate_ms(
    video: &VideoInfo,
    frames: u64,
    quality: Quality,
    total_vram_mb: Option<u64>,
) -> u64 {
    let preset = quality.preset().for_vram_mb(total_vram_mb);
    let megapixels = video.width as f64 * video.height as f64 / 1_000_000.0;
    let pixel_factor = (megapixels / 2.0736).sqrt().clamp(0.65, 1.8);
    let frame_count = frames.max(1) as f64;

    // Feature work grows roughly linearly with frames and pixels, while global
    // reconstruction/BA grows super-linearly with the number of registered views.
    let preparation_ms = 8_000.0 + frame_count * 55.0 * pixel_factor;
    let reconstruction_ms = 176.0 * frame_count.powf(1.5);
    let resolution_factor = (preset.brush_max_resolution as f64 / 960.0).powf(1.35);
    let iteration_factor = preset.brush_iterations as f64 / 6_000.0;
    let sh_factor = 1.0 + f64::from(preset.brush_sh_degree.saturating_sub(2)) * 0.08;
    let brush_ms = 80_000.0 * resolution_factor * iteration_factor * sh_factor;
    (preparation_ms + reconstruction_ms + brush_ms).round() as u64
}

fn median(values: &[f64]) -> Option<f64> {
    match values.len() {
        0 => None,
        length if length % 2 == 1 => Some(values[length / 2]),
        length => Some((values[length / 2 - 1] + values[length / 2]) / 2.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video() -> VideoInfo {
        VideoInfo {
            duration: 12.52,
            width: 3840,
            height: 2160,
            fps: 60.0,
            total_frames: 752,
            codec: "hevc".into(),
            rotation: 0,
        }
    }

    #[test]
    fn quality_and_frame_count_increase_the_estimate() {
        let video = video();
        let fast = base_estimate_ms(&video, 48, Quality::Fast, Some(8_151));
        let balanced = base_estimate_ms(&video, 72, Quality::Balanced, Some(8_151));
        let high = base_estimate_ms(&video, 120, Quality::High, Some(8_151));
        assert!(fast < balanced && balanced < high);
    }

    #[test]
    fn completed_local_runs_calibrate_and_narrow_the_range() {
        let video = video();
        let plan = FramePlan {
            retention_ratio: 0.064,
            sampling_fps: 3.83,
            estimated_frames: 48,
        };
        let sample = RuntimeSample {
            video: video.clone(),
            quality: Quality::Fast,
            extracted_frames: 226,
            duration_ms: 858_613,
        };
        let estimate = estimate_runtime(
            &video,
            &plan,
            Quality::Fast,
            Some(8_151),
            &[sample.clone(), sample.clone(), sample],
        );
        assert_eq!(estimate.confidence, EstimateConfidence::Medium);
        assert_eq!(estimate.sample_count, 3);
        assert!(estimate.lower_bound_ms < estimate.estimated_ms);
        assert!(estimate.upper_bound_ms > estimate.estimated_ms);
    }
}
