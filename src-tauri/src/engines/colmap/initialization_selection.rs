//! A bounded recovery's chosen model must not lose to an invalid, larger
//! native output when a caller selects the result again. This optional local
//! record applies only to initialization recovery; unmarked outputs retain
//! their existing selection and camera policies.
use std::{
    fs::{File, Metadata},
    io::Read,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::initialization::{assess_model, ModelAssessment};
use crate::error::{Result, SplatError};

const VERSION: u32 = 1;
pub(super) const RECORD_NAME: &str = "ooosplat-initialization-selection.json";
const MODEL_FILES: [&str; 3] = ["cameras.bin", "images.bin", "points3D.bin"];
const MAX_RECORD_BYTES: u64 = 16 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Selection {
    version: u32,
    model: String,
    files: [ModelFile; 3],
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ModelFile {
    name: String,
    bytes: u64,
    sha256: String,
}

fn check_cancel(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(SplatError::Cancelled)
    } else {
        Ok(())
    }
}

fn redirected(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn check_directory(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() || redirected(&metadata) {
        return Err(SplatError::InvalidPath(path.into()));
    }
    Ok(())
}

fn model_path(root: &Path, relative: &str) -> Result<PathBuf> {
    // Recovery promotions are always direct children, never external paths,
    // parent traversal, an alternate data stream or an implicit root model.
    let mut components = Path::new(relative).components();
    if relative.is_empty()
        || relative.contains(['/', '\\', ':'])
        || !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(SplatError::Process(
            "Invalid selected Mapper initialization path".into(),
        ));
    }
    check_directory(root)?;
    let path = root.join(relative);
    check_directory(&path)?;
    Ok(path)
}

fn fingerprint(model: &Path, name: &str, cancel: &CancellationToken) -> Result<ModelFile> {
    check_cancel(cancel)?;
    let path = model.join(name);
    let metadata = std::fs::symlink_metadata(&path)?;
    if !metadata.is_file() || redirected(&metadata) {
        return Err(SplatError::InvalidPath(path));
    }
    let mut file = File::open(&path)?;
    let mut hash = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = vec![0; 128 * 1024];
    loop {
        check_cancel(cancel)?;
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes = bytes.checked_add(count as u64).ok_or_else(|| {
            SplatError::Process("Selected Mapper initialization file is too large".into())
        })?;
        hash.update(&buffer[..count]);
    }
    let after = std::fs::symlink_metadata(&path)?;
    if !after.is_file()
        || redirected(&after)
        || bytes != metadata.len()
        || bytes != after.len()
        || metadata.modified()? != after.modified()?
    {
        return Err(SplatError::Process(
            "Selected Mapper initialization changed during validation".into(),
        ));
    }
    Ok(ModelFile {
        name: name.into(),
        bytes,
        sha256: format!("{:x}", hash.finalize()),
    })
}

fn prepare(
    root: &Path,
    selected: &ModelAssessment,
    cancel: &CancellationToken,
) -> Result<Selection> {
    check_cancel(cancel)?;
    if !selected.valid_intrinsics {
        return Err(SplatError::Process(
            "Invalid Mapper initialization cannot be selected".into(),
        ));
    }
    let relative = selected
        .model
        .strip_prefix(root)
        .ok()
        .and_then(Path::to_str)
        .ok_or_else(|| SplatError::InvalidPath(selected.model.clone()))?;
    let model = model_path(root, relative)?;
    let assessment = assess_model(&model, cancel)?.ok_or_else(|| {
        SplatError::Process("Selected Mapper initialization model is empty".into())
    })?;
    if !assessment.valid_intrinsics
        || assessment.registered_images != selected.registered_images
        || assessment.points_3d != selected.points_3d
    {
        return Err(SplatError::Process(
            "Selected Mapper initialization changed before admission".into(),
        ));
    }
    Ok(Selection {
        version: VERSION,
        model: relative.into(),
        files: [
            fingerprint(&model, MODEL_FILES[0], cancel)?,
            fingerprint(&model, MODEL_FILES[1], cancel)?,
            fingerprint(&model, MODEL_FILES[2], cancel)?,
        ],
    })
}

pub(super) async fn save(
    root: &Path,
    selected: &ModelAssessment,
    cancel: &CancellationToken,
) -> Result<()> {
    let directory = root.to_path_buf();
    let selected = selected.clone();
    let token = cancel.clone();
    let record =
        crate::presets::spawn_pipeline_blocking(move || prepare(&directory, &selected, &token))
            .await
            .map_err(|error| {
                SplatError::Process(format!("Mapper selection validation failed: {error}"))
            })??;
    check_cancel(cancel)?;
    crate::project::manager::atomic_write_json(&root.join(RECORD_NAME), &record).await?;
    check_cancel(cancel)
}

/// Only absence retains the legacy largest-model policy. Invalid, incomplete
/// or stale selection evidence must never silently resurrect a rejected model.
pub(super) fn read(root: &Path) -> Result<Option<PathBuf>> {
    let path = root.join(RECORD_NAME);
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() || redirected(&metadata) || metadata.len() > MAX_RECORD_BYTES {
        return Err(SplatError::InvalidPath(path));
    }
    let mut bytes = Vec::new();
    File::open(&path)?
        .take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(SplatError::Process(
            "Mapper initialization selection record is too large".into(),
        ));
    }
    let record: Selection = serde_json::from_slice(&bytes)?;
    if record.version != VERSION {
        return Err(SplatError::Process(
            "Unsupported Mapper initialization selection version".into(),
        ));
    }
    let model = model_path(root, &record.model)?;
    let cancel = CancellationToken::new();
    for (expected, name) in record.files.iter().zip(MODEL_FILES) {
        if expected != &fingerprint(&model, name, &cancel)? {
            return Err(SplatError::Process(
                "Selected Mapper initialization identity no longer matches".into(),
            ));
        }
    }
    if !assess_model(&model, &cancel)?.is_some_and(|assessment| assessment.valid_intrinsics) {
        return Err(SplatError::Process(
            "Selected Mapper initialization is no longer valid".into(),
        ));
    }
    Ok(Some(model))
}
