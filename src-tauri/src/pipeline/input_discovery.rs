use crate::{
    error::{Result, SplatError},
    pipeline::inputs::{ColmapInput, InputSource, MaterialInput},
    reconstruction::{colmap_model::read_registered_images, direct_input::image_path},
};
use serde::Serialize;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};
use uuid::Uuid;

const COLMAP_EXCLUSIVE: &str = "COLMAP 数据不能与其他素材一起添加，请先移除当前输入。";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ColmapModelCandidate {
    pub model: PathBuf,
    pub image_candidates: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ColmapDiscovery {
    pub models: Vec<ColmapModelCandidate>,
    pub database: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum GenerationPathSelection {
    Materials { items: Vec<MaterialInput> },
    Colmap { input: ColmapInput },
    ColmapDiscovery { discovery: ColmapDiscovery },
}

fn is_video(path: &Path) -> bool {
    path.is_file()
        && path.extension().is_some_and(|extension| {
            extension.eq_ignore_ascii_case("mp4") || extension.eq_ignore_ascii_case("mov")
        })
}

fn is_colmap_core_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    matches!(
        name.to_ascii_lowercase().as_str(),
        "cameras.bin"
            | "images.bin"
            | "points3d.bin"
            | "cameras.txt"
            | "images.txt"
            | "points3d.txt"
    )
}

fn is_complete_model(path: &Path) -> bool {
    crate::reconstruction::colmap_model::reusable_model_files(path).is_ok()
}

fn add_model_candidate(candidates: &mut BTreeSet<PathBuf>, path: &Path) {
    if path.is_dir() && is_complete_model(path) {
        if let Ok(path) = std::fs::canonicalize(path) {
            candidates.insert(path);
        }
    }
}

fn direct_child_directories(path: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(path) else {
        return vec![];
    };
    entries
        .flatten()
        .take(256)
        .filter_map(|entry| {
            let metadata = entry.file_type().ok()?;
            (metadata.is_dir() && !metadata.is_symlink()).then(|| entry.path())
        })
        .collect()
}

fn directory_has_colmap_core(path: &Path) -> bool {
    let has_core = |directory: &Path| {
        std::fs::read_dir(directory).is_ok_and(|entries| {
            entries.flatten().take(256).any(|entry| {
                entry.file_type().is_ok_and(|kind| kind.is_file())
                    && is_colmap_core_file(&entry.path())
            })
        })
    };
    if has_core(path) {
        return true;
    }
    let sparse = path.join("sparse");
    has_core(&sparse)
        || direct_child_directories(&sparse)
            .into_iter()
            .any(|child| has_core(&child))
        || (path
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("sparse"))
            && direct_child_directories(path)
                .into_iter()
                .any(|child| has_core(&child)))
}

fn model_candidates(paths: &[PathBuf]) -> BTreeSet<PathBuf> {
    let mut candidates = BTreeSet::new();
    for path in paths {
        if path.is_file() && is_colmap_core_file(path) {
            if let Some(parent) = path.parent() {
                add_model_candidate(&mut candidates, parent);
            }
            continue;
        }
        if !path.is_dir() {
            continue;
        }
        add_model_candidate(&mut candidates, path);
        let sparse = path.join("sparse");
        add_model_candidate(&mut candidates, &sparse);
        if sparse.is_dir() {
            for child in direct_child_directories(&sparse) {
                add_model_candidate(&mut candidates, &child);
            }
        }
        if path
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("sparse"))
        {
            for child in direct_child_directories(path) {
                add_model_candidate(&mut candidates, &child);
            }
        }
    }
    candidates
}

fn conventional_image_directories(model: &Path, selected: &[PathBuf]) -> BTreeSet<PathBuf> {
    let mut paths = BTreeSet::new();
    for path in selected.iter().filter(|path| path.is_dir()) {
        paths.insert(path.clone());
        paths.insert(path.join("images"));
    }
    let mut ancestor = model.parent();
    for _ in 0..3 {
        let Some(path) = ancestor else { break };
        paths.insert(path.join("images"));
        ancestor = path.parent();
    }
    paths
}

fn image_directory_matches(model: &Path, images: &Path) -> bool {
    if !images.is_dir() {
        return false;
    }
    let Ok(registered) = read_registered_images(model) else {
        return false;
    };
    registered.len() >= 2
        && registered
            .iter()
            .all(|registered| image_path(images, &registered.name).is_ok())
}

fn find_database(paths: &[PathBuf], models: &BTreeSet<PathBuf>) -> Option<PathBuf> {
    let mut databases = BTreeSet::new();
    for path in paths {
        if path.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("db"))
        {
            databases.insert(path.clone());
        }
        if path.is_dir() {
            let database = path.join("database.db");
            if database.is_file() {
                databases.insert(database);
            }
        }
    }
    for model in models {
        let mut ancestor = Some(model.as_path());
        for _ in 0..4 {
            let Some(path) = ancestor else { break };
            let database = path.join("database.db");
            if database.is_file() {
                databases.insert(database);
            }
            ancestor = path.parent();
        }
    }
    (databases.len() == 1)
        .then(|| databases.into_iter().next())
        .flatten()
}

pub fn classify_media_paths(paths: Vec<PathBuf>) -> Result<Vec<MaterialInput>> {
    let mut items = vec![];
    let mut images = vec![];
    for path in paths {
        let source = if path.is_dir() {
            Some(InputSource::ImageFolder { path })
        } else if crate::video::image_sequence::is_image_file(&path) {
            images.push(path);
            None
        } else if is_video(&path) {
            Some(InputSource::Video { path })
        } else {
            return Err(SplatError::InvalidPath(path));
        };
        if let Some(source) = source {
            items.push(MaterialInput {
                id: Uuid::new_v4().to_string(),
                source,
                identity: None,
                projection: Default::default(),
                image_matching_strategy: Default::default(),
                camera_relation: None,
                confirmed_small_images: false,
                confirmed_image_order: false,
                frame_removal_fps: None,
            });
        }
    }
    if !images.is_empty() {
        images = InputSource::Images { paths: images }.members()?;
        items.push(MaterialInput {
            id: Uuid::new_v4().to_string(),
            source: InputSource::Images { paths: images },
            identity: None,
            projection: Default::default(),
            image_matching_strategy: Default::default(),
            camera_relation: None,
            confirmed_small_images: false,
            confirmed_image_order: false,
            frame_removal_fps: None,
        });
    }
    Ok(items)
}

pub fn classify(paths: Vec<PathBuf>) -> Result<GenerationPathSelection> {
    if paths.is_empty() {
        return Ok(GenerationPathSelection::Materials { items: vec![] });
    }
    let paths = paths
        .into_iter()
        .map(std::fs::canonicalize)
        .collect::<std::io::Result<Vec<_>>>()?;
    super::input_admission::ensure_new_paths_supported(&paths)?;
    let models = model_candidates(&paths);
    let has_colmap_file = paths.iter().any(|path| {
        is_colmap_core_file(path)
            || path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("db"))
            || (path.is_dir() && directory_has_colmap_core(path))
    });
    if models.is_empty() {
        if has_colmap_file {
            return Err(SplatError::Process(
                "COLMAP 数据不完整或与图片不匹配，无法直接训练。".into(),
            ));
        }
        return Ok(GenerationPathSelection::Materials {
            items: classify_media_paths(paths)?,
        });
    }
    if paths.iter().any(|path| {
        is_video(path)
            || crate::video::image_sequence::is_image_file(path)
            || (path.is_dir()
                && !models.contains(path)
                && !models.iter().any(|model| model.starts_with(path))
                && !models
                    .iter()
                    .any(|model| image_directory_matches(model, path)))
    }) {
        return Err(SplatError::Process(COLMAP_EXCLUSIVE.into()));
    }
    let database = find_database(&paths, &models);
    let candidates = models
        .iter()
        .map(|model| ColmapModelCandidate {
            model: model.clone(),
            image_candidates: conventional_image_directories(model, &paths)
                .into_iter()
                .filter(|images| image_directory_matches(model, images))
                .filter_map(|images| std::fs::canonicalize(images).ok())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        })
        .collect::<Vec<_>>();
    if candidates.len() == 1 && candidates[0].image_candidates.len() == 1 {
        return Ok(GenerationPathSelection::Colmap {
            input: ColmapInput {
                model: candidates[0].model.clone(),
                images: candidates[0].image_candidates[0].clone(),
                database,
                identity: None,
            },
        });
    }
    Ok(GenerationPathSelection::ColmapDiscovery {
        discovery: ColmapDiscovery {
            models: candidates,
            database,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn core(root: &Path) {
        std::fs::create_dir_all(root).unwrap();
        for name in ["cameras.bin", "images.bin", "points3D.bin"] {
            std::fs::File::create(root.join(name)).unwrap();
        }
    }

    fn text_core(root: &Path, image_names: &[&str]) {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(
            root.join("cameras.txt"),
            "1 SIMPLE_RADIAL 640 480 500 320 240 0\n",
        )
        .unwrap();
        let images = image_names
            .iter()
            .enumerate()
            .map(|(index, name)| format!("{} 1 0 0 0 0 0 0 1 {}\n\n", index + 1, name))
            .collect::<String>();
        std::fs::write(root.join("images.txt"), images).unwrap();
        std::fs::write(root.join("points3D.txt"), "").unwrap();
    }

    #[test]
    fn finds_bounded_standard_model_locations() {
        let root = tempfile::tempdir().unwrap();
        core(&root.path().join("sparse/0"));
        core(&root.path().join("sparse/1"));
        let candidates = model_candidates(&[root.path().to_path_buf()]);
        assert_eq!(candidates.len(), 2);
    }

    #[test]
    fn incomplete_selected_core_file_is_not_treated_as_media() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("cameras.txt");
        writeln!(std::fs::File::create(&file).unwrap(), "# incomplete").unwrap();
        assert!(classify(vec![file]).is_err());
        assert!(classify(vec![root.path().to_path_buf()]).is_err());
    }

    #[test]
    fn rejects_new_text_model_import_without_changing_discovery_support() {
        let root = tempfile::tempdir().unwrap();
        text_core(&root.path().join("sparse/0"), &["a.jpg", "nested/b.jpg"]);
        let images = root.path().join("images");
        std::fs::create_dir_all(images.join("nested")).unwrap();
        std::fs::write(images.join("a.jpg"), []).unwrap();
        std::fs::write(images.join("nested/b.jpg"), []).unwrap();

        assert_eq!(model_candidates(&[root.path().to_path_buf()]).len(), 1);
        assert!(matches!(
            classify(vec![root.path().to_path_buf()]),
            Err(SplatError::InputAdmission(
                crate::error::InputAdmissionReason::ColmapDisabled
            ))
        ));
    }

    #[test]
    fn multiple_models_are_disabled_for_new_imports_without_removing_readers() {
        let root = tempfile::tempdir().unwrap();
        text_core(&root.path().join("sparse/0"), &["a.jpg", "b.jpg"]);
        text_core(&root.path().join("sparse/1"), &["a.jpg", "b.jpg"]);
        let images = root.path().join("images");
        std::fs::create_dir_all(&images).unwrap();
        std::fs::write(images.join("a.jpg"), []).unwrap();
        std::fs::write(images.join("b.jpg"), []).unwrap();

        assert_eq!(model_candidates(&[root.path().to_path_buf()]).len(), 2);
        assert!(matches!(
            classify(vec![root.path().to_path_buf()]),
            Err(SplatError::InputAdmission(
                crate::error::InputAdmissionReason::ColmapDisabled
            ))
        ));
    }

    #[test]
    fn database_only_and_mixed_batches_fail_with_admission_reason() {
        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("database.db");
        let video = root.path().join("video.mp4");
        std::fs::write(&database, []).unwrap();
        std::fs::write(&video, []).unwrap();
        for paths in [
            vec![database.clone()],
            vec![video.clone(), database],
            vec![root.path().to_path_buf()],
        ] {
            assert!(matches!(
                classify(paths),
                Err(SplatError::InputAdmission(
                    crate::error::InputAdmissionReason::ColmapDisabled
                ))
            ));
        }
        let GenerationPathSelection::Materials { items } = classify(vec![video]).unwrap() else {
            panic!("video should remain available")
        };
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn unsupported_files_and_empty_folders_are_not_empty_materials() {
        let root = tempfile::tempdir().unwrap();
        let bad = root.path().join("notes.pdf");
        std::fs::write(&bad, []).unwrap();
        for path in [bad, root.path().to_path_buf()] {
            assert!(matches!(
                classify(vec![path]),
                Err(SplatError::InputAdmission(
                    crate::error::InputAdmissionReason::UnsupportedInput
                ))
            ));
        }
    }
}
