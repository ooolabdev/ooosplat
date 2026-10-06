use std::{
    cmp::Ordering,
    path::{Path, PathBuf},
    sync::Arc,
};

use image::{DynamicImage, GrayImage, ImageBuffer, ImageDecoder, ImageReader, Luma, Rgb, RgbImage};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedImage {
    pub path: PathBuf,
    pub has_alpha_channel: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImageSequenceScan {
    pub info: ImageSequenceInfo,
    pub images: Vec<ScannedImage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageSequenceNaming {
    Primary,
    Reshoot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImagePreparationPhase {
    LinkingFrames,
    InspectingAlpha,
    WritingOpaqueMasks,
    Validating,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImagePreparationProgress {
    pub phase: ImagePreparationPhase,
    pub current: u64,
    pub total: u64,
    pub stage_progress: f32,
}

pub type ImagePreparationObserver = Arc<dyn Fn(ImagePreparationProgress) + Send + Sync + 'static>;

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
    Ok(scan_image_sequence(dir, None)?.info)
}

pub fn scan_image_sequence(
    dir: &Path,
    cancellation: Option<&CancellationToken>,
) -> Result<ImageSequenceScan> {
    scan_image_sequence_with_boundary(dir, cancellation, None)
}

pub fn scan_image_sequence_with_boundary(
    dir: &Path,
    cancellation: Option<&CancellationToken>,
    boundary: Option<&Path>,
) -> Result<ImageSequenceScan> {
    let files = list_images(dir)?;
    if files.len() < 2 {
        return Err(SplatError::InvalidVideo(
            "图片序列文件夹中至少需要 2 张 JPG、JPEG 或 PNG 图片".into(),
        ));
    }

    let mut dimensions: Option<(u32, u32)> = None;
    let mut has_alpha = false;
    let mut images = Vec::with_capacity(files.len());
    for path in files {
        if let Some(boundary) = boundary {
            crate::tasks::authorize_input(&path, &[boundary.to_path_buf()])
                .map_err(|e| SplatError::Process(e.to_string()))?;
        }
        ensure_not_cancelled(cancellation)?;
        let (current, has_alpha_channel) = read_image_header(&path)?;
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
        has_alpha |= has_alpha_channel;
        images.push(ScannedImage {
            path,
            has_alpha_channel,
        });
    }

    let (width, height) = dimensions.expect("two image headers provide dimensions");
    let image_count = images.len() as u64;
    Ok(ImageSequenceScan {
        info: ImageSequenceInfo {
            image_count,
            width,
            height,
            has_alpha,
            requires_large_sequence_confirmation: image_count > LARGE_SEQUENCE_WARNING_COUNT,
        },
        images,
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
    normalized_sequence_name(index, source, "frame_")
}

fn normalized_sequence_name(index: usize, source: &Path, prefix: &str) -> Result<String> {
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .filter(|value| IMAGE_EXTENSIONS.contains(&value.as_str()))
        .ok_or_else(|| SplatError::InvalidPath(source.to_path_buf()))?;
    Ok(format!("{prefix}{:06}.{extension}", index + 1))
}

pub fn prepare_image_sequence(
    source_dir: &Path,
    frames_dir: &Path,
    colmap_frames_dir: &Path,
    masks_dir: &Path,
) -> Result<PreparedImageSequence> {
    let scan = scan_image_sequence(source_dir, None)?;
    prepare_scanned_image_sequence(
        scan,
        frames_dir,
        colmap_frames_dir,
        masks_dir,
        ImageSequenceNaming::Primary,
        None,
        None,
    )
}

pub fn prepare_reshoot_image_sequence(
    source_dir: &Path,
    frames_dir: &Path,
    colmap_frames_dir: &Path,
    masks_dir: &Path,
) -> Result<PreparedImageSequence> {
    let scan = scan_image_sequence(source_dir, None)?;
    prepare_scanned_image_sequence(
        scan,
        frames_dir,
        colmap_frames_dir,
        masks_dir,
        ImageSequenceNaming::Reshoot,
        None,
        None,
    )
}

pub fn prepare_scanned_image_sequence(
    scan: ImageSequenceScan,
    frames_dir: &Path,
    colmap_frames_dir: &Path,
    masks_dir: &Path,
    naming: ImageSequenceNaming,
    observer: Option<ImagePreparationObserver>,
    cancellation: Option<&CancellationToken>,
) -> Result<PreparedImageSequence> {
    let info = &scan.info;
    std::fs::create_dir_all(frames_dir)?;
    std::fs::create_dir_all(colmap_frames_dir)?;
    if info.has_alpha {
        std::fs::create_dir_all(masks_dir)?;
    }

    let mut reporter = ProgressReporter::new(observer);
    let total_images = scan.images.len() as u64;
    let alpha_images = scan
        .images
        .iter()
        .filter(|image| image.has_alpha_channel)
        .count() as u64;
    let link_end = if alpha_images == 0 { 0.9 } else { 0.35 };
    let prefix = match naming {
        ImageSequenceNaming::Primary => "frame_",
        ImageSequenceNaming::Reshoot => "reshoot_",
    };

    let mut names = Vec::with_capacity(scan.images.len());
    for (index, image) in scan.images.iter().enumerate() {
        ensure_not_cancelled(cancellation)?;
        let name = normalized_sequence_name(index, &image.path, prefix)?;
        link_or_copy(&image.path, &frames_dir.join(&name))?;
        if !image.has_alpha_channel {
            link_or_copy(&image.path, &colmap_frames_dir.join(&name))?;
        }
        names.push(name);
        reporter.report(ImagePreparationProgress {
            phase: ImagePreparationPhase::LinkingFrames,
            current: index as u64 + 1,
            total: total_images,
            stage_progress: link_end * (index as f32 + 1.0) / total_images as f32,
        });
    }

    let mut has_transparency = false;
    if alpha_images > 0 {
        let mut completed = 0_u64;
        for (index, image) in scan.images.iter().enumerate() {
            if !image.has_alpha_channel {
                continue;
            }
            ensure_not_cancelled(cancellation)?;
            let decoded = decode_image(&image.path)?;
            let transparent = has_native_transparency(&decoded);
            write_alpha_mask(&decoded, &masks_dir.join(format!("{}.png", names[index])))?;
            if transparent {
                write_premultiplied_colmap_image(&decoded, &colmap_frames_dir.join(&names[index]))?;
            } else {
                link_or_copy(&image.path, &colmap_frames_dir.join(&names[index]))?;
            }
            has_transparency |= transparent;
            completed += 1;
            reporter.report(ImagePreparationProgress {
                phase: ImagePreparationPhase::InspectingAlpha,
                current: completed,
                total: alpha_images,
                stage_progress: 0.35 + 0.5 * completed as f32 / alpha_images as f32,
            });
        }

        if has_transparency {
            let opaque_images = total_images - alpha_images;
            if opaque_images > 0 {
                let opaque_mask = GrayImage::from_pixel(info.width, info.height, Luma([255]));
                let mut completed = 0_u64;
                for (index, image) in scan.images.iter().enumerate() {
                    if image.has_alpha_channel {
                        continue;
                    }
                    ensure_not_cancelled(cancellation)?;
                    save_mask(
                        &opaque_mask,
                        &masks_dir.join(format!("{}.png", names[index])),
                    )?;
                    completed += 1;
                    reporter.report(ImagePreparationProgress {
                        phase: ImagePreparationPhase::WritingOpaqueMasks,
                        current: completed,
                        total: opaque_images,
                        stage_progress: (0.85 + 0.1 * completed as f32 / opaque_images as f32)
                            .min(0.95),
                    });
                }
            }
        } else {
            for (index, image) in scan.images.iter().enumerate() {
                if image.has_alpha_channel {
                    ensure_not_cancelled(cancellation)?;
                    let mask = masks_dir.join(format!("{}.png", names[index]));
                    if mask.exists() {
                        std::fs::remove_file(mask)?;
                    }
                }
            }
        }
    }

    ensure_not_cancelled(cancellation)?;
    reporter.report(ImagePreparationProgress {
        phase: ImagePreparationPhase::Validating,
        current: 0,
        total: total_images,
        stage_progress: 0.95,
    });
    let prepared = validate_named_sequence_with_cancellation(
        frames_dir,
        colmap_frames_dir,
        masks_dir,
        info.image_count,
        has_transparency,
        prefix,
        cancellation,
    )?;
    reporter.report(ImagePreparationProgress {
        phase: ImagePreparationPhase::Validating,
        current: total_images,
        total: total_images,
        stage_progress: 1.0,
    });
    Ok(prepared)
}

pub fn validate_reshoot_image_sequence(
    frames_dir: &Path,
    colmap_frames_dir: &Path,
    masks_dir: &Path,
    expected_count: u64,
    has_alpha: bool,
) -> Result<PreparedImageSequence> {
    validate_named_sequence_with_cancellation(
        frames_dir,
        colmap_frames_dir,
        masks_dir,
        expected_count,
        has_alpha,
        "reshoot_",
        None,
    )
}

pub fn validate_prepared_image_sequence(
    frames_dir: &Path,
    colmap_frames_dir: &Path,
    masks_dir: &Path,
    expected_count: u64,
    has_alpha: bool,
) -> Result<PreparedImageSequence> {
    validate_named_sequence_with_cancellation(
        frames_dir,
        colmap_frames_dir,
        masks_dir,
        expected_count,
        has_alpha,
        "frame_",
        None,
    )
}

fn validate_named_sequence_with_cancellation(
    frames_dir: &Path,
    colmap_frames_dir: &Path,
    masks_dir: &Path,
    expected_count: u64,
    has_alpha: bool,
    prefix: &str,
    cancellation: Option<&CancellationToken>,
) -> Result<PreparedImageSequence> {
    let frames = list_images(frames_dir)?
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(prefix))
        })
        .collect::<Vec<_>>();
    let mut invalid_frame = false;
    for path in &frames {
        ensure_not_cancelled(cancellation)?;
        invalid_frame |= std::fs::metadata(path).map_or(true, |metadata| metadata.len() == 0);
    }
    if frames.len() as u64 != expected_count || invalid_frame {
        return Err(SplatError::Process(format!(
            "图片序列检查点不完整：预期 {expected_count} 张，实际 {} 张",
            frames.len()
        )));
    }

    let colmap_frames = list_images(colmap_frames_dir)?
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(prefix))
        })
        .collect::<Vec<_>>();
    if colmap_frames.len() != frames.len() {
        return Err(SplatError::Process(format!(
            "COLMAP 图片检查点不完整：预期 {} 张，实际 {} 张",
            frames.len(),
            colmap_frames.len()
        )));
    }
    for (frame, colmap_frame) in frames.iter().zip(&colmap_frames) {
        ensure_not_cancelled(cancellation)?;
        if frame.file_name() != colmap_frame.file_name()
            || std::fs::metadata(colmap_frame).map_or(true, |metadata| metadata.len() == 0)
        {
            return Err(SplatError::Process(format!(
                "COLMAP 图片与原始准备帧不一致：{}",
                colmap_frame.display()
            )));
        }
        if read_image_header(frame)?.0 != read_image_header(colmap_frame)?.0 {
            return Err(SplatError::Process(format!(
                "COLMAP 图片尺寸与原始准备帧不一致：{}",
                colmap_frame.display()
            )));
        }
    }

    let mask_count = if has_alpha {
        for frame in &frames {
            ensure_not_cancelled(cancellation)?;
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
            if read_image_header(frame)?.0 != read_image_header(&mask)?.0 {
                return Err(SplatError::Process(format!(
                    "COLMAP Mask 尺寸与原始准备帧不一致：{}",
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

fn read_image_header(path: &Path) -> Result<((u32, u32), bool)> {
    let decoder = ImageReader::open(path)
        .map_err(|error| SplatError::Process(format!("无法读取图片 {}：{error}", path.display())))?
        .with_guessed_format()
        .map_err(|error| {
            SplatError::Process(format!("无法识别图片格式 {}：{error}", path.display()))
        })?
        .into_decoder()
        .map_err(|error| {
            SplatError::Process(format!("无法读取图片头 {}：{error}", path.display()))
        })?;
    Ok((decoder.dimensions(), decoder.color_type().has_alpha()))
}

fn write_alpha_mask(image: &DynamicImage, destination: &Path) -> Result<()> {
    let rgba = image.to_rgba8();
    let mut mask = GrayImage::new(rgba.width(), rgba.height());
    for (x, y, pixel) in rgba.enumerate_pixels() {
        mask.put_pixel(x, y, Luma([pixel[3]]));
    }
    save_mask(&mask, destination)?;
    Ok(())
}

fn has_native_transparency(image: &DynamicImage) -> bool {
    match image {
        DynamicImage::ImageLumaA16(value) => value.pixels().any(|pixel| pixel[1] < u16::MAX),
        DynamicImage::ImageRgba16(value) => value.pixels().any(|pixel| pixel[3] < u16::MAX),
        DynamicImage::ImageLumaA8(value) => value.pixels().any(|pixel| pixel[1] < u8::MAX),
        DynamicImage::ImageRgba8(value) => value.pixels().any(|pixel| pixel[3] < u8::MAX),
        _ => false,
    }
}

fn write_premultiplied_colmap_image(image: &DynamicImage, destination: &Path) -> Result<()> {
    let result = match image {
        DynamicImage::ImageLumaA16(_) | DynamicImage::ImageRgba16(_) => {
            let rgba = image.to_rgba16();
            let mut rgb = ImageBuffer::<Rgb<u16>, Vec<u16>>::new(rgba.width(), rgba.height());
            for (x, y, pixel) in rgba.enumerate_pixels() {
                let alpha = pixel[3];
                rgb.put_pixel(
                    x,
                    y,
                    Rgb([
                        premultiply_u16(pixel[0], alpha),
                        premultiply_u16(pixel[1], alpha),
                        premultiply_u16(pixel[2], alpha),
                    ]),
                );
            }
            DynamicImage::ImageRgb16(rgb).save(destination)
        }
        _ => {
            let rgba = image.to_rgba8();
            let mut rgb = RgbImage::new(rgba.width(), rgba.height());
            for (x, y, pixel) in rgba.enumerate_pixels() {
                let alpha = pixel[3];
                rgb.put_pixel(
                    x,
                    y,
                    Rgb([
                        premultiply_u8(pixel[0], alpha),
                        premultiply_u8(pixel[1], alpha),
                        premultiply_u8(pixel[2], alpha),
                    ]),
                );
            }
            DynamicImage::ImageRgb8(rgb).save(destination)
        }
    };
    result.map_err(|error| {
        SplatError::Process(format!(
            "无法写入 COLMAP 专用图片 {}：{error}",
            destination.display()
        ))
    })
}

fn premultiply_u8(channel: u8, alpha: u8) -> u8 {
    ((u32::from(channel) * u32::from(alpha) + 127) / 255) as u8
}

fn premultiply_u16(channel: u16, alpha: u16) -> u16 {
    ((u64::from(channel) * u64::from(alpha) + 32_767) / 65_535) as u16
}

fn save_mask(mask: &GrayImage, destination: &Path) -> Result<()> {
    mask.save(destination).map_err(|error| {
        SplatError::Process(format!(
            "无法写入 COLMAP Mask {}：{error}",
            destination.display()
        ))
    })
}

fn link_or_copy(source: &Path, destination: &Path) -> Result<()> {
    link_or_copy_with(source, destination, |source, destination| {
        std::fs::hard_link(source, destination)
    })
}

fn link_or_copy_with(
    source: &Path,
    destination: &Path,
    hard_link: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> Result<()> {
    if destination.exists() {
        std::fs::remove_file(destination)?;
    }
    if hard_link(source, destination).is_err() {
        std::fs::copy(source, destination)?;
    }
    Ok(())
}

fn ensure_not_cancelled(cancellation: Option<&CancellationToken>) -> Result<()> {
    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        return Err(SplatError::Cancelled);
    }
    Ok(())
}

struct ProgressReporter {
    observer: Option<ImagePreparationObserver>,
    last_phase: Option<ImagePreparationPhase>,
    last_percent: i32,
}

impl ProgressReporter {
    fn new(observer: Option<ImagePreparationObserver>) -> Self {
        Self {
            observer,
            last_phase: None,
            last_percent: -1,
        }
    }

    fn report(&mut self, progress: ImagePreparationProgress) {
        let percent = (progress.stage_progress.clamp(0.0, 1.0) * 100.0).floor() as i32;
        if self.last_phase == Some(progress.phase)
            && self.last_percent == percent
            && progress.current < progress.total
        {
            return;
        }
        self.last_phase = Some(progress.phase);
        self.last_percent = percent;
        if let Some(observer) = &self.observer {
            observer(progress);
        }
    }
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
    use image::{ColorType, Rgba, RgbaImage};

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
    fn detects_alpha_channels_from_headers_and_rejects_mismatched_sizes() {
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
    fn header_scan_does_not_decode_pixel_payloads() {
        let dir = tempfile::tempdir().unwrap();
        write_rgba(&dir.path().join("1.png"), 255);
        let corrupt = dir.path().join("2.png");
        write_rgba(&corrupt, 255);
        let mut bytes = std::fs::read(&corrupt).unwrap();
        let idat = bytes
            .windows(4)
            .position(|window| window == b"IDAT")
            .expect("test PNG contains IDAT");
        bytes[idat + 4] ^= 0xff;
        std::fs::write(&corrupt, bytes).unwrap();

        let info = analyze_image_sequence(dir.path()).unwrap();

        assert_eq!(info.image_count, 2);
        let frames = tempfile::tempdir().unwrap();
        let colmap_frames = tempfile::tempdir().unwrap();
        let masks = tempfile::tempdir().unwrap();
        let error = prepare_image_sequence(
            dir.path(),
            frames.path(),
            colmap_frames.path(),
            masks.path(),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("2.png"));
    }

    #[test]
    fn opaque_images_are_linked_without_decoding_pixel_payloads() {
        let source = tempfile::tempdir().unwrap();
        let frames = tempfile::tempdir().unwrap();
        let colmap_frames = tempfile::tempdir().unwrap();
        let masks = tempfile::tempdir().unwrap();
        image::RgbImage::new(2, 2)
            .save(source.path().join("1.png"))
            .unwrap();
        let corrupt = source.path().join("2.png");
        image::RgbImage::new(2, 2).save(&corrupt).unwrap();
        let mut bytes = std::fs::read(&corrupt).unwrap();
        let idat = bytes
            .windows(4)
            .position(|window| window == b"IDAT")
            .expect("test PNG contains IDAT");
        bytes[idat + 4] ^= 0xff;
        std::fs::write(&corrupt, bytes).unwrap();

        let scan = scan_image_sequence(source.path(), None).unwrap();
        assert!(!scan.info.has_alpha);
        let prepared = prepare_scanned_image_sequence(
            scan,
            frames.path(),
            colmap_frames.path(),
            masks.path(),
            ImageSequenceNaming::Primary,
            None,
            None,
        )
        .unwrap();

        assert!(!prepared.has_alpha);
        assert_eq!(prepared.mask_count, 0);
        assert!(frames.path().join("frame_000002.png").is_file());
    }

    #[test]
    fn transparent_sequences_generate_matching_masks() {
        let source = tempfile::tempdir().unwrap();
        let frames = tempfile::tempdir().unwrap();
        let colmap_frames = tempfile::tempdir().unwrap();
        let masks = tempfile::tempdir().unwrap();
        write_rgba(&source.path().join("1.png"), 0);
        write_rgba(&source.path().join("2.png"), 255);
        let prepared = prepare_image_sequence(
            source.path(),
            frames.path(),
            colmap_frames.path(),
            masks.path(),
        )
        .unwrap();
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
        assert_eq!(
            image::open(colmap_frames.path().join("frame_000001.png"))
                .unwrap()
                .to_rgb8()
                .get_pixel(0, 0)
                .0,
            [0, 0, 0]
        );
        assert_eq!(
            image::open(colmap_frames.path().join("frame_000002.png"))
                .unwrap()
                .to_rgb8()
                .get_pixel(0, 0)
                .0,
            [10, 20, 30]
        );
    }

    #[test]
    fn rgba8_colmap_images_use_exact_encoded_space_premultiplication() {
        let source = tempfile::tempdir().unwrap();
        let frames = tempfile::tempdir().unwrap();
        let colmap_frames = tempfile::tempdir().unwrap();
        let masks = tempfile::tempdir().unwrap();
        let pixels = [
            Rgba([255, 128, 1, 0]),
            Rgba([255, 128, 1, 128]),
            Rgba([255, 128, 1, 255]),
        ];
        for name in ["1.png", "2.png"] {
            let image = RgbaImage::from_fn(3, 1, |x, _| pixels[x as usize]);
            image.save(source.path().join(name)).unwrap();
        }
        let original = std::fs::read(source.path().join("1.png")).unwrap();

        let prepared = prepare_image_sequence(
            source.path(),
            frames.path(),
            colmap_frames.path(),
            masks.path(),
        )
        .unwrap();

        assert!(prepared.has_alpha);
        assert_eq!(
            std::fs::read(frames.path().join("frame_000001.png")).unwrap(),
            original
        );
        let output = image::open(colmap_frames.path().join("frame_000001.png"))
            .unwrap()
            .to_rgb8();
        assert_eq!(output.get_pixel(0, 0).0, [0, 0, 0]);
        assert_eq!(output.get_pixel(1, 0).0, [128, 64, 1]);
        assert_eq!(output.get_pixel(2, 0).0, [255, 128, 1]);
        assert_eq!(
            image::open(masks.path().join("frame_000001.png.png"))
                .unwrap()
                .to_luma8()
                .into_raw(),
            [0, 128, 255]
        );
    }

    #[test]
    fn rgba16_colmap_images_preserve_depth_and_round_exactly() {
        let source = tempfile::tempdir().unwrap();
        let frames = tempfile::tempdir().unwrap();
        let colmap_frames = tempfile::tempdir().unwrap();
        let masks = tempfile::tempdir().unwrap();
        let pixels = [
            Rgba([65_535_u16, 32_768, 1, 0]),
            Rgba([65_535, 32_768, 1, 32_768]),
            Rgba([65_535, 32_768, 1, 65_535]),
        ];
        for name in ["1.png", "2.png"] {
            let image =
                ImageBuffer::<Rgba<u16>, Vec<u16>>::from_fn(3, 1, |x, _| pixels[x as usize]);
            image.save(source.path().join(name)).unwrap();
        }
        let original = std::fs::read(source.path().join("1.png")).unwrap();

        prepare_image_sequence(
            source.path(),
            frames.path(),
            colmap_frames.path(),
            masks.path(),
        )
        .unwrap();

        assert_eq!(
            std::fs::read(frames.path().join("frame_000001.png")).unwrap(),
            original
        );
        let output_path = colmap_frames.path().join("frame_000001.png");
        let decoder = ImageReader::open(&output_path)
            .unwrap()
            .with_guessed_format()
            .unwrap()
            .into_decoder()
            .unwrap();
        assert_eq!(decoder.color_type(), ColorType::Rgb16);
        let output = image::open(output_path).unwrap().to_rgb16();
        assert_eq!(output.get_pixel(0, 0).0, [0, 0, 0]);
        assert_eq!(output.get_pixel(1, 0).0, [32_768, 16_384, 1]);
        assert_eq!(output.get_pixel(2, 0).0, [65_535, 32_768, 1]);
        assert_eq!(
            image::open(masks.path().join("frame_000001.png.png"))
                .unwrap()
                .to_luma8()
                .into_raw(),
            [0, 128, 255]
        );
    }

    #[test]
    fn opaque_inputs_are_byte_identical_in_frames_and_colmap_frames() {
        let source = tempfile::tempdir().unwrap();
        let frames = tempfile::tempdir().unwrap();
        let colmap_frames = tempfile::tempdir().unwrap();
        let masks = tempfile::tempdir().unwrap();
        image::RgbImage::from_pixel(2, 2, image::Rgb([11, 22, 33]))
            .save(source.path().join("1.jpg"))
            .unwrap();
        image::RgbImage::from_pixel(2, 2, image::Rgb([44, 55, 66]))
            .save(source.path().join("2.png"))
            .unwrap();

        let prepared = prepare_image_sequence(
            source.path(),
            frames.path(),
            colmap_frames.path(),
            masks.path(),
        )
        .unwrap();

        assert!(!prepared.has_alpha);
        for (source_name, prepared_name) in
            [("1.jpg", "frame_000001.jpg"), ("2.png", "frame_000002.png")]
        {
            let expected = std::fs::read(source.path().join(source_name)).unwrap();
            assert_eq!(
                std::fs::read(frames.path().join(prepared_name)).unwrap(),
                expected
            );
            assert_eq!(
                std::fs::read(colmap_frames.path().join(prepared_name)).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn checkpoint_validation_rejects_missing_misnamed_or_mismatched_colmap_images() {
        let source = tempfile::tempdir().unwrap();
        let frames = tempfile::tempdir().unwrap();
        let colmap_frames = tempfile::tempdir().unwrap();
        let masks = tempfile::tempdir().unwrap();
        write_rgba(&source.path().join("1.png"), 64);
        write_rgba(&source.path().join("2.png"), 128);
        prepare_image_sequence(
            source.path(),
            frames.path(),
            colmap_frames.path(),
            masks.path(),
        )
        .unwrap();

        let second = colmap_frames.path().join("frame_000002.png");
        std::fs::remove_file(&second).unwrap();
        assert!(validate_prepared_image_sequence(
            frames.path(),
            colmap_frames.path(),
            masks.path(),
            2,
            true,
        )
        .is_err());

        image::RgbImage::new(2, 2).save(&second).unwrap();
        std::fs::rename(&second, colmap_frames.path().join("wrong.png")).unwrap();
        assert!(validate_prepared_image_sequence(
            frames.path(),
            colmap_frames.path(),
            masks.path(),
            2,
            true,
        )
        .is_err());

        std::fs::remove_file(colmap_frames.path().join("wrong.png")).unwrap();
        image::RgbImage::new(3, 2).save(&second).unwrap();
        assert!(validate_prepared_image_sequence(
            frames.path(),
            colmap_frames.path(),
            masks.path(),
            2,
            true,
        )
        .is_err());
    }

    #[test]
    fn opaque_rgba_sequences_discard_candidate_masks() {
        let source = tempfile::tempdir().unwrap();
        let frames = tempfile::tempdir().unwrap();
        let colmap_frames = tempfile::tempdir().unwrap();
        let masks = tempfile::tempdir().unwrap();
        write_rgba(&source.path().join("1.png"), 255);
        write_rgba(&source.path().join("2.png"), 255);

        let scan = scan_image_sequence(source.path(), None).unwrap();
        assert!(scan.info.has_alpha);
        let prepared = prepare_scanned_image_sequence(
            scan,
            frames.path(),
            colmap_frames.path(),
            masks.path(),
            ImageSequenceNaming::Primary,
            None,
            None,
        )
        .unwrap();

        assert!(!prepared.has_alpha);
        assert_eq!(prepared.mask_count, 0);
        assert!(std::fs::read_dir(masks.path()).unwrap().next().is_none());
    }

    #[test]
    fn mixed_sequences_generate_white_masks_for_opaque_images() {
        let source = tempfile::tempdir().unwrap();
        let frames = tempfile::tempdir().unwrap();
        let colmap_frames = tempfile::tempdir().unwrap();
        let masks = tempfile::tempdir().unwrap();
        image::RgbImage::new(2, 2)
            .save(source.path().join("1.jpg"))
            .unwrap();
        write_rgba(&source.path().join("2.png"), 0);

        let prepared = prepare_image_sequence(
            source.path(),
            frames.path(),
            colmap_frames.path(),
            masks.path(),
        )
        .unwrap();

        assert!(prepared.has_alpha);
        assert_eq!(prepared.mask_count, 2);
        assert_eq!(
            image::open(masks.path().join("frame_000001.jpg.png"))
                .unwrap()
                .to_luma8()
                .get_pixel(0, 0)[0],
            255
        );
        assert_eq!(
            image::open(masks.path().join("frame_000002.png.png"))
                .unwrap()
                .to_luma8()
                .get_pixel(0, 0)[0],
            0
        );
    }

    #[test]
    fn reshoot_sequences_use_a_collision_free_prefix() {
        let source = tempfile::tempdir().unwrap();
        let frames = tempfile::tempdir().unwrap();
        let colmap_frames = tempfile::tempdir().unwrap();
        let masks = tempfile::tempdir().unwrap();
        write_rgba(&source.path().join("1.png"), 0);
        write_rgba(&source.path().join("2.png"), 255);
        let scan = scan_image_sequence(source.path(), None).unwrap();
        let prepared = prepare_scanned_image_sequence(
            scan,
            frames.path(),
            colmap_frames.path(),
            masks.path(),
            ImageSequenceNaming::Reshoot,
            None,
            None,
        )
        .unwrap();
        assert_eq!(prepared.mask_count, 2);
        assert!(frames.path().join("reshoot_000001.png").is_file());
        assert!(masks.path().join("reshoot_000001.png.png").is_file());
    }

    #[test]
    fn preparation_progress_is_monotonic_and_cancellation_is_honored() {
        let source = tempfile::tempdir().unwrap();
        let frames = tempfile::tempdir().unwrap();
        let colmap_frames = tempfile::tempdir().unwrap();
        let masks = tempfile::tempdir().unwrap();
        write_rgba(&source.path().join("1.png"), 0);
        write_rgba(&source.path().join("2.png"), 255);
        let scan = scan_image_sequence(source.path(), None).unwrap();
        let updates = Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = updates.clone();
        let observer: ImagePreparationObserver = Arc::new(move |progress| {
            captured.lock().unwrap().push(progress.stage_progress);
        });

        prepare_scanned_image_sequence(
            scan,
            frames.path(),
            colmap_frames.path(),
            masks.path(),
            ImageSequenceNaming::Primary,
            Some(observer),
            None,
        )
        .unwrap();
        let updates = updates.lock().unwrap();
        assert!(!updates.is_empty());
        assert_eq!(updates.last().copied(), Some(1.0));
        assert!(updates.windows(2).all(|pair| pair[0] <= pair[1]));

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert!(matches!(
            scan_image_sequence(source.path(), Some(&cancellation)),
            Err(SplatError::Cancelled)
        ));

        let cancelled_frames = tempfile::tempdir().unwrap();
        let cancelled_colmap_frames = tempfile::tempdir().unwrap();
        let cancelled_masks = tempfile::tempdir().unwrap();
        let cancellation = CancellationToken::new();
        let cancellation_from_observer = cancellation.clone();
        let observer: ImagePreparationObserver = Arc::new(move |progress| {
            if progress.phase == ImagePreparationPhase::LinkingFrames && progress.current == 1 {
                cancellation_from_observer.cancel();
            }
        });
        let scan = scan_image_sequence(source.path(), None).unwrap();
        assert!(matches!(
            prepare_scanned_image_sequence(
                scan,
                cancelled_frames.path(),
                cancelled_colmap_frames.path(),
                cancelled_masks.path(),
                ImageSequenceNaming::Primary,
                Some(observer),
                Some(&cancellation),
            ),
            Err(SplatError::Cancelled)
        ));
    }

    #[test]
    fn hard_link_failure_falls_back_to_copy() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.jpg");
        let destination = dir.path().join("destination.jpg");
        std::fs::write(&source, b"image bytes").unwrap();

        link_or_copy_with(&source, &destination, |_, _| {
            Err(std::io::Error::new(
                std::io::ErrorKind::CrossesDevices,
                "different volume",
            ))
        })
        .unwrap();

        assert_eq!(std::fs::read(destination).unwrap(), b"image bytes");
    }
}
