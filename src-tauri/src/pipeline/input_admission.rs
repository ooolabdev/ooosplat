//! Temporary capability gate for NEW imports only. Do not call this from model
//! readers, artifact validation or saved-task execution: existing COLMAP-backed
//! projects and continuations must remain usable.
use std::path::{Path, PathBuf};

use crate::error::{InputAdmissionReason, Result, SplatError};

use super::inputs::{GenerationInputs, InputSource};

/// Re-enable this single capability when direct COLMAP imports are supported again.
pub const COLMAP_IMPORT_ENABLED: bool = false;

pub fn ensure_colmap_import_enabled() -> Result<()> {
    if COLMAP_IMPORT_ENABLED {
        Ok(())
    } else {
        Err(SplatError::InputAdmission(
            InputAdmissionReason::ColmapDisabled,
        ))
    }
}

pub fn unsupported_input() -> SplatError {
    SplatError::InputAdmission(InputAdmissionReason::UnsupportedInput)
}

pub fn is_colmap_input_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("db"))
        || path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                matches!(
                    name.to_ascii_lowercase().as_str(),
                    "cameras.bin"
                        | "images.bin"
                        | "points3d.bin"
                        | "rigs.bin"
                        | "frames.bin"
                        | "cameras.txt"
                        | "images.txt"
                        | "points3d.txt"
                        | "rigs.txt"
                        | "frames.txt"
                )
            })
}

fn directory_has_marker(path: &Path) -> Result<bool> {
    for name in [
        "cameras.bin",
        "images.bin",
        "points3D.bin",
        "rigs.bin",
        "frames.bin",
        "cameras.txt",
        "images.txt",
        "points3D.txt",
        "rigs.txt",
        "frames.txt",
        "database.db",
    ] {
        match std::fs::symlink_metadata(path.join(name)) {
            Ok(metadata) if metadata.is_file() || metadata.file_type().is_symlink() => {
                return Ok(true)
            }
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(false)
}

fn child_directories(path: &Path) -> Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(vec![])
        }
        Err(error) => return Err(error.into()),
    };
    let mut children = vec![];
    for entry in entries.take(256) {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() && !kind.is_symlink() {
            children.push(entry.path());
        }
    }
    Ok(children)
}

/// Bounded discovery of known COLMAP markers, without parsing models or opening
/// databases. Missing paths are not permission failures; all other I/O survives.
pub fn path_contains_colmap_input(path: &Path) -> Result<bool> {
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_dir() {
        return Ok(metadata.is_file() && is_colmap_input_file(path));
    }
    if directory_has_marker(path)? || directory_has_marker(&path.join("sparse"))? {
        return Ok(true);
    }
    for child in child_directories(&path.join("sparse"))? {
        if directory_has_marker(&child)? {
            return Ok(true);
        }
    }
    if path
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("sparse"))
    {
        for child in child_directories(path)? {
            if directory_has_marker(&child)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Admission of a selected batch is atomic. No pictures are decoded and no
/// state is changed before every selected path is accepted.
pub fn ensure_new_paths_supported(paths: &[PathBuf]) -> Result<()> {
    let mut colmap_paths = vec![];
    let mut loose_images = 0;
    for path in paths {
        if path_contains_colmap_input(path)? {
            ensure_colmap_import_enabled()?;
            colmap_paths.push(path);
        }
    }
    for path in paths {
        if colmap_paths.contains(&path) {
            continue;
        }
        let metadata = std::fs::metadata(path)?;
        if metadata.is_dir() {
            // Preserve access/read-directory failures instead of quietly
            // treating an unreadable folder as an empty image set.
            let mut image_count = 0;
            for entry in std::fs::read_dir(path)? {
                let entry = entry?;
                if crate::video::image_sequence::is_image_file(&entry.path())
                    && entry.metadata()?.is_file()
                {
                    image_count += 1;
                    if image_count >= 2 {
                        break;
                    }
                }
            }
            if image_count < 2 {
                return Err(unsupported_input());
            }
        } else if !metadata.is_file()
            || !(crate::video::image_sequence::is_image_file(path)
                || path.extension().is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("mp4") || extension.eq_ignore_ascii_case("mov")
                })
                || (COLMAP_IMPORT_ENABLED && is_colmap_input_file(path)))
        {
            return Err(unsupported_input());
        }
        if metadata.is_file() && crate::video::image_sequence::is_image_file(path) {
            loose_images += 1;
        }
    }
    if loose_images == 1 {
        return Err(unsupported_input());
    }
    Ok(())
}

/// Call only when creating/inspecting NEW input, never when restoring a saved task.
pub fn ensure_new_inputs_supported(inputs: &GenerationInputs) -> Result<()> {
    match inputs {
        GenerationInputs::Colmap { .. } => ensure_colmap_import_enabled(),
        GenerationInputs::Materials { items } => {
            if items.is_empty() {
                return Err(unsupported_input());
            }
            if items.iter().any(
                |item| matches!(&item.source, InputSource::Images { paths } if paths.len() < 2),
            ) {
                return Err(unsupported_input());
            }
            let paths = items
                .iter()
                .flat_map(|item| match &item.source {
                    InputSource::Images { paths } => paths.clone(),
                    source => vec![source.path().to_path_buf()],
                })
                .collect::<Vec<_>>();
            ensure_new_paths_supported(&paths)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_colmap_is_rejected_before_reading_any_model() {
        let input = GenerationInputs::Colmap {
            input: super::super::inputs::ColmapInput {
                model: "missing-model".into(),
                images: "missing-images".into(),
                database: None,
                identity: None,
            },
        };
        assert!(matches!(
            ensure_new_inputs_supported(&input),
            Err(SplatError::InputAdmission(
                InputAdmissionReason::ColmapDisabled
            ))
        ));
    }

    #[test]
    fn known_files_nested_models_and_incomplete_colmap_are_disabled() {
        for relative in [
            "database.db",
            "cameras.bin",
            "sparse/0/images.txt",
            "sparse/1/rigs.bin",
        ] {
            let root = tempfile::tempdir().unwrap();
            let marker = root.path().join(relative);
            std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
            std::fs::write(&marker, []).unwrap();
            for path in [marker.as_path(), root.path()] {
                assert!(matches!(
                    ensure_new_paths_supported(&[path.to_path_buf()]),
                    Err(SplatError::InputAdmission(
                        InputAdmissionReason::ColmapDisabled
                    ))
                ));
            }
        }
    }

    #[test]
    fn media_remains_supported_and_bad_batches_are_atomic() {
        let root = tempfile::tempdir().unwrap();
        let image = root.path().join("image.JPG");
        let second_image = root.path().join("second.png");
        let video = root.path().join("video.MOV");
        std::fs::write(&image, []).unwrap();
        std::fs::write(&second_image, []).unwrap();
        std::fs::write(&video, []).unwrap();
        ensure_new_paths_supported(&[
            image.clone(),
            second_image,
            video.clone(),
            root.path().to_path_buf(),
        ])
        .unwrap();
        for name in ["unsupported.pdf", "unsupported.spp"] {
            let bad = root.path().join(name);
            std::fs::write(&bad, []).unwrap();
            assert!(matches!(
                ensure_new_paths_supported(&[video.clone(), bad]),
                Err(SplatError::InputAdmission(
                    InputAdmissionReason::UnsupportedInput
                ))
            ));
        }
        let empty = tempfile::tempdir().unwrap();
        assert!(matches!(
            ensure_new_paths_supported(&[empty.path().to_path_buf()]),
            Err(SplatError::InputAdmission(
                InputAdmissionReason::UnsupportedInput
            ))
        ));
    }

    #[test]
    fn missing_input_remains_a_real_io_error() {
        let root = tempfile::tempdir().unwrap();
        assert!(matches!(
            ensure_new_paths_supported(&[root.path().join("missing.mp4")]),
            Err(SplatError::Io(_))
        ));
    }

    #[test]
    fn new_image_sets_need_two_images_for_folders_and_loose_files() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first.jpg");
        std::fs::write(&first, []).unwrap();
        for paths in [vec![first.clone()], vec![root.path().to_path_buf()]] {
            assert!(matches!(
                ensure_new_paths_supported(&paths),
                Err(SplatError::InputAdmission(
                    InputAdmissionReason::UnsupportedInput
                ))
            ));
        }
        let item = super::super::inputs::MaterialInput {
            id: "one-image".into(),
            source: InputSource::Images {
                paths: vec![first.clone()],
            },
            identity: None,
            projection: Default::default(),
            image_matching_strategy: Default::default(),
            camera_relation: None,
            confirmed_small_images: false,
            confirmed_image_order: false,
            frame_removal_fps: None,
        };
        assert!(matches!(
            ensure_new_inputs_supported(&GenerationInputs::Materials { items: vec![item] }),
            Err(SplatError::InputAdmission(
                InputAdmissionReason::UnsupportedInput
            ))
        ));
        let second = root.path().join("second.png");
        std::fs::write(&second, []).unwrap();
        ensure_new_paths_supported(&[first, second]).unwrap();
        ensure_new_paths_supported(&[root.path().to_path_buf()]).unwrap();
    }
}
