//! Keep the initialization actually visible to Brush consistent with the saved
//! execution plan. This is not a recipe resolver: callers supply an identity
//! already proved against their snapshot, coordinate frame and saved plan.
//!
//! The caller must persist a route change before preparing this directory.
//! Only task-owned, content-identified initialization files may be archived;
//! their names alone are never evidence of ownership.

use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    error::{Result, SplatError},
    reconstruction::{ply::PlyInfo, splat_transform::validate_training_ply},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GaussianIdentity {
    pub sha256: String,
    pub splat_count: u64,
    pub sh_degree: u32,
}

impl GaussianIdentity {
    pub fn from_validated(info: &PlyInfo, sh_degree: u32, sha256: String) -> Self {
        Self {
            sha256,
            splat_count: info.splat_count,
            sh_degree,
        }
    }

    fn valid(&self) -> bool {
        self.splat_count > 0
            && self.sh_degree <= 3
            && self.sha256.len() == 64
            && self.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum InitializationContract {
    SparsePoints,
    GaussianSnapshot {
        identity: GaussianIdentity,
        /// Supplied only after the caller verifies the frozen source and any
        /// applied coordinate transform. It is deliberately not guessed here.
        coordinate_frame_id: String,
    },
}

fn stopped(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(SplatError::Cancelled)
    } else {
        Ok(())
    }
}

fn invalid(detail: &str) -> SplatError {
    SplatError::Process(format!("Brush initialization contract: {detail}"))
}

fn io_context(error: SplatError, substep: &str) -> SplatError {
    error.with_io_diagnostic(format!(
        "training_preparation_operation=prepare_initialization\npath_role=training_initialization\nio_substep={substep}"
    ))
}

fn redirected(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_type().is_symlink()
            || windows_attributes_redirected(metadata.file_attributes())
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

#[cfg(windows)]
fn windows_attributes_redirected(attributes: u32) -> bool {
    attributes & 0x400 != 0
}

/// Validate every path component before canonicalization can erase evidence of
/// a junction/symlink. Both endpoints are internal, absolute project paths.
pub(super) fn check_dataset_boundary(project_root: &Path, directory: &Path) -> Result<()> {
    if !project_root.is_absolute() || !directory.is_absolute() {
        return Err(invalid("project and dataset must use absolute paths"));
    }
    let relative = directory
        .strip_prefix(project_root)
        .map_err(|_| invalid("dataset is outside its owning project"))?;
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(invalid("invalid dataset boundary"));
    }
    let mut current = project_root.to_path_buf();
    checked_directory(&current)?;
    for part in relative.components() {
        current.push(part.as_os_str());
        checked_directory(&current)?;
    }
    let root = fs::canonicalize(project_root)?;
    let actual = fs::canonicalize(directory)?;
    if actual == root || !actual.starts_with(root) {
        return Err(invalid("dataset boundary changed"));
    }
    Ok(())
}

fn checked_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if redirected(&metadata) || !metadata.is_dir() {
        return Err(invalid("redirected or invalid dataset directory"));
    }
    Ok(())
}

fn validate_contract(contract: &InitializationContract) -> Result<()> {
    if let InitializationContract::GaussianSnapshot {
        identity,
        coordinate_frame_id,
    } = contract
    {
        if !identity.valid() || coordinate_frame_id.trim().is_empty() {
            return Err(invalid("initialization identity is incomplete"));
        }
    }
    Ok(())
}

fn is_ply(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("ply"))
}

fn known_initialization_name(dataset: &Path, file: &Path) -> bool {
    if file.parent() != Some(dataset) {
        return false;
    }
    let Some(name) = file.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if name == "init.ply" {
        return true;
    }
    name.strip_prefix("init.warm-")
        .and_then(|name| name.strip_suffix(".ply"))
        .is_some_and(|id| Uuid::parse_str(id).is_ok_and(|uuid| uuid.to_string() == id))
}

/// Match Brush's recursively mounted input, including unexpected nested PLYs.
/// Do not decode or hash the ordinary training images while walking it.
fn dataset_initializations(
    project_root: &Path,
    dataset: &Path,
    cancel: &CancellationToken,
) -> Result<Vec<(PathBuf, GaussianIdentity)>> {
    stopped(cancel)?;
    check_dataset_boundary(project_root, dataset)?;
    let mut pending = vec![dataset.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        stopped(cancel)?;
        checked_directory(&directory)?;
        for entry in fs::read_dir(directory)? {
            stopped(cancel)?;
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;
            if redirected(&metadata) {
                return Err(invalid("redirected entry in training dataset"));
            }
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if !metadata.is_file() {
                return Err(invalid("unsupported entry in training dataset"));
            } else if is_ply(&entry.path()) {
                if !known_initialization_name(dataset, &entry.path()) {
                    return Err(invalid("unrecognized PLY in training dataset"));
                }
                files.push(entry.path());
            }
        }
    }
    files.sort();
    files
        .into_iter()
        .map(|path| {
            stopped(cancel)?;
            let metadata = fs::symlink_metadata(&path)?;
            if redirected(&metadata) || !metadata.is_file() {
                return Err(invalid("initialization entry changed"));
            }
            let (info, sh, hash) = validate_training_ply(&path, cancel)?;
            Ok((path, GaussianIdentity::from_validated(&info, sh, hash)))
        })
        .collect()
}

fn preparation_plan(
    project_root: &Path,
    dataset: &Path,
    contract: &InitializationContract,
    owned_initializations: &[GaussianIdentity],
    cancel: &CancellationToken,
) -> Result<Vec<(PathBuf, GaussianIdentity)>> {
    validate_contract(contract)?;
    let files = dataset_initializations(project_root, dataset, cancel)?;
    let expected = match contract {
        InitializationContract::SparsePoints => None,
        InitializationContract::GaussianSnapshot { identity, .. } => Some(identity),
    };
    let mut retained = false;
    let mut archive = Vec::new();
    for (path, identity) in files {
        if path == dataset.join("init.ply") && expected == Some(&identity) {
            retained = true;
            continue;
        }
        if !identity.valid()
            || !(owned_initializations.contains(&identity) || expected == Some(&identity))
        {
            return Err(invalid("initialization file ownership is unproven"));
        }
        archive.push((path, identity));
    }
    if expected.is_some() && !retained {
        return Err(invalid("trusted initialization is missing or changed"));
    }
    Ok(archive)
}

/// Only call after the route/attempt has been durably saved. Scan and validate
/// the whole batch before moving anything. A partial rename is recoverable:
/// subsequent calls see only the remaining dataset entries; archived assets
/// are outside Brush's VFS and never removed or copied over another file.
pub(super) async fn prepare_owned_dataset(
    project_root: &Path,
    dataset: &Path,
    contract: &InitializationContract,
    owned_initializations: &[GaussianIdentity],
    cancel: &CancellationToken,
) -> Result<()> {
    let root = project_root.to_path_buf();
    let input = dataset.to_path_buf();
    let mode = contract.clone();
    let known = owned_initializations.to_vec();
    let token = cancel.clone();
    let archive = crate::presets::spawn_pipeline_blocking(move || {
        preparation_plan(&root, &input, &mode, &known, &token)
    })
    .await
    .map_err(|_| invalid("initialization inspection worker failed"))?
    .map_err(|error| io_context(error, "validate_owned_files"))?;
    // No mutation took place. The plan already verified the complete input;
    // avoid immediately hashing a large warm initialization a second time.
    // Actual process launch still performs its own mandatory read-only check.
    if archive.is_empty() {
        return stopped(cancel);
    }
    stopped(cancel)?;
    let work = project_root.join("work");
    let archive_root = work.join("initialization-archive");
    if archive_root.starts_with(dataset) {
        return Err(invalid("archive must be outside the training dataset"));
    }
    for directory in [&work, &archive_root] {
        match tokio::fs::create_dir(directory).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(io_context(error.into(), "create_initialization_archive"));
            }
        }
        checked_directory(directory)?;
    }
    let destination = archive_root.join(Uuid::new_v4().to_string());
    tokio::fs::create_dir(&destination)
        .await
        .map_err(SplatError::from)
        .map_err(|error| io_context(error, "create_initialization_archive"))?;
    for (source, expected) in archive {
        stopped(cancel)?;
        let root = project_root.to_path_buf();
        let input = dataset.to_path_buf();
        let source_copy = source.clone();
        let token = cancel.clone();
        crate::presets::spawn_pipeline_blocking(move || {
            check_dataset_boundary(&root, &input)?;
            let metadata = fs::symlink_metadata(&source_copy)?;
            if redirected(&metadata) || !metadata.is_file() {
                return Err(invalid("initialization entry changed before archive"));
            }
            let (info, sh, hash) = validate_training_ply(&source_copy, &token)?;
            if GaussianIdentity::from_validated(&info, sh, hash) != expected {
                return Err(invalid("initialization content changed before archive"));
            }
            Ok(())
        })
        .await
        .map_err(|_| invalid("initialization inspection worker failed"))?
        .map_err(|error| io_context(error, "revalidate_owned_file"))?;
        check_dataset_boundary(project_root, &destination)?;
        let target = destination.join(
            source
                .file_name()
                .ok_or_else(|| invalid("invalid initialization filename"))?,
        );
        match tokio::fs::symlink_metadata(&target).await {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_context(error.into(), "inspect_archive_target")),
            Ok(_) => return Err(invalid("initialization archive target already exists")),
        }
        super::runner::rename_initialization_entry(&source, &target, cancel)
            .await
            .map_err(|error| io_context(error, "archive_owned_file"))?;
    }
    let root = project_root.to_path_buf();
    let input = dataset.to_path_buf();
    let mode = contract.clone();
    let token = cancel.clone();
    crate::presets::spawn_pipeline_blocking(move || {
        verify_before_start(&root, &input, &mode, &token)
    })
    .await
    .map_err(|_| invalid("initialization inspection worker failed"))?
}

/// Used before copying a fresh trusted warm initialization. It never creates
/// or removes the source snapshot; all recognized prior inputs remain archived.
pub(super) async fn archive_owned_initializations(
    project_root: &Path,
    dataset: &Path,
    owned_initializations: &[GaussianIdentity],
    cancel: &CancellationToken,
) -> Result<()> {
    prepare_owned_dataset(
        project_root,
        dataset,
        &InitializationContract::SparsePoints,
        owned_initializations,
        cancel,
    )
    .await
}

/// Read-only admission immediately before each actual Brush process, including
/// OOM attempts. Run on the existing blocking pool, not the Tokio worker.
pub(super) fn verify_before_start(
    project_root: &Path,
    dataset: &Path,
    contract: &InitializationContract,
    cancel: &CancellationToken,
) -> Result<()> {
    validate_contract(contract)?;
    let files = dataset_initializations(project_root, dataset, cancel)
        .map_err(|error| io_context(error, "verify_before_start"))?;
    match contract {
        InitializationContract::SparsePoints if files.is_empty() => Ok(()),
        InitializationContract::GaussianSnapshot { identity, .. }
            if files.len() == 1
                && files[0].0 == dataset.join("init.ply")
                && &files[0].1 == identity =>
        {
            Ok(())
        }
        _ => Err(invalid(
            "visible initialization does not match the saved route",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let dataset = root.path().join("work/brush/dataset");
        fs::create_dir_all(dataset.join("sparse/0")).unwrap();
        fs::create_dir(dataset.join("images")).unwrap();
        fs::write(dataset.join("sparse/0/points3D.bin"), [1, 2, 3]).unwrap();
        fs::write(
            dataset.join("images/frame.png"),
            b"image bytes are not decoded",
        )
        .unwrap();
        (root, dataset)
    }

    fn gaussian(path: &Path, x: f32) -> GaussianIdentity {
        let properties = [
            "x", "y", "z", "f_dc_0", "f_dc_1", "f_dc_2", "opacity", "scale_0", "scale_1",
            "scale_2", "rot_0", "rot_1", "rot_2", "rot_3",
        ];
        let mut bytes = b"ply\nformat binary_little_endian 1.0\nelement vertex 1\n".to_vec();
        for property in properties {
            bytes.extend_from_slice(format!("property float {property}\n").as_bytes());
        }
        bytes.extend_from_slice(b"end_header\n");
        for value in [x, 0., 0., 0.1, 0.2, 0.3, 1., 0., 0., 0., 1., 0., 0., 0.] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        fs::write(path, bytes).unwrap();
        let (info, sh, hash) = validate_training_ply(path, &CancellationToken::new()).unwrap();
        GaussianIdentity::from_validated(&info, sh, hash)
    }

    fn warm(identity: GaussianIdentity) -> InitializationContract {
        InitializationContract::GaussianSnapshot {
            identity,
            coordinate_frame_id: "verified-source-frame".into(),
        }
    }

    fn archived(root: &Path) -> Vec<PathBuf> {
        let directory = root.join("work/initialization-archive");
        if !directory.exists() {
            return vec![];
        }
        fs::read_dir(directory)
            .unwrap()
            .flat_map(|entry| {
                fs::read_dir(entry.unwrap().path())
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    #[tokio::test]
    async fn cold_archives_identified_init_and_legacy_aliases_outside_mounted_dataset() {
        let (root, dataset) = project();
        let init = dataset.join("init.ply");
        let identity = gaussian(&init, 1.);
        let original = fs::read(&init).unwrap();
        let alias = dataset.join(format!("init.warm-{}.ply", Uuid::new_v4()));
        fs::copy(&init, &alias).unwrap();
        let cancel = CancellationToken::new();
        prepare_owned_dataset(
            root.path(),
            &dataset,
            &InitializationContract::SparsePoints,
            std::slice::from_ref(&identity),
            &cancel,
        )
        .await
        .unwrap();
        assert!(!init.exists());
        assert!(!alias.exists());
        let saved = archived(root.path());
        assert_eq!(saved.len(), 2);
        for path in &saved {
            assert!(!path.starts_with(&dataset));
            assert_eq!(fs::read(path).unwrap(), original);
        }
        assert_eq!(
            fs::read(dataset.join("sparse/0/points3D.bin")).unwrap(),
            [1, 2, 3]
        );
        prepare_owned_dataset(
            root.path(),
            &dataset,
            &InitializationContract::SparsePoints,
            &[identity],
            &cancel,
        )
        .await
        .unwrap();
        assert_eq!(archived(root.path()).len(), 2);
    }

    #[tokio::test]
    async fn warm_retains_exact_trusted_init_and_archives_only_identified_alias() {
        let (root, dataset) = project();
        let init = dataset.join("init.ply");
        let identity = gaussian(&init, 1.);
        let original = fs::read(&init).unwrap();
        let alias = dataset.join(format!("init.warm-{}.ply", Uuid::new_v4()));
        fs::copy(&init, &alias).unwrap();
        let contract = warm(identity);
        let cancel = CancellationToken::new();
        prepare_owned_dataset(root.path(), &dataset, &contract, &[], &cancel)
            .await
            .unwrap();
        verify_before_start(root.path(), &dataset, &contract, &cancel).unwrap();
        assert_eq!(fs::read(init).unwrap(), original);
        assert!(!alias.exists());
        assert_eq!(archived(root.path()).len(), 1);
    }

    #[tokio::test]
    async fn recognized_filename_does_not_prove_ownership_and_batch_moves_nothing() {
        let (root, dataset) = project();
        let init = dataset.join("init.ply");
        let known = gaussian(&init, 1.);
        let alias = dataset.join(format!("init.warm-{}.ply", Uuid::new_v4()));
        gaussian(&alias, 2.);
        let result = archive_owned_initializations(
            root.path(),
            &dataset,
            &[known],
            &CancellationToken::new(),
        )
        .await;
        assert!(result.is_err());
        assert!(init.is_file());
        assert!(alias.is_file());
        assert!(archived(root.path()).is_empty());
    }

    #[test]
    fn each_launch_rechecks_route_hash_point_count_sh_and_coordinate_evidence() {
        let (root, dataset) = project();
        let init = dataset.join("init.ply");
        let identity = gaussian(&init, 1.);
        let cancel = CancellationToken::new();
        let contract = warm(identity.clone());
        verify_before_start(root.path(), &dataset, &contract, &cancel).unwrap();
        assert!(verify_before_start(
            root.path(),
            &dataset,
            &InitializationContract::SparsePoints,
            &cancel
        )
        .is_err());
        let mut wrong = identity.clone();
        wrong.splat_count += 1;
        assert!(verify_before_start(root.path(), &dataset, &warm(wrong), &cancel).is_err());
        let mut wrong = identity.clone();
        wrong.sh_degree = 1;
        assert!(verify_before_start(root.path(), &dataset, &warm(wrong), &cancel).is_err());
        let missing_frame = InitializationContract::GaussianSnapshot {
            identity,
            coordinate_frame_id: String::new(),
        };
        assert!(verify_before_start(root.path(), &dataset, &missing_frame, &cancel).is_err());
        gaussian(&init, 2.);
        assert!(verify_before_start(root.path(), &dataset, &contract, &cancel).is_err());
    }

    #[tokio::test]
    async fn nested_unknown_and_malformed_names_are_never_moved() {
        for name in [
            "images/unexpected.PLY",
            "init.warm-not-a-uuid.ply",
            "other.ply",
        ] {
            let (root, dataset) = project();
            let path = dataset.join(name);
            let identity = gaussian(&path, 1.);
            assert!(archive_owned_initializations(
                root.path(),
                &dataset,
                &[identity],
                &CancellationToken::new()
            )
            .await
            .is_err());
            assert!(path.is_file());
            assert!(archived(root.path()).is_empty());
        }
    }

    #[tokio::test]
    async fn cancellation_and_blocked_archive_preserve_initialization() {
        let (root, dataset) = project();
        let init = dataset.join("init.ply");
        let identity = gaussian(&init, 1.);
        let token = CancellationToken::new();
        token.cancel();
        assert!(matches!(
            archive_owned_initializations(
                root.path(),
                &dataset,
                std::slice::from_ref(&identity),
                &token
            )
            .await,
            Err(SplatError::Cancelled)
        ));
        assert!(init.exists());
        fs::write(
            root.path().join("work/initialization-archive"),
            b"existing file",
        )
        .unwrap();
        assert!(archive_owned_initializations(
            root.path(),
            &dataset,
            &[identity],
            &CancellationToken::new()
        )
        .await
        .is_err());
        assert!(init.exists());
    }

    #[tokio::test]
    async fn resumed_preparation_preserves_previously_archived_files() {
        let (root, dataset) = project();
        let init = dataset.join("init.ply");
        let identity = gaussian(&init, 1.);
        let alias = dataset.join(format!("init.warm-{}.ply", Uuid::new_v4()));
        fs::copy(&init, &alias).unwrap();
        let first_archive = root.path().join("work/initialization-archive/interrupted");
        fs::create_dir_all(&first_archive).unwrap();
        fs::rename(&alias, first_archive.join(alias.file_name().unwrap())).unwrap();
        archive_owned_initializations(
            root.path(),
            &dataset,
            &[identity],
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(archived(root.path()).len(), 2);
        assert!(first_archive.join(alias.file_name().unwrap()).exists());
    }

    #[test]
    fn external_and_parent_relative_datasets_are_rejected() {
        let (root, dataset) = project();
        let other = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        for directory in [
            other.path().to_path_buf(),
            dataset.join(".."),
            root.path().to_path_buf(),
        ] {
            assert!(verify_before_start(
                root.path(),
                &directory,
                &InitializationContract::SparsePoints,
                &cancel
            )
            .is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_files_and_directories_are_rejected_without_following() {
        let (root, dataset) = project();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dataset.join("linked")).unwrap();
        assert!(verify_before_start(
            root.path(),
            &dataset,
            &InitializationContract::SparsePoints,
            &CancellationToken::new()
        )
        .is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_reparse_attributes_are_always_rejected() {
        assert!(windows_attributes_redirected(0x400));
        assert!(windows_attributes_redirected(0x410));
        assert!(!windows_attributes_redirected(0x10));
        assert!(!windows_attributes_redirected(0x20));
    }

    #[cfg(windows)]
    #[test]
    fn windows_redirected_dataset_is_rejected_when_symlink_privilege_is_available() {
        let (root, dataset) = project();
        let outside = tempfile::tempdir().unwrap();
        let link = dataset.join("linked");
        if let Err(error) = std::os::windows::fs::symlink_dir(outside.path(), &link) {
            if error.raw_os_error() == Some(1314) {
                eprintln!("Windows symlink test unavailable: privilege 1314");
                return;
            }
            panic!("Cannot create test symlink: {error}");
        }
        let result = verify_before_start(
            root.path(),
            &dataset,
            &InitializationContract::SparsePoints,
            &CancellationToken::new(),
        );
        fs::remove_dir(&link).unwrap();
        assert!(result.is_err());
    }
}
