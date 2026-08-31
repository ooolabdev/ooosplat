use serde::{Deserialize, Serialize};

use crate::error::{Result, SplatError};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoInfo {
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub total_frames: u64,
    pub codec: String,
    pub rotation: i32,
    #[serde(default)]
    pub pixel_format: String,
    #[serde(default)]
    pub has_alpha: bool,
}

#[derive(Debug, Deserialize)]
struct ProbeDocument {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    format: Option<ProbeFormat>,
}

#[derive(Debug, Deserialize)]
struct ProbeStream {
    width: Option<u32>,
    height: Option<u32>,
    codec_name: Option<String>,
    pix_fmt: Option<String>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
    nb_frames: Option<String>,
    tags: Option<ProbeTags>,
    #[serde(default)]
    side_data_list: Vec<ProbeSideData>,
}

#[derive(Debug, Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProbeTags {
    rotate: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProbeSideData {
    rotation: Option<i32>,
}

pub fn parse_ffprobe_json(json: &str) -> Result<VideoInfo> {
    let document: ProbeDocument = serde_json::from_str(json)?;
    let stream = document
        .streams
        .first()
        .ok_or_else(|| SplatError::InvalidVideo("文件中没有视频轨道".into()))?;
    let duration = document
        .format
        .and_then(|format| format.duration)
        .and_then(|value| parse_positive_f64(&value))
        .ok_or_else(|| SplatError::InvalidVideo("无法读取视频时长".into()))?;
    if duration < 0.25 {
        return Err(SplatError::InvalidVideo(
            "视频太短，至少需要 0.25 秒".into(),
        ));
    }

    let width = stream
        .width
        .filter(|value| *value > 0)
        .ok_or_else(|| SplatError::InvalidVideo("视频宽度无效".into()))?;
    let height = stream
        .height
        .filter(|value| *value > 0)
        .ok_or_else(|| SplatError::InvalidVideo("视频高度无效".into()))?;
    let fps = stream
        .avg_frame_rate
        .as_deref()
        .and_then(parse_rate)
        .or_else(|| stream.r_frame_rate.as_deref().and_then(parse_rate))
        .filter(|value| value.is_finite() && *value > 0.0 && *value <= 1_000.0)
        .ok_or_else(|| SplatError::InvalidVideo("视频 FPS 无效".into()))?;
    let total_frames = stream
        .nb_frames
        .as_deref()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or_else(|| (duration * fps).round() as u64);
    if total_frames == 0 {
        return Err(SplatError::InvalidVideo("无法估算视频画面数量".into()));
    }

    let rotation = stream
        .side_data_list
        .iter()
        .find_map(|data| data.rotation)
        .or_else(|| {
            stream
                .tags
                .as_ref()
                .and_then(|tags| tags.rotate.as_deref())
                .and_then(|value| value.parse().ok())
        })
        .unwrap_or(0);

    let pixel_format = stream.pix_fmt.clone().unwrap_or_default();
    Ok(VideoInfo {
        duration,
        width,
        height,
        fps,
        total_frames,
        codec: stream
            .codec_name
            .clone()
            .unwrap_or_else(|| "unknown".into()),
        rotation,
        has_alpha: pixel_format_has_alpha(&pixel_format),
        pixel_format,
    })
}

/// Mirrors FFmpeg's AV_PIX_FMT_FLAG_ALPHA for pixel formats that can be
/// emitted by the bundled decoders. FFprobe reports the decoder output format,
/// so this detects transparency by content rather than by file extension.
fn pixel_format_has_alpha(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.starts_with("yuva")
        || value.starts_with("gbrap")
        || value.starts_with("rgba")
        || value.starts_with("bgra")
        || value.starts_with("rgbaf")
        || value.starts_with("yaf")
        || matches!(
            value.as_str(),
            "pal8"
                | "argb"
                | "abgr"
                | "ya8"
                | "ya16be"
                | "ya16le"
                | "ayuv"
                | "uyva"
                | "vuya"
                | "ayuv64be"
                | "ayuv64le"
        )
}

fn parse_positive_f64(value: &str) -> Option<f64> {
    value
        .parse::<f64>()
        .ok()
        .filter(|number| number.is_finite() && *number > 0.0)
}

fn parse_rate(value: &str) -> Option<f64> {
    let (numerator, denominator) = value.split_once('/')?;
    let numerator = numerator.parse::<f64>().ok()?;
    let denominator = denominator.parse::<f64>().ok()?;
    if denominator == 0.0 {
        return None;
    }
    Some(numerator / denominator)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json_and_prefers_nb_frames() {
        let json = r#"{
          "streams": [{
            "width": 1920,
            "height": 1080,
            "codec_name": "h264",
            "pix_fmt": "yuv420p",
            "avg_frame_rate": "30000/1001",
            "r_frame_rate": "30/1",
            "nb_frames": "1798",
            "side_data_list": [{"rotation": -90}]
          }],
          "format": {"duration": "60.0"}
        }"#;
        let info = parse_ffprobe_json(json).unwrap();
        assert!((info.fps - 29.970_029).abs() < 0.000_1);
        assert_eq!(info.total_frames, 1798);
        assert_eq!(info.rotation, -90);
        assert_eq!(info.pixel_format, "yuv420p");
        assert!(!info.has_alpha);
    }

    #[test]
    fn estimates_frames_when_nb_frames_is_unavailable() {
        let json = r#"{
          "streams": [{
            "width": 1280,
            "height": 720,
            "codec_name": "hevc",
            "avg_frame_rate": "30/1",
            "r_frame_rate": "30/1",
            "nb_frames": "N/A"
          }],
          "format": {"duration": "60.0"}
        }"#;
        assert_eq!(parse_ffprobe_json(json).unwrap().total_frames, 1800);
    }

    #[test]
    fn rejects_missing_video_stream() {
        let error =
            parse_ffprobe_json(r#"{"streams": [], "format": {"duration": "10"}}"#).unwrap_err();
        assert!(error.to_string().contains("没有视频轨道"));
    }

    #[test]
    fn detects_common_alpha_pixel_formats() {
        for format in [
            "argb",
            "rgba",
            "yuva444p10le",
            "gbrap12le",
            "rgba64le",
            "pal8",
        ] {
            assert!(pixel_format_has_alpha(format), "{format}");
        }
        for format in ["yuv420p", "rgb24", "gbrp10le", "nv12", ""] {
            assert!(!pixel_format_has_alpha(format), "{format}");
        }
    }

    #[test]
    fn old_probe_documents_default_to_opaque() {
        let json = r#"{
          "streams": [{
            "width": 1280,
            "height": 720,
            "codec_name": "h264",
            "avg_frame_rate": "30/1",
            "nb_frames": "300"
          }],
          "format": {"duration": "10"}
        }"#;
        let info = parse_ffprobe_json(json).unwrap();
        assert_eq!(info.pixel_format, "");
        assert!(!info.has_alpha);
    }
}
