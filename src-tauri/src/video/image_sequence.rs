//! Image-sequence (image folder) input support.
//!
//! OOOSplat originally only accepted a single video. This module lets the user
//! hand OOOSplat an ordered set of images (a folder of photos) and reconstruct
//! from them directly, bypassing FFprobe/FFmpeg frame extraction. The images
//! are later normalised into `work/frames/` so the COLMAP + Brush pipeline is
//! unchanged. Design follows nerfstudio's image-input abstraction
//! (`ImagesToNerfstudioDataset`): collect, sort by filename, then hand a stable
//! image set to COLMAP.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    error::{Result, SplatError},
    presets::QualityPreset,
    video::FramePlan,
};

/// Image extensions accepted as a sequence input. Mirrors the formats most
/// COLMAP builds can load (other formats such as CR2 require extra codecs).
pub const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "tif", "tiff", "bmp", "webp"];

/// A lightweight description of an image-sequence input, shown in the UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageSequenceInfo {
    pub count: u64,
}

/// Returns true when `path` looks like a supported still image file.
pub fn is_image_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|ext| IMAGE_EXTENSIONS.contains(&ext.as_str()))
}

/// Collect image files under `dir`, sorted by full path so ordering is
/// deterministic. This mirrors nerfstudio's `sorted()` filename sort and is the
/// ordering that makes COLMAP `sequential_matcher` meaningful.
pub fn list_images(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.is_dir() {
        return Err(SplatError::InvalidPath(dir.to_path_buf()));
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_file() && is_image_file(path))
        .collect();
    files.sort();
    Ok(files)
}

/// How many images matched in `dir`.
pub fn image_count(dir: &Path) -> Result<u64> {
    Ok(list_images(dir)?.len() as u64)
}

/// Build a frame plan for an image sequence.
///
/// Images are not sampled by FPS the way video is: the user's chosen set is
/// what reaches COLMAP, so we retain all of them. `retention_ratio` stays at
/// 1.0 and `estimated_frames` equals the image count (a cap can be applied by
/// callers that want to downsample very large sets).
pub fn create_plan(count: u64, _preset: &QualityPreset) -> FramePlan {
    FramePlan {
        retention_ratio: 1.0,
        sampling_fps: 0.0,
        estimated_frames: count,
    }
}

/// Validate that `dir` contains at least one supported image.
pub fn validate_image_sequence(dir: &Path) -> Result<()> {
    if !dir.is_dir() {
        return Err(SplatError::InvalidPath(dir.to_path_buf()));
    }
    if list_images(dir)?.is_empty() {
        return Err(SplatError::InvalidVideo(
            "图片序列为空，未找到支持的图片文件".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_supported_image_extensions() {
        assert!(is_image_file(Path::new("a.JPG")));
        assert!(is_image_file(Path::new("b.jpeg")));
        assert!(is_image_file(Path::new("c.PNG")));
        assert!(!is_image_file(Path::new("d.mp4")));
        assert!(!is_image_file(Path::new("e.txt")));
    }

    #[test]
    fn lists_and_sorts_images_only() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["z.png", "a.png", "m.jpeg", "skip.mp4"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        let images = list_images(dir.path()).unwrap();
        let names: Vec<_> = images
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["a.png", "m.jpeg", "z.png"]);
    }

    #[test]
    fn empty_folders_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(validate_image_sequence(dir.path()).is_err());
    }

    #[test]
    fn image_plan_retains_all_images() {
        let plan = create_plan(12, &crate::presets::Quality::Balanced.preset());
        assert_eq!(plan.retention_ratio, 1.0);
        assert_eq!(plan.estimated_frames, 12);
    }
}
