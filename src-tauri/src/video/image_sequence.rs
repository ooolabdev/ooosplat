use std::{
    cmp::Ordering,
    path::{Path, PathBuf},
    sync::Arc,
};

use image::{DynamicImage, GrayImage, ImageDecoder, ImageReader, Luma};
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
        quality: None,
        retention_ratio: 1.0,
        sampling_fps: 0.0,
        actual_average_fps: 0.0,
        target_fps: 0.0,
        candidate_fps: 0.0,
        estimated_frames: info.image_count,
        planning_mode: Default::default(),
        preferred_fps: 0.0,
        selected_frames: Vec::new(),
        candidate_frames: Vec::new(),
        minimum_frame_protection: Default::default(),
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
    let scan = scan_image_sequence(source_dir, None)?;
    prepare_scanned_image_sequence(scan, frames_dir, masks_dir, None, None)
}

pub fn prepare_scanned_image_sequence(
    scan: ImageSequenceScan,
    frames_dir: &Path,
    masks_dir: &Path,
    observer: Option<ImagePreparationObserver>,
    cancellation: Option<&CancellationToken>,
) -> Result<PreparedImageSequence> {
    let info = &scan.info;
    std::fs::create_dir_all(frames_dir)?;
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

    let mut names = Vec::with_capacity(scan.images.len());
    for (index, image) in scan.images.iter().enumerate() {
        ensure_not_cancelled(cancellation)?;
        let name = normalized_image_name(index, &image.path)?;
        link_or_copy(&image.path, &frames_dir.join(&name))?;
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
            let transparent =
                write_alpha_mask(&decoded, &masks_dir.join(format!("{}.png", names[index])))?;
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
                        stage_progress: 0.85 + 0.1 * completed as f32 / opaque_images as f32,
                    });
                }
            }
        } else {
            for (index, image) in scan.images.iter().enumerate() {
                if image.has_alpha_channel {
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
    let prepared = validate_prepared_image_sequence(
        frames_dir,
        masks_dir,
        info.image_count,
        has_transparency,
    )?;
    reporter.report(ImagePreparationProgress {
        phase: ImagePreparationPhase::Validating,
        current: total_images,
        total: total_images,
        stage_progress: 1.0,
    });
    Ok(prepared)
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

fn write_alpha_mask(image: &DynamicImage, destination: &Path) -> Result<bool> {
    let rgba = image.to_rgba8();
    let mut mask = GrayImage::new(rgba.width(), rgba.height());
    let mut has_transparency = false;
    for (x, y, pixel) in rgba.enumerate_pixels() {
        has_transparency |= pixel[3] < 255;
        mask.put_pixel(x, y, Luma([pixel[3]]));
    }
    save_mask(&mask, destination)?;
    Ok(has_transparency)
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
        assert!(decode_image(&corrupt).is_err());
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

    #[test]
    fn opaque_rgba_sequences_discard_candidate_masks() {
        let source = tempfile::tempdir().unwrap();
        let frames = tempfile::tempdir().unwrap();
        let masks = tempfile::tempdir().unwrap();
        write_rgba(&source.path().join("1.png"), 255);
        write_rgba(&source.path().join("2.png"), 255);

        let scan = scan_image_sequence(source.path(), None).unwrap();
        assert!(scan.info.has_alpha);
        let prepared =
            prepare_scanned_image_sequence(scan, frames.path(), masks.path(), None, None).unwrap();

        assert!(!prepared.has_alpha);
        assert_eq!(prepared.mask_count, 0);
        assert!(std::fs::read_dir(masks.path()).unwrap().next().is_none());
    }

    #[test]
    fn mixed_sequences_generate_white_masks_for_opaque_images() {
        let source = tempfile::tempdir().unwrap();
        let frames = tempfile::tempdir().unwrap();
        let masks = tempfile::tempdir().unwrap();
        image::RgbImage::new(2, 2)
            .save(source.path().join("1.jpg"))
            .unwrap();
        write_rgba(&source.path().join("2.png"), 0);

        let prepared = prepare_image_sequence(source.path(), frames.path(), masks.path()).unwrap();

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
    fn preparation_progress_is_monotonic_and_cancellation_is_honored() {
        let source = tempfile::tempdir().unwrap();
        let frames = tempfile::tempdir().unwrap();
        let masks = tempfile::tempdir().unwrap();
        write_rgba(&source.path().join("1.png"), 0);
        write_rgba(&source.path().join("2.png"), 255);
        let scan = scan_image_sequence(source.path(), None).unwrap();
        let updates = Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = updates.clone();
        let observer: ImagePreparationObserver = Arc::new(move |progress| {
            captured.lock().unwrap().push(progress.stage_progress);
        });

        prepare_scanned_image_sequence(scan, frames.path(), masks.path(), Some(observer), None)
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
