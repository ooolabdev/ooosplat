use std::{
    cmp::Ordering,
    path::{Path, PathBuf},
};

use image::{DynamicImage, GenericImageView, GrayImage, ImageReader, Luma};
use serde::{Deserialize, Serialize};

use crate::{
    error::{Result, SplatError},
    presets::QualityPreset,
    video::FramePlan,
};

pub const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png"];
pub const LARGE_SEQUENCE_WARNING_COUNT: u64 = 500;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageSequenceInfo {
    pub image_count: u64,
    pub width: u32,
    pub height: u32,
    pub has_alpha: bool,
    pub requires_large_sequence_confirmation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedImageSequence {
    pub image_count: u64,
    pub mask_count: u64,
    pub has_alpha: bool,
}

pub fn is_image_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|ext| IMAGE_EXTENSIONS.contains(&ext.as_str()))
}

pub fn list_images(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.is_dir() {
        return Err(SplatError::InvalidPath(dir.to_path_buf()));
    }
    let mut files = std::fs::read_dir(dir)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_file() && is_image_file(path))
        .collect::<Vec<_>>();
    files.sort_by(|left, right| natural_cmp(&file_name(left), &file_name(right)));
    Ok(files)
}

pub fn analyze_image_sequence(dir: &Path) -> Result<ImageSequenceInfo> {
    let files = list_images(dir)?;
    if files.len() < 2 {
        return Err(SplatError::InvalidVideo(
            "图片序列文件夹中至少需要 2 张 JPG、JPEG 或 PNG 图片".into(),
        ));
    }

    let mut dimensions: Option<(u32, u32)> = None;
    let mut has_alpha = false;
    for path in &files {
        let image = decode_image(path)?;
        let current = image.dimensions();
        if let Some(expected) = dimensions {
            if expected != current {
                return Err(SplatError::InvalidVideo(format!(
                    "图片序列分辨率不一致：{} 为 {}×{}，预期 {}×{}",
                    path.display(),
                    current.0,
                    current.1,
                    expected.0,
                    expected.1
                )));
            }
        } else {
            dimensions = Some(current);
        }
        has_alpha |= image_has_transparency(&image);
    }

    let (width, height) = dimensions.expect("two decoded images provide dimensions");
    let image_count = files.len() as u64;
    Ok(ImageSequenceInfo {
        image_count,
        width,
        height,
        has_alpha,
        requires_large_sequence_confirmation: image_count > LARGE_SEQUENCE_WARNING_COUNT,
    })
}

pub fn create_plan(info: &ImageSequenceInfo, _preset: &QualityPreset) -> FramePlan {
    FramePlan {
        retention_ratio: 1.0,
        sampling_fps: 0.0,
        estimated_frames: info.image_count,
        rescue_max_frames: info.image_count,
        ..FramePlan::default()
    }
}

pub fn normalized_image_name(index: usize, source: &Path) -> Result<String> {
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .filter(|value| IMAGE_EXTENSIONS.contains(&value.as_str()))
        .ok_or_else(|| SplatError::InvalidPath(source.to_path_buf()))?;
    Ok(format!("frame_{:06}.{extension}", index + 1))
}

pub fn prepare_image_sequence(
    source_dir: &Path,
    frames_dir: &Path,
    masks_dir: &Path,
) -> Result<PreparedImageSequence> {
    let info = analyze_image_sequence(source_dir)?;
    std::fs::create_dir_all(frames_dir)?;
    if info.has_alpha {
        std::fs::create_dir_all(masks_dir)?;
    }

    let files = list_images(source_dir)?;
    for (index, source) in files.iter().enumerate() {
        let name = normalized_image_name(index, source)?;
        std::fs::copy(source, frames_dir.join(&name))?;
        if info.has_alpha {
            let image = decode_image(source)?;
            write_alpha_mask(&image, &masks_dir.join(format!("{name}.png")))?;
        }
    }

    validate_prepared_image_sequence(frames_dir, masks_dir, info.image_count, info.has_alpha)
}

pub fn validate_prepared_image_sequence(
    frames_dir: &Path,
    masks_dir: &Path,
    expected_count: u64,
    has_alpha: bool,
) -> Result<PreparedImageSequence> {
    let frames = list_images(frames_dir)?;
    if frames.len() as u64 != expected_count
        || frames
            .iter()
            .any(|path| std::fs::metadata(path).map_or(true, |metadata| metadata.len() == 0))
    {
        return Err(SplatError::Process(format!(
            "图片序列检查点不完整：预期 {expected_count} 张，实际 {} 张",
            frames.len()
        )));
    }

    let mask_count = if has_alpha {
        for frame in &frames {
            let name = frame
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| SplatError::Process("图片序列包含无法读取的文件名".into()))?;
            let mask = masks_dir.join(format!("{name}.png"));
            if !mask.is_file() || std::fs::metadata(&mask)?.len() == 0 {
                return Err(SplatError::Process(format!(
                    "COLMAP Mask 缺失：{}",
                    mask.display()
                )));
            }
        }
        frames.len() as u64
    } else {
        0
    };

    Ok(PreparedImageSequence {
        image_count: frames.len() as u64,
        mask_count,
        has_alpha,
    })
}

fn decode_image(path: &Path) -> Result<DynamicImage> {
    ImageReader::open(path)
        .map_err(|error| SplatError::Process(format!("无法读取图片 {}：{error}", path.display())))?
        .with_guessed_format()
        .map_err(|error| {
            SplatError::Process(format!("无法识别图片格式 {}：{error}", path.display()))
        })?
        .decode()
        .map_err(|error| SplatError::Process(format!("图片解码失败 {}：{error}", path.display())))
}

fn image_has_transparency(image: &DynamicImage) -> bool {
    image.color().has_alpha() && image.to_rgba8().pixels().any(|pixel| pixel[3] < 255)
}

fn write_alpha_mask(image: &DynamicImage, destination: &Path) -> Result<()> {
    let rgba = image.to_rgba8();
    let mut mask = GrayImage::new(rgba.width(), rgba.height());
    for (x, y, pixel) in rgba.enumerate_pixels() {
        mask.put_pixel(x, y, Luma([pixel[3]]));
    }
    mask.save(destination).map_err(|error| {
        SplatError::Process(format!(
            "无法写入 COLMAP Mask {}：{error}",
            destination.display()
        ))
    })
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|value| value.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn natural_cmp(left: &str, right: &str) -> Ordering {
    let left = left.as_bytes();
    let right = right.as_bytes();
    let (mut a, mut b) = (0, 0);
    while a < left.len() && b < right.len() {
        if left[a].is_ascii_digit() && right[b].is_ascii_digit() {
            let (a_start, b_start) = (a, b);
            while a < left.len() && left[a].is_ascii_digit() {
                a += 1;
            }
            while b < right.len() && right[b].is_ascii_digit() {
                b += 1;
            }
            let a_number = left[a_start..a].iter().fold(0_u128, |value, digit| {
                value
                    .saturating_mul(10)
                    .saturating_add((digit - b'0') as u128)
            });
            let b_number = right[b_start..b].iter().fold(0_u128, |value, digit| {
                value
                    .saturating_mul(10)
                    .saturating_add((digit - b'0') as u128)
            });
            match a_number.cmp(&b_number) {
                Ordering::Equal => continue,
                ordering => return ordering,
            }
        }
        match left[a].cmp(&right[b]) {
            Ordering::Equal => {
                a += 1;
                b += 1;
            }
            ordering => return ordering,
        }
    }
    left.len().cmp(&right.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn write_rgba(path: &Path, alpha: u8) {
        RgbaImage::from_pixel(2, 2, Rgba([10, 20, 30, alpha]))
            .save(path)
            .unwrap();
    }

    #[test]
    fn accepts_only_first_phase_formats_and_naturally_sorts() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["frame10.jpg", "frame2.png", "frame1.jpeg", "skip.webp"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        let names = list_images(dir.path())
            .unwrap()
            .into_iter()
            .map(|path| file_name(&path))
            .collect::<Vec<_>>();
        assert_eq!(names, ["frame1.jpeg", "frame2.png", "frame10.jpg"]);
    }

    #[test]
    fn detects_real_png_transparency_and_rejects_mismatched_sizes() {
        let dir = tempfile::tempdir().unwrap();
        write_rgba(&dir.path().join("1.png"), 255);
        write_rgba(&dir.path().join("2.png"), 64);
        let info = analyze_image_sequence(dir.path()).unwrap();
        assert_eq!(info.image_count, 2);
        assert!(info.has_alpha);

        RgbaImage::new(3, 2).save(dir.path().join("2.png")).unwrap();
        assert!(analyze_image_sequence(dir.path()).is_err());
    }

    #[test]
    fn transparent_sequences_generate_matching_masks() {
        let source = tempfile::tempdir().unwrap();
        let frames = tempfile::tempdir().unwrap();
        let masks = tempfile::tempdir().unwrap();
        write_rgba(&source.path().join("1.png"), 0);
        write_rgba(&source.path().join("2.png"), 255);
        let prepared = prepare_image_sequence(source.path(), frames.path(), masks.path()).unwrap();
        assert_eq!(prepared.image_count, 2);
        assert_eq!(prepared.mask_count, 2);
        assert_eq!(
            image::open(masks.path().join("frame_000001.png.png"))
                .unwrap()
                .to_luma8()
                .get_pixel(0, 0)[0],
            0
        );
        assert_eq!(
            image::open(masks.path().join("frame_000002.png.png"))
                .unwrap()
                .to_luma8()
                .get_pixel(0, 0)[0],
            255
        );
    }
}
