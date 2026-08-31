use std::{
    collections::BTreeSet,
    ffi::OsString,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    error::{Result, SplatError},
    process::{ProcessManager, ProcessObserver, ProcessSpec},
    video::FramePlan,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FrameImageFormat {
    Jpeg,
    Png,
}

impl FrameImageFormat {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Jpeg => "jpeg",
            Self::Png => "png",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameExtractionResult {
    pub frame_count: u64,
    pub image_format: FrameImageFormat,
    pub mask_count: u64,
    pub has_alpha: bool,
}

#[allow(clippy::too_many_arguments)]
pub async fn extract_uniform_frames(
    executable: &Path,
    input: &Path,
    output_directory: &Path,
    mask_directory: &Path,
    plan: &FramePlan,
    has_alpha: bool,
    log_path: Option<PathBuf>,
    process_manager: &ProcessManager,
    observer: Option<ProcessObserver>,
) -> Result<FrameExtractionResult> {
    if !input.is_file() {
        return Err(SplatError::InvalidPath(input.to_path_buf()));
    }
    ensure_clean_output(output_directory, mask_directory).await?;
    tokio::fs::create_dir_all(output_directory).await?;
    if has_alpha {
        tokio::fs::create_dir_all(mask_directory).await?;
    }

    let args = frame_extraction_args(
        input,
        output_directory,
        mask_directory,
        plan.sampling_fps,
        has_alpha,
    );
    let output = process_manager
        .run(ProcessSpec {
            executable: executable.to_path_buf(),
            args,
            working_directory: output_directory.parent().map(Path::to_path_buf),
            log_path,
            observer,
        })
        .await?;
    if !output.success {
        let operation = if has_alpha {
            "透明画面或 Alpha Mask 提取"
        } else {
            "画面提取"
        };
        return Err(SplatError::Process(format!(
            "FFmpeg {operation}失败，退出码 {:?}",
            output.exit_code
        )));
    }

    validate_extraction(output_directory, mask_directory, has_alpha).await
}

fn frame_extraction_args(
    input: &Path,
    output_directory: &Path,
    mask_directory: &Path,
    sampling_fps: f64,
    has_alpha: bool,
) -> Vec<OsString> {
    let mut args = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-nostats".into(),
        "-y".into(),
        "-i".into(),
        input.as_os_str().to_owned(),
    ];
    let scale = "scale='min(1920,iw)':'min(1920,ih)':force_original_aspect_ratio=decrease";
    if has_alpha {
        let filter = format!(
            "[0:v]fps={sampling_fps:.8},{scale},format=rgba,split=2[rgba][masksrc];[masksrc]alphaextract[mask]"
        );
        args.extend([
            "-filter_complex".into(),
            filter.into(),
            "-map".into(),
            "[rgba]".into(),
            "-c:v".into(),
            "png".into(),
            "-pix_fmt".into(),
            "rgba".into(),
            "-start_number".into(),
            "1".into(),
            output_directory.join("frame_%06d.png").into_os_string(),
            "-map".into(),
            "[mask]".into(),
            "-c:v".into(),
            "png".into(),
            "-pix_fmt".into(),
            "gray".into(),
            "-start_number".into(),
            "1".into(),
            mask_directory.join("frame_%06d.png.png").into_os_string(),
        ]);
    } else {
        let filter = format!("fps={sampling_fps:.8},{scale}");
        args.extend([
            "-vf".into(),
            filter.into(),
            "-q:v".into(),
            "2".into(),
            "-start_number".into(),
            "1".into(),
            output_directory.join("frame_%06d.jpg").into_os_string(),
        ]);
    }
    args.extend(["-progress".into(), "pipe:1".into()]);
    args
}

async fn ensure_clean_output(frames: &Path, masks: &Path) -> Result<()> {
    for directory in [frames, masks] {
        if !directory.exists() {
            continue;
        }
        let mut entries = tokio::fs::read_dir(directory).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.path().is_file() && is_pipeline_image(&entry.path()) {
                return Err(SplatError::Process(format!(
                    "输出目录 {} 中已有图像；为避免混用残缺结果，任务已停止",
                    directory.display()
                )));
            }
        }
    }
    Ok(())
}

async fn validate_extraction(
    frames: &Path,
    masks: &Path,
    has_alpha: bool,
) -> Result<FrameExtractionResult> {
    let expected_extension = if has_alpha { "png" } else { "jpg" };
    let frame_names = image_names(frames, expected_extension).await?;
    if frame_names.is_empty() {
        return Err(SplatError::Process("FFmpeg 未输出任何画面".into()));
    }

    let mask_count = if has_alpha {
        let mask_names = image_names(masks, "png").await?;
        if frame_names.len() != mask_names.len()
            || frame_names
                .iter()
                .any(|name| !mask_names.contains(&format!("{name}.png")))
        {
            return Err(SplatError::Process(format!(
                "透明画面与 Alpha Mask 不完整：画面 {} 张，Mask {} 张",
                frame_names.len(),
                mask_names.len()
            )));
        }
        mask_names.len() as u64
    } else {
        0
    };

    Ok(FrameExtractionResult {
        frame_count: frame_names.len() as u64,
        image_format: if has_alpha {
            FrameImageFormat::Png
        } else {
            FrameImageFormat::Jpeg
        },
        mask_count,
        has_alpha,
    })
}

async fn image_names(directory: &Path, extension: &str) -> Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    let mut entries = tokio::fs::read_dir(directory).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .is_some_and(|value| value.eq_ignore_ascii_case(extension))
        {
            names.insert(entry.file_name().to_string_lossy().into_owned());
        }
    }
    Ok(names)
}

fn is_pipeline_image(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("jpg")
            || extension.eq_ignore_ascii_case("jpeg")
            || extension.eq_ignore_ascii_case("png")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args_as_strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn opaque_extraction_keeps_the_jpeg_pipeline() {
        let args = args_as_strings(frame_extraction_args(
            Path::new("input.mov"),
            Path::new("frames"),
            Path::new("masks"),
            15.0,
            false,
        ));
        assert!(args.iter().any(|value| value.ends_with("frame_%06d.jpg")));
        assert!(!args.iter().any(|value| value == "-filter_complex"));
        assert!(!args.iter().any(|value| value.contains("alphaextract")));
    }

    #[test]
    fn alpha_extraction_emits_rgba_frames_and_colmap_masks() {
        let args = args_as_strings(frame_extraction_args(
            Path::new("input.mov"),
            Path::new("frames"),
            Path::new("masks"),
            15.0,
            true,
        ));
        assert!(args.iter().any(|value| value == "-filter_complex"));
        assert!(args.iter().any(|value| value.contains("alphaextract")));
        assert!(args.iter().any(|value| value.ends_with("frame_%06d.png")));
        assert!(args
            .iter()
            .any(|value| value.ends_with("frame_%06d.png.png")));
    }

    #[tokio::test]
    async fn validates_matching_alpha_frame_and_mask_names() {
        let temporary = tempfile::tempdir().unwrap();
        let frames = temporary.path().join("frames");
        let masks = temporary.path().join("masks");
        tokio::fs::create_dir_all(&frames).await.unwrap();
        tokio::fs::create_dir_all(&masks).await.unwrap();
        tokio::fs::write(frames.join("frame_000001.png"), b"png")
            .await
            .unwrap();
        tokio::fs::write(masks.join("frame_000001.png.png"), b"mask")
            .await
            .unwrap();
        let result = validate_extraction(&frames, &masks, true).await.unwrap();
        assert_eq!(result.frame_count, 1);
        assert_eq!(result.mask_count, 1);
        assert_eq!(result.image_format, FrameImageFormat::Png);
    }

    #[tokio::test]
    async fn rejects_partial_alpha_output() {
        let temporary = tempfile::tempdir().unwrap();
        let frames = temporary.path().join("frames");
        let masks = temporary.path().join("masks");
        tokio::fs::create_dir_all(&frames).await.unwrap();
        tokio::fs::create_dir_all(&masks).await.unwrap();
        tokio::fs::write(frames.join("frame_000001.png"), b"png")
            .await
            .unwrap();
        let error = validate_extraction(&frames, &masks, true)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Alpha Mask 不完整"));
    }
}
