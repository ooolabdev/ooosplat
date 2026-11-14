//! Narrow recovery decisions for optional Mapper attempts. Display categories
//! and strings are deliberately not an execution policy.
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use crate::{
    error::{Result, SplatError},
    failure::EngineProcessFailure,
    reconstruction::{good_registered_ratio, ReconstructionQuality, ReconstructionReport},
};

pub(crate) fn is_recoverable_mapper_geometry_failure(error: &SplatError) -> bool {
    match error {
        SplatError::EngineProcess(failure) => failure.is_mapper_geometry_failure(),
        SplatError::Diagnostic { source, .. } => is_recoverable_mapper_geometry_failure(source),
        _ => false,
    }
}

/// Select the same largest/stably ordered model as before, without interpreting
/// a permission error, broken model or missing member as an empty result.
/// `None` means no model was produced, or a complete empty model was read.
pub(crate) fn select_reconstruction_model(
    root: &Path,
    input_images: u64,
) -> Result<Option<(PathBuf, ReconstructionReport)>> {
    if input_images == 0 {
        return Err(SplatError::Process(
            "Invalid Mapper input image count".into(),
        ));
    }
    let root_meta = std::fs::symlink_metadata(root)?;
    if !root_meta.file_type().is_dir() {
        return Err(SplatError::InvalidPath(root.into()));
    }
    let mut children = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(SplatError::InvalidPath(entry.path()));
        }
        if kind.is_dir() {
            children.push(entry.path());
        }
    }
    children.sort();
    let mut candidates = vec![root.to_path_buf()];
    candidates.extend(children);
    let mut best: Option<(PathBuf, ReconstructionReport)> = None;
    for candidate in candidates {
        let Some(report) = inspect_candidate(&candidate, input_images)? else {
            continue;
        };
        if best
            .as_ref()
            .is_none_or(|(_, previous)| report.registered_images > previous.registered_images)
        {
            best = Some((candidate, report));
        }
    }
    Ok(best)
}

fn inspect_candidate(path: &Path, input_images: u64) -> Result<Option<ReconstructionReport>> {
    let mut counts = Vec::with_capacity(3);
    for name in ["cameras.bin", "images.bin", "points3D.bin"] {
        let file = path.join(name);
        let metadata = match std::fs::symlink_metadata(&file) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if !metadata.file_type().is_file() {
            return Err(SplatError::InvalidPath(file));
        }
        let mut reader = File::open(&file)?;
        let mut count = [0; 8];
        reader.read_exact(&mut count)?;
        counts.push((u64::from_le_bytes(count), metadata.len()));
    }
    if counts.is_empty() {
        return Ok(None);
    }
    if counts.len() != 3 {
        return Err(SplatError::Process(
            "COLMAP sparse model output is incomplete".into(),
        ));
    }
    if counts[0].0 == 0 {
        return Err(SplatError::Process("Invalid COLMAP camera count".into()));
    }
    // Zero image/point records are unambiguous only when the complete empty
    // binary tables really contain just their headers. Do not mask a partial
    // non-empty table, trailing bytes or a camera read failure as geometry.
    if counts[1] == (0, 8) && counts[2] == (0, 8) {
        crate::reconstruction::colmap_model::read_cameras(path)?;
        return Ok(None);
    }
    let model = crate::reconstruction::direct_input::read_model(path)?;
    let registered_images = model.images.len() as u64;
    let points_3d = model.points.len() as u64;
    if registered_images > input_images {
        return Err(SplatError::Process(
            "COLMAP model contains more registered images than its input".into(),
        ));
    }
    let registered_ratio = registered_images as f64 / input_images as f64;
    Ok(Some(ReconstructionReport {
        input_images,
        registered_images,
        registered_ratio,
        points_3d,
        quality: if registered_ratio >= good_registered_ratio() {
            ReconstructionQuality::Good
        } else {
            ReconstructionQuality::Warning
        },
    }))
}

pub(super) fn with_previous_geometry_attempts(
    current: SplatError,
    previous: Vec<EngineProcessFailure>,
) -> SplatError {
    if previous.is_empty() || matches!(current, SplatError::Cancelled) {
        return current;
    }
    let mut flattened = Vec::new();
    for attempt in previous {
        flatten_attempt(attempt, &mut flattened);
    }
    match current {
        SplatError::EngineProcess(mut failure) => {
            // At most the three frozen initialization seeds, with any native
            // Caspar/Ceres chain still owned by its respective seed.
            for attempt in std::mem::take(&mut failure.previous_attempts) {
                flatten_attempt(attempt, &mut flattened);
            }
            failure.previous_attempts = flattened;
            SplatError::EngineProcess(failure)
        }
        error => {
            let detail = flattened
                .iter()
                .enumerate()
                .map(|(index, attempt)| {
                    let tail: String = attempt.detail.chars().take(1024).collect();
                    format!(
                        "Previous Mapper geometry attempt {}: {}\n{}",
                        index + 1,
                        attempt.termination,
                        tail
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            SplatError::Diagnostic {
                source: Box::new(error),
                detail,
            }
        }
    }
}

pub(super) fn with_previous_mapper_attempt(
    current: SplatError,
    previous: SplatError,
) -> SplatError {
    match previous {
        SplatError::EngineProcess(failure) => {
            with_previous_geometry_attempts(current, vec![*failure])
        }
        _ => current,
    }
}

fn flatten_attempt(mut attempt: EngineProcessFailure, output: &mut Vec<EngineProcessFailure>) {
    for earlier in std::mem::take(&mut attempt.previous_attempts) {
        flatten_attempt(earlier, output);
    }
    output.push(attempt);
    // Original Mapper plus three seeds, each with at most Caspar/Ceres.
    // Keep the most recent/root-cause context if a foreign caller passes more.
    const MAX_CONTEXT_ATTEMPTS: usize = 8;
    if output.len() > MAX_CONTEXT_ATTEMPTS {
        output.remove(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reconstruction::{
        colmap_model::ColmapCamera,
        direct_input::{Model, ModelImage, Observation, Point},
    };
    use crate::video::panorama::PosedImage;

    fn counted_file(path: &Path, count: u64) {
        let mut bytes = count.to_le_bytes().to_vec();
        bytes.push(0);
        std::fs::write(path, bytes).unwrap();
    }

    fn model(path: &Path, images: u32) {
        Model {
            cameras: vec![ColmapCamera {
                id: 1,
                model_id: 2,
                model: "SIMPLE_RADIAL".into(),
                width: 64,
                height: 48,
                params: vec![50., 32., 24., 0.],
            }],
            images: (1..=images)
                .map(|id| ModelImage {
                    pose: PosedImage {
                        id,
                        camera_id: 1,
                        name: format!("{id}.png"),
                        qvec: [1., 0., 0., 0.],
                        tvec: [id as f64, 0., 0.],
                    },
                    observations: vec![Observation {
                        x: 32.,
                        y: 24.,
                        point: Some(1),
                    }],
                })
                .collect(),
            points: vec![Point {
                id: 1,
                xyz: [0., 0., 1.],
                rgb: [128; 3],
                error: 0.,
                track: (1..=images).map(|id| (id, 0)).collect(),
            }],
        }
        .write(path)
        .unwrap();
    }

    #[test]
    fn selector_keeps_stable_largest_model_without_swallowing_broken_siblings() {
        let root = tempfile::tempdir().unwrap();
        assert!(select_reconstruction_model(root.path(), 20)
            .unwrap()
            .is_none());
        for (name, images) in [("0", 3), ("1", 12), ("2", 12)] {
            let path = root.path().join(name);
            model(&path, images);
        }
        let (selected, report) = select_reconstruction_model(root.path(), 20)
            .unwrap()
            .unwrap();
        assert_eq!(selected, root.path().join("1"));
        assert_eq!(report.registered_images, 12);
        std::fs::write(root.path().join("2/images.bin"), b"broken").unwrap();
        assert!(matches!(
            select_reconstruction_model(root.path(), 20),
            Err(SplatError::Io(_))
        ));
    }

    #[test]
    fn readable_headers_do_not_hide_corrupt_records_or_observation_references() {
        let root = tempfile::tempdir().unwrap();
        model(root.path(), 3);
        let mut broken = crate::reconstruction::direct_input::read_model(root.path()).unwrap();
        broken.images[0].observations[0].point = Some(999);
        broken.write(root.path()).unwrap();
        assert!(select_reconstruction_model(root.path(), 20).is_err());
        model(root.path(), 3);
        counted_file(&root.path().join("images.bin"), 3);
        assert!(select_reconstruction_model(root.path(), 20).is_err());
    }

    #[test]
    fn partial_model_and_non_file_members_are_not_geometry_rejections() {
        let root = tempfile::tempdir().unwrap();
        counted_file(&root.path().join("cameras.bin"), 1);
        assert!(select_reconstruction_model(root.path(), 20).is_err());
        std::fs::create_dir(root.path().join("images.bin")).unwrap();
        assert!(matches!(
            select_reconstruction_model(root.path(), 20),
            Err(SplatError::InvalidPath(_))
        ));
        assert!(matches!(
            select_reconstruction_model(&root.path().join("missing"), 20),
            Err(SplatError::Io(_))
        ));
    }

    #[test]
    fn generic_source_text_or_io_never_enables_recovery() {
        for error in [
            SplatError::Process("Failed to create any sparse model".into()),
            SplatError::Io(std::io::Error::from_raw_os_error(5)),
            SplatError::Cancelled,
        ] {
            assert!(!is_recoverable_mapper_geometry_failure(&error));
        }
    }
}
