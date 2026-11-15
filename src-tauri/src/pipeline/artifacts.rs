//! Dependency-aware validation for reusable generation artifacts.
//!
//! Manifests are written with durable checkpoints and compared on the next
//! run. Merely finding parseable files never promotes them to trusted caches.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    error::{Result, SplatError},
    pipeline::PipelineStage,
    project::{PipelineStateFile, ProjectPaths},
    reconstruction::{
        colmap_model::reusable_model_files,
        direct_input::{database_read_error, open_immutable_database, read_model},
        ply::inspect_gaussian_ply,
    },
};

pub const ARTIFACT_MANIFEST_VERSION: u32 = 2;
const PREPARED: &str = "prepared";
const FEATURES: &str = "features";
const MATCHING: &str = "matching";
const RECONSTRUCTION: &str = "reconstruction";
const TRAINING_DATASET: &str = "trainingDataset";
const BRUSH_CANDIDATE: &str = "brushCandidate";
const MATCHING_SEMANTIC_PREFIX: &str = "artifact-matches-v2:";

/// Called only after the shared normalizer has verified the upstream chain and
/// the durable archive barrier has completed. An invalid *derived* dataset may
/// be rebuilt without discarding the saved recipe or granting new retries.
pub(super) fn validated_training_dataset_rebuild(state: &PipelineStateFile) -> bool {
    let Some(checkpoint) = state.inputs.as_ref() else {
        return false;
    };
    let manifests = &state.artifact_manifests;
    if manifests.version != ARTIFACT_MANIFEST_VERSION
        || !checkpoint.prepared
        || !state.reconstruction_complete
        || state.brush_complete
        || checkpoint.reference_image.is_some()
        || !state.retry_cache_pending.is_empty()
        || manifests.entries.contains_key(TRAINING_DATASET)
    {
        return false;
    }
    let Some(prepared) = manifests.entries.get(PREPARED) else {
        return false;
    };
    let Some(reconstruction) = manifests.entries.get(RECONSTRUCTION) else {
        return false;
    };
    if checkpoint.imported_reconstruction {
        return reconstruction.upstream_identity == prepared.artifact_identity;
    }
    match (
        manifests.entries.get(FEATURES),
        manifests.entries.get(MATCHING),
    ) {
        (Some(features), Some(matching)) => {
            state.features_complete
                && state.matching_complete
                && features.upstream_identity == prepared.artifact_identity
                && matching.upstream_identity == features.artifact_identity
                && reconstruction.upstream_identity == matching.artifact_identity
        }
        _ => false,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactManifestSet {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub entries: BTreeMap<String, ArtifactManifest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactManifest {
    /// Diagnostic ownership only. A new run does not change artifact identity.
    #[serde(default)]
    pub attempt_id: String,
    pub upstream_identity: String,
    pub expected_count: u64,
    #[serde(default)]
    pub semantic_identity: Option<String>,
    #[serde(default)]
    pub artifact_identity: String,
    pub files: Vec<ArtifactFile>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArtifactFileType {
    #[default]
    RegularFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactFile {
    pub relative_path: String,
    #[serde(default)]
    pub file_type: ArtifactFileType,
    pub file_size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_nanos: Option<u128>,
    /// Databases, sparse models, immutable snapshot manifests and Brush
    /// candidates are hashed in addition to their filesystem identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// Ephemeral, per-executor evidence. This is deliberately not populated from
/// persisted manifests: a fresh run must deeply validate its own baseline.
#[derive(Clone, Default)]
pub struct ArtifactRecorder {
    cache: Arc<Mutex<CaptureCache>>,
}

/// The writer's actual output boundary, not the UI's current parent stage.
/// Bridge and GPS can run matching under a reconstruction parent stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ColmapWrite {
    Database,
    Reconstruction,
    TrainingDataset,
}

impl ArtifactRecorder {
    pub async fn record(
        &self,
        paths: &ProjectPaths,
        state: &mut PipelineStateFile,
        run_id: Option<Uuid>,
        cancel: &CancellationToken,
    ) -> Result<()> {
        check_cancel(cancel)?;
        let paths = paths.clone();
        let captured_state = Box::new(state.clone());
        let cache = self.cache.clone();
        let worker_cancel = cancel.clone();
        let manifests = crate::presets::spawn_pipeline_blocking(move || {
            check_cancel(&worker_cancel)?;
            let mut cache = cache.lock().unwrap_or_else(|p| p.into_inner());
            cache.bind(&paths, run_id);
            capture_with_cache(&paths, &captured_state, run_id, &mut cache, &worker_cancel)
        })
        .await
        .map_err(|error| SplatError::Process(format!("Artifact capture failed: {error}")))??;
        check_cancel(cancel)?;
        state.artifact_manifests = manifests;
        Ok(())
    }

    /// Call before operations that may modify a database or another upstream
    /// asset. File-identity checks are an additional safeguard, not a substitute
    /// for invalidating known writes (which may preserve timestamps).
    pub fn invalidate(&self) {
        *self.cache.lock().unwrap_or_else(|p| p.into_inner()) = CaptureCache::default();
    }

    /// Keep evidence for immutable upstream assets within this invocation.
    /// Every retained entry still checks file identities before reuse. A fresh
    /// recorder/run and explicit unknown writers always start without evidence.
    pub(super) fn invalidate_colmap(&self, write: ColmapWrite) {
        let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        if write == ColmapWrite::Database {
            cache.database = None;
        }
        cache.stages.retain(|stage, _| match write {
            ColmapWrite::Database | ColmapWrite::Reconstruction => *stage == PREPARED,
            ColmapWrite::TrainingDataset => matches!(*stage, PREPARED | RECONSTRUCTION),
        });
    }

    #[cfg(test)]
    pub(super) fn capture_counts(&self) -> (BTreeMap<&'static str, u64>, u64) {
        let cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        (cache.stage_captures.clone(), cache.database_captures)
    }
}

#[derive(Default)]
struct CaptureCache {
    owner: Option<(PathBuf, Option<Uuid>)>,
    stages: BTreeMap<&'static str, CachedStage>,
    database: Option<CachedDatabase>,
    #[cfg(test)]
    stage_captures: BTreeMap<&'static str, u64>,
    #[cfg(test)]
    database_captures: u64,
}

struct CachedStage {
    key: Vec<u8>,
    stamp: Vec<FileStamp>,
    manifest: ArtifactManifest,
}

struct CachedDatabase {
    stamp: Vec<FileStamp>,
    summary: DatabaseSummary,
    file: ArtifactFile,
}

#[derive(Debug, PartialEq, Eq)]
struct FileStamp {
    path: PathBuf,
    canonical_path: PathBuf,
    directory: bool,
    length: u64,
    modified: std::time::SystemTime,
    /// A same-path replacement or an in-place hard-link write must not reuse
    /// a live-run validation merely because size/mtime were restored.
    change_identity: Option<FileChangeIdentity>,
}

#[derive(Debug, PartialEq, Eq)]
struct FileChangeIdentity {
    device: u64,
    file: u64,
    change_time: (i64, i64),
    #[cfg(windows)]
    usn: i64,
}

impl CaptureCache {
    fn bind(&mut self, paths: &ProjectPaths, run_id: Option<Uuid>) {
        let owner = (paths.project.clone(), run_id);
        if self.owner.as_ref() != Some(&owner) {
            *self = Self::default();
            self.owner = Some(owner);
        }
    }

    fn stage(
        &mut self,
        stage: &'static str,
        key: Vec<u8>,
        roots: &[PathBuf],
        cancel: &CancellationToken,
        capture: impl FnOnce() -> Result<ArtifactManifest>,
    ) -> Result<ArtifactManifest> {
        let stamp = file_stamps(roots, cancel)?;
        if let Some(cached) = self.stages.get(stage) {
            if cached.key == key && reliable_stamps(&stamp) && cached.stamp == stamp {
                return Ok(cached.manifest.clone());
            }
        }
        check_cancel(cancel)?;
        let manifest = capture()?;
        check_cancel(cancel)?;
        if file_stamps(roots, cancel)? != stamp {
            self.stages.remove(stage);
            return Err(untrusted_artifact());
        }
        #[cfg(test)]
        {
            *self.stage_captures.entry(stage).or_default() += 1;
        }
        self.stages.insert(
            stage,
            CachedStage {
                key,
                stamp,
                manifest: manifest.clone(),
            },
        );
        Ok(manifest)
    }

    fn database(
        &mut self,
        paths: &ProjectPaths,
        cancel: &CancellationToken,
    ) -> Result<(DatabaseSummary, ArtifactFile)> {
        let path = paths.colmap.join("database.db");
        let stamp = database_stamps(&path, cancel)?;
        if let Some(cached) = &self.database {
            if reliable_stamps(&stamp) && cached.stamp == stamp {
                return Ok((cached.summary.clone(), cached.file.clone()));
            }
        }
        let summary = validate_database_cancellable(&path, cancel)?;
        let file = describe_file_cancellable(&paths.project, &path, true, cancel)?;
        check_cancel(cancel)?;
        if database_stamps(&path, cancel)? != stamp {
            self.database = None;
            return Err(database_in_use("COLMAP database changed during validation"));
        }
        #[cfg(test)]
        {
            self.database_captures += 1;
        }
        self.database = Some(CachedDatabase {
            stamp,
            summary: summary.clone(),
            file: file.clone(),
        });
        Ok((summary, file))
    }
}

fn check_cancel(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(SplatError::Cancelled)
    } else {
        Ok(())
    }
}

/// Include directory membership as well as size/mtime: merely stat'ing the old
/// files would miss a newly added binary model taking precedence over text.
fn file_stamps(roots: &[PathBuf], cancel: &CancellationToken) -> Result<Vec<FileStamp>> {
    let mut pending = roots.to_vec();
    let mut result = Vec::new();
    while let Some(path) = pending.pop() {
        check_cancel(cancel)?;
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let directory = metadata.file_type().is_dir();
        if !directory && !metadata.file_type().is_file() {
            return Err(SplatError::InvalidPath(path));
        }
        result.push(FileStamp {
            path: path.clone(),
            canonical_path: std::fs::canonicalize(&path)?,
            directory,
            length: metadata.len(),
            modified: metadata.modified()?,
            change_identity: file_change_identity(&path, &metadata).map_err(|error| {
                error.with_io_diagnostic(format!(
                    "artifact_validation_operation=read_change_identity\npath_role={}",
                    artifact_path_role(&path, directory)
                ))
            })?,
        });
        if directory {
            for entry in std::fs::read_dir(&path)? {
                pending.push(entry?.path());
            }
        }
    }
    result.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(result)
}

fn artifact_path_role(path: &Path, directory: bool) -> &'static str {
    let name = path.file_name().and_then(|name| name.to_str());
    if directory {
        "artifact_directory"
    } else if name == Some("database.db") {
        "colmap_database"
    } else if name.is_some_and(|name| name.ends_with("-wal") || name.ends_with("-journal")) {
        "database_sidecar"
    } else {
        "artifact_file"
    }
}

fn reliable_stamps(stamps: &[FileStamp]) -> bool {
    !stamps.is_empty() && stamps.iter().all(|stamp| stamp.change_identity.is_some())
}

fn file_change_identity(
    path: &Path,
    metadata: &std::fs::Metadata,
) -> Result<Option<FileChangeIdentity>> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = path;
        Ok((metadata.ino() != 0).then_some(FileChangeIdentity {
            device: metadata.dev(),
            file: metadata.ino(),
            change_time: (metadata.ctime(), metadata.ctime_nsec()),
        }))
    }
    #[cfg(windows)]
    {
        use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
        use windows_sys::Win32::Storage::FileSystem::{
            FileBasicInfo, GetFileInformationByHandle, GetFileInformationByHandleEx,
            BY_HANDLE_FILE_INFORMATION, FILE_BASIC_INFO, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_SHARE_DELETE,
            FILE_SHARE_READ,
        };
        use windows_sys::Win32::System::{
            Ioctl::{FSCTL_READ_FILE_USN_DATA, READ_FILE_USN_DATA},
            IO::DeviceIoControl,
        };
        let _ = metadata;
        let handle = std::fs::OpenOptions::new()
            // Attribute-only handles do not participate in Windows' data
            // sharing checks. READ_DATA excludes a currently open writer; no
            // pixel bytes are read by this short-lived metadata handle.
            .access_mode(FILE_READ_ATTRIBUTES | FILE_READ_DATA)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        let mut basic: FILE_BASIC_INFO = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(handle.as_raw_handle() as _, &mut info) } == 0 {
            let error = std::io::Error::last_os_error();
            return match error.raw_os_error() {
                Some(1 | 50 | 87) => Ok(None),
                _ => Err(error.into()),
            };
        }
        if unsafe {
            GetFileInformationByHandleEx(
                handle.as_raw_handle() as _,
                FileBasicInfo,
                (&mut basic as *mut FILE_BASIC_INFO).cast(),
                std::mem::size_of::<FILE_BASIC_INFO>() as u32,
            )
        } == 0
        {
            let error = std::io::Error::last_os_error();
            return match error.raw_os_error() {
                Some(1 | 50 | 87) => Ok(None),
                _ => Err(error.into()),
            };
        }
        // Windows change timestamps can remain identical across fast writes,
        // even after every writer closes. A journal USN supplies the change
        // sequence; a still-open writer was excluded by the handle above.
        let request = READ_FILE_USN_DATA {
            MinMajorVersion: 2,
            MaxMajorVersion: 2,
        };
        let mut buffer = [0_u8; 4096];
        let mut returned = 0_u32;
        if unsafe {
            DeviceIoControl(
                handle.as_raw_handle() as _,
                FSCTL_READ_FILE_USN_DATA,
                (&request as *const READ_FILE_USN_DATA).cast(),
                std::mem::size_of::<READ_FILE_USN_DATA>() as u32,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        } == 0
        {
            let error = std::io::Error::last_os_error();
            return match error.raw_os_error() {
                // Journal support/permission is optional for this optimization.
                // File-open and genuine storage errors retain their I/O source.
                Some(1 | 5 | 38 | 50 | 87 | 1178 | 1179 | 1181) => Ok(None),
                _ => Err(error.into()),
            };
        }
        let file = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
        let Some(usn) = file_usn_record(&buffer, returned, file) else {
            return Ok(None);
        };
        Ok(
            (file != 0 && basic.ChangeTime > 0).then_some(FileChangeIdentity {
                device: u64::from(info.dwVolumeSerialNumber),
                file,
                change_time: (basic.ChangeTime, 0),
                usn,
            }),
        )
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (path, metadata);
        // A platform without a reliable change identity still validates deeply,
        // but cannot take the same-invocation shortcut.
        Ok(None)
    }
}

/// The requested USN_RECORD_V2 has a fixed prefix followed by a variable file
/// name. Parse only the versioned fixed prefix, without assuming alignment.
#[cfg(any(windows, test))]
fn file_usn_record(buffer: &[u8], returned: u32, expected_file: u64) -> Option<i64> {
    let bytes = buffer.get(..usize::try_from(returned).ok()?)?;
    if bytes.len() < 60 {
        return None;
    }
    let length = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
    let major = u16::from_le_bytes(bytes.get(4..6)?.try_into().ok()?);
    let minor = u16::from_le_bytes(bytes.get(6..8)?.try_into().ok()?);
    let file = u64::from_le_bytes(bytes.get(8..16)?.try_into().ok()?);
    let usn = i64::from_le_bytes(bytes.get(24..32)?.try_into().ok()?);
    (major == 2
        && minor == 0
        && length >= 60
        && length <= bytes.len()
        && file == expected_file
        && expected_file != 0
        && usn > 0)
        .then_some(usn)
}

fn database_in_use(message: &str) -> SplatError {
    SplatError::Io(std::io::Error::new(std::io::ErrorKind::WouldBlock, message))
}

/// A PERSIST journal remains allocated after commit, with an invalidated zero
/// header. Do not mistake this SQLite-owned, cold journal for pending writes.
fn journal_is_cold(path: &Path, length: u64) -> Result<bool> {
    if length == 0 {
        return Ok(true);
    }
    if length < 8 {
        return Ok(false);
    }
    let mut header = [0_u8; 8];
    File::open(path)?.read_exact(&mut header)?;
    Ok(header == [0; 8])
}

pub(super) fn validate_recovery_path(project: &Path, path: &Path) -> Result<()> {
    let relative = path.strip_prefix(project).map_err(|_| {
        SplatError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Unsafe COLMAP database recovery path",
        ))
    })?;
    let mut current = project.to_path_buf();
    for part in relative.components() {
        if !matches!(part, std::path::Component::Normal(_)) {
            return Err(SplatError::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Unsafe COLMAP database recovery path",
            )));
        }
        current.push(part);
        let metadata = std::fs::symlink_metadata(&current)?;
        let redirected = metadata.file_type().is_symlink();
        #[cfg(windows)]
        let redirected = {
            use std::os::windows::fs::MetadataExt;
            redirected || metadata.file_attributes() & 0x400 != 0
        };
        if redirected {
            return Err(SplatError::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Redirected COLMAP database recovery path",
            )));
        }
    }
    Ok(())
}

pub(super) fn reject_shared_recovery_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    let shared = {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path)?.nlink() > 1
    };
    #[cfg(windows)]
    let shared = {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };
        let file = File::open(path)?;
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        info.nNumberOfLinks > 1
    };
    #[cfg(not(any(unix, windows)))]
    let shared = {
        let _ = path;
        // Unknown link semantics cannot prove a mutable file is privately owned.
        true
    };
    if shared {
        return Err(SplatError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "COLMAP database recovery requires a private file, not a hard-linked source snapshot",
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WorkingDatabaseRecovery {
    NotNeeded,
    Reusable,
    RebuildRequired { reason: &'static str },
}

/// Only the current task's mutable database is eligible for recovery. The
/// caller must first resolve publication/candidate fast paths and own the run
/// lock. RebuildRequired is a request for a durable archive barrier, not
/// permission to remove individual sidecars or restart against a stale model.
pub(super) fn recover_working_database(
    paths: &ProjectPaths,
    state: &PipelineStateFile,
    cancel: &CancellationToken,
) -> Result<WorkingDatabaseRecovery> {
    check_cancel(cancel)?;
    if state
        .inputs
        .as_ref()
        .is_some_and(|value| value.imported_reconstruction)
    {
        return Ok(WorkingDatabaseRecovery::NotNeeded);
    }
    let path = paths.colmap.join("database.db");
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(WorkingDatabaseRecovery::NotNeeded)
        }
        Err(error) => return Err(error.into()),
    };
    validate_recovery_path(&paths.project, &path)?;
    let root = std::fs::canonicalize(&paths.project)?;
    if !metadata.file_type().is_file() || !std::fs::canonicalize(&path)?.starts_with(&root) {
        return Err(SplatError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Unsafe COLMAP database recovery path",
        )));
    }
    let mut pending = false;
    let mut existing_sidecars = Vec::new();
    for suffix in ["-wal", "-journal", "-shm"] {
        let mut sidecar_name = path.as_os_str().to_os_string();
        sidecar_name.push(suffix);
        let sidecar = PathBuf::from(sidecar_name);
        match std::fs::symlink_metadata(&sidecar) {
            Ok(value) => {
                validate_recovery_path(&paths.project, &sidecar)?;
                if !value.file_type().is_file()
                    || !std::fs::canonicalize(&sidecar)?.starts_with(&root)
                {
                    return Err(SplatError::Io(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "Unsafe COLMAP database sidecar recovery path",
                    )));
                }
                // A cold PERSIST journal is legal, but its connection may
                // still own a live write transaction. Let SQLite prove the
                // exclusive lock instead of equating a zero header with idle.
                pending |= matches!(suffix, "-wal" | "-journal") && value.len() > 0;
                existing_sidecars.push(sidecar);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    if !pending {
        return Ok(WorkingDatabaseRecovery::NotNeeded);
    }
    reject_shared_recovery_file(&path)?;
    for sidecar in existing_sidecars {
        reject_shared_recovery_file(&sidecar)?;
    }
    // Historical tasks have no semantic manifest. An unchanged main-file hash
    // may continue through their existing strict validator; changed bytes
    // cannot be used to manufacture evidence for an old reconstructed model.
    let before = describe_file_cancellable(&paths.project, &path, true, cancel)?;
    let recovered = (|| {
        let connection = super::sqlite_recovery::settle(&path, cancel)?;
        let summary = read_database_summary(&connection, cancel)?;
        // Bind the semantic read to bytes while SQLite still owns the lock.
        // This plain read deliberately does not open Windows change-evidence
        // handles. A writer after close must not pair a stale summary with new
        // bytes in the manifest below.
        let locked_sha256 = hash_file_cancellable(&path, cancel)?;
        // Close SQLite before filesystem change-evidence handles (Windows).
        connection
            .close()
            .map_err(|(_, error)| database_error(error))?;
        database_stamps(&path, cancel)?;
        let file = describe_file_cancellable(&paths.project, &path, true, cancel)?;
        ensure_recovered_file_identity(&locked_sha256, &file)?;
        Ok::<_, SplatError>((summary, file))
    })();
    let (summary, file) = match recovered {
        Ok(value) => value,
        Err(error) if invalid_artifact_error(&error) => {
            return Ok(WorkingDatabaseRecovery::RebuildRequired {
                reason: "sqlite_recovery_invalid",
            })
        }
        Err(error) => return Err(error),
    };
    if state.features_complete {
        if let Some(saved) = state.artifact_manifests.entries.get(FEATURES) {
            if saved.semantic_identity.as_ref() != Some(&summary.feature_identity) {
                return Ok(WorkingDatabaseRecovery::RebuildRequired {
                    reason: "sqlite_recovery_features_changed",
                });
            }
        }
    }
    let matching_complete = state.matching_complete
        || state
            .reshoot
            .as_ref()
            .is_some_and(|value| value.incremental_matching_complete);
    if matching_complete {
        if let Some(saved) = state.artifact_manifests.entries.get(MATCHING) {
            let current = database_manifest_for_file(
                file,
                &saved.attempt_id,
                &saved.upstream_identity,
                &summary.matching_identity,
                summary.match_count,
            )?;
            if !matching_manifest_match(saved, &current) {
                return Ok(WorkingDatabaseRecovery::RebuildRequired {
                    reason: "sqlite_recovery_matching_changed",
                });
            }
        } else if !same_file_content(&before, &file) {
            return Ok(WorkingDatabaseRecovery::RebuildRequired {
                reason: "sqlite_recovery_legacy_identity_unproven",
            });
        }
    }
    Ok(WorkingDatabaseRecovery::Reusable)
}

fn ensure_recovered_file_identity(locked_sha256: &str, file: &ArtifactFile) -> Result<()> {
    if file.sha256.as_deref() != Some(locked_sha256) {
        return Err(database_in_use(
            "COLMAP database changed after SQLite recovery",
        ));
    }
    Ok(())
}

/// Preserve prepared pixels, recipes, source snapshots and every rescue ledger.
/// Database/model-derived outputs must move together before any extractor runs.
pub(super) fn invalidate_database_for_rebuild(state: &mut PipelineStateFile) {
    state.features_complete = false;
    state.matching_complete = false;
    state.reconstruction_complete = false;
    state.brush_complete = false;
    state.validated_output = None;
    state.stage = PipelineStage::ExtractingFeatures;
    if let Some(gps) = state.gps.as_mut() {
        gps.database_recovery_visual_only |= gps.status == "fallback";
    }
    if let Some(checkpoint) = state.inputs.as_mut() {
        checkpoint.reference_image = None;
        checkpoint.unused.clear();
        checkpoint.source_adoption.clear();
    }
    if let Some(reshoot) = state.reshoot.as_mut() {
        reshoot.supplemental_features_complete = false;
        reshoot.incremental_matching_complete = false;
        reshoot.incremental_reconstruction_complete = false;
    }
    state
        .artifact_manifests
        .entries
        .retain(|stage, _| stage == PREPARED);
    if state.database_recovery_pending.is_none() {
        state.database_recovery_pending = Some("database_cache_untrusted".into());
    }
    for name in [
        "brush",
        "colmap",
        "joint-bridge",
        "panorama-training",
        "undistorted-training",
        "ordinary-training-model",
        "sphere-training-model",
        "sphere-training-images",
    ] {
        if !state.retry_cache_pending.iter().any(|entry| entry == name) {
            state.retry_cache_pending.push(name.into());
        }
    }
}

fn database_stamps(path: &Path, cancel: &CancellationToken) -> Result<Vec<FileStamp>> {
    check_cancel(cancel)?;
    let mut roots = vec![path.to_path_buf()];
    for suffix in ["-wal", "-journal"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        let sidecar = PathBuf::from(sidecar);
        match std::fs::symlink_metadata(&sidecar) {
            Ok(metadata)
                if !metadata.file_type().is_file()
                    || (metadata.len() > 0
                        && !(suffix == "-journal"
                            && journal_is_cold(&sidecar, metadata.len())?)) =>
            {
                // Immutable SQLite would ignore pending sidecar writes. This is
                // an operational busy condition, not proof of a corrupt cache.
                return Err(database_in_use(
                    "COLMAP database has pending sidecar writes",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        roots.push(sidecar);
    }
    let stamps = match file_stamps(&roots, cancel) {
        Err(SplatError::InvalidPath(invalid)) if invalid != path => {
            return Err(database_in_use(
                "COLMAP database sidecar is not a regular file",
            ));
        }
        result => result?,
    };
    // file_stamps also observes a sidecar that appeared after the first check.
    for stamp in &stamps {
        if stamp.path != path
            && (stamp.directory
                || (stamp.length > 0
                    && !(stamp
                        .path
                        .as_os_str()
                        .to_string_lossy()
                        .ends_with("-journal")
                        && journal_is_cold(&stamp.path, stamp.length)?)))
        {
            return Err(database_in_use(
                "COLMAP database has pending sidecar writes",
            ));
        }
    }
    Ok(stamps)
}

pub fn record_collection_artifacts(
    paths: &ProjectPaths,
    state: &mut PipelineStateFile,
    run_id: Option<Uuid>,
) -> Result<()> {
    state.artifact_manifests = capture_collection_artifacts(paths, state, run_id)?;
    Ok(())
}

pub fn capture_collection_artifacts(
    paths: &ProjectPaths,
    state: &PipelineStateFile,
    run_id: Option<Uuid>,
) -> Result<ArtifactManifestSet> {
    capture_with_cache(
        paths,
        state,
        run_id,
        &mut CaptureCache::default(),
        &CancellationToken::new(),
    )
}

fn capture_with_cache(
    paths: &ProjectPaths,
    state: &PipelineStateFile,
    run_id: Option<Uuid>,
    cache: &mut CaptureCache,
    cancel: &CancellationToken,
) -> Result<ArtifactManifestSet> {
    check_cancel(cancel)?;
    // A rejected cache must be durably archived before any later weak cache
    // signature can be captured as a new trusted baseline.
    if !state.retry_cache_pending.is_empty() {
        return Err(untrusted_artifact());
    }
    let Some(checkpoint) = state.inputs.as_ref() else {
        return Ok(ArtifactManifestSet {
            version: ARTIFACT_MANIFEST_VERSION,
            entries: BTreeMap::new(),
        });
    };
    let attempt = attempt_label(run_id);
    let source_identity = source_identity(state, checkpoint)?;
    let mut entries = BTreeMap::new();
    let mut prepared_identity = None;
    if checkpoint.prepared {
        let names = checkpoint
            .sources
            .iter()
            .map(|source| (&source.id, &source.names))
            .collect::<Vec<_>>();
        let mut roots = vec![
            paths.frames.clone(),
            paths.colmap_frames.clone(),
            paths.masks.clone(),
        ];
        if state.reshoot.is_some() {
            roots.extend([
                paths.work.join("reshoot-snapshot-v3"),
                paths.colmap.join("base-database.db"),
                paths.colmap.join("base-model"),
            ]);
        }
        let key = serde_json::to_vec(&(&source_identity, names))?;
        let manifest = cache.stage(PREPARED, key, &roots, cancel, || {
            capture_prepared_cancellable(
                paths,
                state,
                checkpoint,
                &attempt,
                &source_identity,
                cancel,
            )
        })?;
        prepared_identity = Some(manifest.artifact_identity.clone());
        entries.insert(PREPARED.into(), manifest);
    }

    let mut database = None;
    let mut feature_identity = None;
    if state.features_complete {
        let prepared = prepared_identity
            .as_deref()
            .ok_or_else(untrusted_artifact)?;
        let (summary, file) = cache.database(paths, cancel)?;
        let manifest = database_manifest_for_file(
            file,
            &attempt,
            prepared,
            &summary.feature_identity,
            summary.images.len() as u64,
        )?;
        feature_identity = Some(manifest.artifact_identity.clone());
        entries.insert(FEATURES.into(), manifest);
        database = Some((summary, entries[FEATURES].files[0].clone()));
    }

    let mut matching_identity = None;
    if state.matching_complete {
        let feature = feature_identity.as_deref().ok_or_else(untrusted_artifact)?;
        if database.is_none() {
            database = Some(cache.database(paths, cancel)?);
        }
        let (summary, file) = database.as_ref().expect("database was assigned");
        let manifest = database_manifest_for_file(
            file.clone(),
            &attempt,
            feature,
            &summary.matching_identity,
            summary.match_count,
        )?;
        matching_identity = Some(manifest.artifact_identity.clone());
        entries.insert(MATCHING.into(), manifest);
    }

    let mut reconstruction_identity = None;
    if state.reconstruction_complete {
        let upstream = if checkpoint.imported_reconstruction {
            prepared_identity.as_deref()
        } else {
            matching_identity.as_deref()
        }
        .ok_or_else(untrusted_artifact)?;
        let manifest = cache.stage(
            RECONSTRUCTION,
            serde_json::to_vec(&upstream)?,
            &[paths.colmap.join("authoritative")],
            cancel,
            || {
                capture_reconstruction_cancellable(
                    paths,
                    &attempt,
                    upstream,
                    database.as_ref().map(|(summary, _)| summary),
                    cancel,
                )
            },
        )?;
        reconstruction_identity = Some(manifest.artifact_identity.clone());
        entries.insert(RECONSTRUCTION.into(), manifest);
    }

    let mut dataset_identity = None;
    if checkpoint.reference_image.is_some() || state.brush_complete {
        let upstream = reconstruction_identity
            .as_deref()
            .ok_or_else(untrusted_artifact)?;
        let root = paths.brush.join("dataset");
        let manifest = cache.stage(
            TRAINING_DATASET,
            serde_json::to_vec(&upstream)?,
            &[
                root.join("inputs-dataset.json"),
                root.join("images"),
                root.join("sparse"),
            ],
            cancel,
            || capture_training_dataset_cancellable(paths, &attempt, upstream, cancel),
        )?;
        dataset_identity = Some(manifest.artifact_identity.clone());
        entries.insert(TRAINING_DATASET.into(), manifest);
    }

    if state.brush_complete {
        let upstream = dataset_identity.as_deref().ok_or_else(untrusted_artifact)?;
        let training_attempt = state
            .reshoot
            .as_ref()
            .and_then(|value| value.recipe.as_ref())
            .map_or(0, |recipe| recipe.training_fallbacks);
        let root = paths
            .brush
            .join(format!("collection-training-{training_attempt}"));
        let manifest = cache.stage(
            BRUSH_CANDIDATE,
            serde_json::to_vec(&(
                upstream,
                training_attempt,
                candidate_recipe_identity(state)?,
            ))?,
            &[root.join("final.ply.tmp"), root.join("final.ply.tmp.ply")],
            cancel,
            || capture_candidate_cancellable(paths, state, &attempt, upstream, cancel),
        )?;
        entries.insert(BRUSH_CANDIDATE.into(), manifest);
    }
    check_cancel(cancel)?;
    Ok(ArtifactManifestSet {
        version: ARTIFACT_MANIFEST_VERSION,
        entries,
    })
}

/// Compares current artifacts with the previously saved manifests. Only the
/// first untrusted stage and its dependants are downgraded.
pub async fn normalize_collection(
    paths: &ProjectPaths,
    state: &mut PipelineStateFile,
    run_id: Option<Uuid>,
) -> Result<bool> {
    normalize_collection_cancellable(paths, state, run_id, &CancellationToken::new()).await
}

pub async fn normalize_collection_cancellable(
    paths: &ProjectPaths,
    state: &mut PipelineStateFile,
    run_id: Option<Uuid>,
    cancel: &CancellationToken,
) -> Result<bool> {
    check_cancel(cancel)?;
    let paths = paths.clone();
    let mut normalized = Box::new(state.clone());
    let worker_cancel = cancel.clone();
    let (normalized, changed) = crate::presets::spawn_pipeline_blocking(move || {
        let changed = normalize_collection_inner(&paths, &mut normalized, run_id, &worker_cancel)?;
        Ok::<_, SplatError>((normalized, changed))
    })
    .await
    .map_err(|error| SplatError::Process(format!("Artifact normalization failed: {error}")))??;
    check_cancel(cancel)?;
    *state = *normalized;
    Ok(changed)
}

/// Prove a publication candidate before touching disposable working databases.
/// Completion flags alone are not sufficient: a missing/corrupt candidate must
/// still allow normal database recovery and artifact normalization to run.
pub(super) async fn recover_collection_candidate(
    paths: &ProjectPaths,
    state: &PipelineStateFile,
    cancel: &CancellationToken,
) -> Result<Option<PathBuf>> {
    let paths = paths.clone();
    let saved = Box::new(state.clone());
    let cancel = cancel.clone();
    crate::presets::spawn_pipeline_blocking(move || {
        verified_collection_candidate(&paths, &saved, &cancel)
    })
    .await
    .map_err(|error| SplatError::Process(error.to_string()))?
}

fn verified_collection_candidate(
    paths: &ProjectPaths,
    state: &PipelineStateFile,
    cancel: &CancellationToken,
) -> Result<Option<PathBuf>> {
    check_cancel(cancel)?;
    let Some(checkpoint) = state.inputs.as_ref() else {
        return Ok(None);
    };
    validate_collection_camera_policy(state, checkpoint)?;
    let previous = &state.artifact_manifests;
    if previous.version != ARTIFACT_MANIFEST_VERSION
        || !state.brush_complete
        || !state.retry_cache_pending.is_empty()
        || state.database_recovery_pending.is_some()
        || checkpoint.temporary_png_focal_pending_records.is_some()
        || state.brush_training.dataset_identity.is_none()
    {
        return Ok(None);
    }
    if let (Some(output), Some(saved), Some(dataset)) = (
        state.validated_output.as_ref(),
        previous.entries.get(BRUSH_CANDIDATE),
        previous.entries.get(TRAINING_DATASET),
    ) {
        if saved.semantic_identity.is_some() && saved.upstream_identity == dataset.artifact_identity
        {
            let current = usable_validation(
                capture_candidate_cancellable(
                    paths,
                    state,
                    &saved.attempt_id,
                    &saved.upstream_identity,
                    cancel,
                )
                .map(Some),
                cancel,
            )?;
            if let Some(current) = current.filter(|current| {
                candidate_manifest_match(saved, current)
                    && current.expected_count == output.splat_count
                    && current.files.len() == 1
                    && current.files[0].file_size == output.file_size
            }) {
                return Ok(Some(paths.project.join(&current.files[0].relative_path)));
            }
        }
    }
    Ok(None)
}

fn normalize_collection_inner(
    paths: &ProjectPaths,
    state: &mut PipelineStateFile,
    run_id: Option<Uuid>,
    cancel: &CancellationToken,
) -> Result<bool> {
    check_cancel(cancel)?;
    let Some(mut checkpoint) = state.inputs.clone() else {
        return Ok(false);
    };
    // A contradictory saved policy is not a disposable cache miss. Reject it
    // before candidate fast paths or invalidation can reinterpret old cameras.
    validate_collection_camera_policy(state, &checkpoint)?;
    let before = normalization_state(state, &checkpoint)?;
    let previous = state.artifact_manifests.clone();
    let usable_previous = previous.version == ARTIFACT_MANIFEST_VERSION;
    let attempt = attempt_label(run_id);
    // A fully validated immutable candidate is sufficient for publication.
    // Its saved source/config/recipe identity and dataset key must match, but
    // regenerable upstream images/databases no longer need to be on disk.
    if verified_collection_candidate(paths, state, cancel)?.is_some() {
        return Ok(false);
    }
    let upstream = source_identity(state, &checkpoint)?;
    let mut retained = ArtifactManifestSet {
        version: ARTIFACT_MANIFEST_VERSION,
        entries: BTreeMap::new(),
    };

    let current_prepared = usable_validation(
        checkpoint
            .prepared
            .then(|| {
                capture_prepared_cancellable(paths, state, &checkpoint, &attempt, &upstream, cancel)
            })
            .transpose(),
        cancel,
    )?;
    let prepared = usable_previous
        && current_prepared.as_ref().is_some_and(|current| {
            previous
                .entries
                .get(PREPARED)
                .is_some_and(|saved| exact_manifest_match(saved, current))
        });
    if prepared {
        retained
            .entries
            .insert(PREPARED.into(), previous.entries[PREPARED].clone());
    } else {
        invalidate_prepared(state, &mut checkpoint);
    }

    let database = usable_validation(
        prepared
            .then(|| validate_database_cancellable(&paths.colmap.join("database.db"), cancel))
            .transpose(),
        cancel,
    )?;
    // Deep recovery still reads and validates the database on every new run,
    // but features and matching refer to the same file, so hash it only once.
    let database_file = usable_validation(
        database
            .as_ref()
            .map(|_| {
                describe_file_cancellable(
                    &paths.project,
                    &paths.colmap.join("database.db"),
                    true,
                    cancel,
                )
            })
            .transpose(),
        cancel,
    )?;
    let prepared_identity = retained
        .entries
        .get(PREPARED)
        .map(|manifest| manifest.artifact_identity.as_str());
    let features_expected = state.features_complete;
    let matching_expected = state.matching_complete;
    let current_feature = usable_validation(
        match (
            state.features_complete,
            database.as_ref(),
            prepared_identity,
            database_file.as_ref(),
        ) {
            (true, Some(summary), Some(upstream), Some(file)) => Some(database_manifest_for_file(
                file.clone(),
                &attempt,
                upstream,
                &summary.feature_identity,
                summary.images.len() as u64,
            )),
            _ => None,
        }
        .transpose(),
        cancel,
    )?;
    let features = prepared
        && current_feature.as_ref().is_some_and(|current| {
            // Matching can be interrupted after modifying the shared database.
            // Features remain reusable when their actual rows are unchanged,
            // regardless of whether matching reached its completion flag.
            previous
                .entries
                .get(FEATURES)
                .is_some_and(|saved| semantic_manifest_match(saved, current))
        });
    state.features_complete &= features;
    if features {
        retained
            .entries
            .insert(FEATURES.into(), previous.entries[FEATURES].clone());
    }

    let feature_identity = retained
        .entries
        .get(FEATURES)
        .map(|manifest| manifest.artifact_identity.as_str());
    let current_matching = usable_validation(
        match (
            state.matching_complete,
            database.as_ref(),
            feature_identity,
            database_file.as_ref(),
        ) {
            (true, Some(summary), Some(upstream), Some(file)) => Some(database_manifest_for_file(
                file.clone(),
                &attempt,
                upstream,
                &summary.matching_identity,
                summary.match_count,
            )),
            _ => None,
        }
        .transpose(),
        cancel,
    )?;
    let matching = features
        && current_matching.as_ref().is_some_and(|current| {
            previous
                .entries
                .get(MATCHING)
                .is_some_and(|saved| matching_manifest_match(saved, current))
        });
    state.matching_complete &= matching;
    if matching {
        retained
            .entries
            .insert(MATCHING.into(), previous.entries[MATCHING].clone());
    }

    if prepared
        && !checkpoint.imported_reconstruction
        && ((features_expected && !features)
            || (matching_expected && !matching)
            || (database.is_none() && paths.colmap.join("database.db").try_exists()?))
    {
        // Do not let collection_features rename just database.db while leaving
        // its old WAL behind, or let a matcher reuse rejected verified pairs.
        state.inputs = Some(checkpoint);
        invalidate_database_for_rebuild(state);
        return Ok(true);
    }

    let reconstruction_upstream = if checkpoint.imported_reconstruction {
        retained.entries.get(PREPARED)
    } else {
        retained.entries.get(MATCHING)
    };
    let current_reconstruction = usable_validation(
        match (state.reconstruction_complete, reconstruction_upstream) {
            (true, Some(upstream)) => Some(capture_reconstruction_cancellable(
                paths,
                &attempt,
                &upstream.artifact_identity,
                database.as_ref(),
                cancel,
            )),
            _ => None,
        }
        .transpose(),
        cancel,
    )?;
    let reconstruction = current_reconstruction.as_ref().is_some_and(|current| {
        previous
            .entries
            .get(RECONSTRUCTION)
            .is_some_and(|saved| exact_manifest_match(saved, current))
    });
    state.reconstruction_complete &= reconstruction;
    if reconstruction {
        retained.entries.insert(
            RECONSTRUCTION.into(),
            previous.entries[RECONSTRUCTION].clone(),
        );
    }

    let dataset_expected = checkpoint.reference_image.is_some() || state.brush_complete;
    let reconstruction_identity = retained
        .entries
        .get(RECONSTRUCTION)
        .map(|manifest| manifest.artifact_identity.as_str());
    let current_dataset = usable_validation(
        match (dataset_expected, reconstruction_identity) {
            (true, Some(upstream)) => Some(capture_training_dataset_cancellable(
                paths, &attempt, upstream, cancel,
            )),
            _ => None,
        }
        .transpose(),
        cancel,
    )?;
    let dataset = !dataset_expected
        || (reconstruction
            && current_dataset.as_ref().is_some_and(|current| {
                previous
                    .entries
                    .get(TRAINING_DATASET)
                    .is_some_and(|saved| exact_manifest_match(saved, current))
            }));
    // A previously validated candidate can finish publication without its
    // disposable dataset. Only a recipe-bound manifest may prove that case;
    // legacy candidates still require a fully verified dataset.
    let saved_dataset = previous.entries.get(TRAINING_DATASET);
    let saved_candidate = previous.entries.get(BRUSH_CANDIDATE);
    let dataset_identity = saved_dataset
        .filter(|saved| reconstruction_identity == Some(saved.upstream_identity.as_str()))
        .map(|manifest| manifest.artifact_identity.as_str());
    let current_candidate = usable_validation(
        match (state.brush_complete, dataset_identity) {
            (true, Some(upstream)) => Some(capture_candidate_cancellable(
                paths, state, &attempt, upstream, cancel,
            )),
            _ => None,
        }
        .transpose(),
        cancel,
    )?;
    let candidate_matches = current_candidate.as_ref().is_some_and(|current| {
        saved_candidate.is_some_and(|saved| candidate_manifest_match(saved, current))
    });
    let independently_verified_candidate = saved_candidate
        .is_some_and(|saved| saved.semantic_identity.is_some())
        && state.retry_cache_pending.is_empty()
        && candidate_matches;
    let brush =
        reconstruction && (dataset || independently_verified_candidate) && candidate_matches;
    if dataset_expected && (dataset || brush) {
        if let Some(saved) = saved_dataset {
            retained
                .entries
                .insert(TRAINING_DATASET.into(), saved.clone());
        }
    } else if dataset_expected {
        checkpoint.reference_image = None;
        mark_training_cache_invalid(state);
    }
    state.brush_complete &= brush;
    if brush {
        retained.entries.insert(
            BRUSH_CANDIDATE.into(),
            previous.entries[BRUSH_CANDIDATE].clone(),
        );
    }
    // An unfinished or rejected candidate never resets the saved recipe or
    // consumed rescue budget. The executor validates compatibility before it
    // starts work again; normalization only determines artifact trust.

    state.stage = if !prepared {
        PipelineStage::Created
    } else if !state.features_complete && !checkpoint.imported_reconstruction {
        PipelineStage::ExtractingFeatures
    } else if !state.matching_complete && !checkpoint.imported_reconstruction {
        PipelineStage::Matching
    } else if !state.reconstruction_complete {
        PipelineStage::Reconstructing
    } else {
        PipelineStage::TrainingSplats
    };
    check_cancel(cancel)?;
    state.inputs = Some(checkpoint);
    state.artifact_manifests = retained;
    let after_checkpoint = state
        .inputs
        .as_ref()
        .expect("input checkpoint was restored");
    Ok(before != normalization_state(state, after_checkpoint)?)
}

/// Invalid artifacts downgrade the checkpoint; cancellation must never be
/// interpreted as evidence of corruption and persisted as such.
pub(super) fn usable_validation<T>(
    result: Result<Option<T>>,
    cancel: &CancellationToken,
) -> Result<Option<T>> {
    check_cancel(cancel)?;
    match result {
        Ok(value) => Ok(value),
        Err(error) if invalid_artifact_error(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

fn invalid_artifact_error(error: &SplatError) -> bool {
    match error {
        SplatError::Diagnostic { source, .. } => invalid_artifact_error(source),
        SplatError::Io(error) => matches!(
            error.kind(),
            std::io::ErrorKind::NotFound
                | std::io::ErrorKind::UnexpectedEof
                | std::io::ErrorKind::InvalidData
        ),
        SplatError::InvalidPath(_) | SplatError::Json(_) | SplatError::Process(_) => true,
        _ => false,
    }
}

fn mark_training_cache_invalid(state: &mut PipelineStateFile) {
    state.brush_complete = false;
    if !state.retry_cache_pending.iter().any(|name| name == "brush") {
        state.retry_cache_pending.push("brush".into());
    }
}

/// Compatibility normalizer for a saved Reshoot V2 recipe that cannot yet be
/// proven safe to migrate to `InputCheckpoint`. It follows the same dependency
/// rule as the collection normalizer, but validates the legacy directory names
/// in place and never promotes a missing checkpoint from files alone.
pub fn normalize_reshoot_v2(paths: &ProjectPaths, state: &mut PipelineStateFile) -> Result<bool> {
    let mut normalized = state.clone();
    let changed = normalize_reshoot_v2_inner(paths, &mut normalized)?;
    *state = normalized;
    Ok(changed)
}

fn normalize_reshoot_v2_inner(paths: &ProjectPaths, state: &mut PipelineStateFile) -> Result<bool> {
    let Some(mut checkpoint) = state.reshoot.clone() else {
        return Ok(false);
    };
    let Some(recipe) = checkpoint.recipe.clone() else {
        return Ok(false);
    };
    let before = serde_json::to_vec(&(
        &state.reshoot,
        state.brush_complete,
        &state.brush_training,
        &state.gps,
        state.stage,
    ))?;

    // The original V2 producer retained the combined database only. Never
    // manufacture a pristine base DB from it, or let that historical layout
    // bypass an existing-but-damaged dedicated base database.
    let mut combined_database = legacy_v2_combined_database(paths, state, &checkpoint, &recipe)?;
    let uses_combined_database = combined_database.is_some();
    let source_snapshot = checkpoint.source_snapshot_complete
        && (uses_combined_database
            || (valid_model_root(&paths.colmap.join("base-model"))?
                && usable_validation(
                    validate_database(&paths.colmap.join("base-database.db")).map(Some),
                    &CancellationToken::new(),
                )?
                .is_some()
                && prepared_names_exist(paths, &recipe.source_names)?));
    checkpoint.source_snapshot_complete &= source_snapshot;

    let supplemental_frames = source_snapshot
        && checkpoint.supplemental_frames_complete
        && !recipe.new_names.is_empty()
        && prepared_names_exist(paths, &recipe.new_names)?;
    checkpoint.supplemental_frames_complete &= supplemental_frames;

    let database = if supplemental_frames && uses_combined_database {
        combined_database.take()
    } else {
        usable_validation(
            supplemental_frames
                .then(|| validate_database(&paths.colmap.join("database.db")))
                .transpose(),
            &CancellationToken::new(),
        )?
    };
    let expected_names = recipe
        .source_names
        .iter()
        .chain(&recipe.new_names)
        .collect::<BTreeSet<_>>();
    let features = supplemental_frames
        && checkpoint.supplemental_features_complete
        && database.as_ref().is_some_and(|summary| {
            summary.images.keys().collect::<BTreeSet<_>>() == expected_names
        });
    checkpoint.supplemental_features_complete &= features;
    checkpoint.incremental_matching_complete &= features;

    let reconstruction_root = match recipe.actual.reconstruction {
        crate::pipeline::reshoot::ReconstructionMode::Incremental => paths.colmap.join("sparse"),
        crate::pipeline::reshoot::ReconstructionMode::Full => paths.colmap.join("sparse-full"),
    };
    let reconstruction = checkpoint.incremental_matching_complete
        && checkpoint.incremental_reconstruction_complete
        && valid_model_root(&reconstruction_root)?
        && (!uses_combined_database
            || usable_validation(
                validate_legacy_v2_models(
                    &reconstruction_root,
                    &paths.colmap.join("database.db"),
                    database.as_ref().expect("validated combined database"),
                    &expected_names,
                )
                .map(Some),
                &CancellationToken::new(),
            )?
            .is_some());
    checkpoint.incremental_reconstruction_complete &= reconstruction;
    if !reconstruction {
        state.gps = None;
        state.brush_complete = false;
    }

    if state.brush_complete {
        let attempt = paths.brush.join(format!(
            "attempt-{}-{}",
            recipe.reconstruction_fallbacks, recipe.training_fallbacks
        ));
        state.brush_complete = reconstruction && valid_brush_candidate(&attempt)?;
    }
    state.stage =
        if !checkpoint.source_snapshot_complete || !checkpoint.supplemental_frames_complete {
            PipelineStage::ExtractingFrames
        } else if !checkpoint.supplemental_features_complete {
            PipelineStage::ExtractingFeatures
        } else if !checkpoint.incremental_matching_complete {
            PipelineStage::Matching
        } else if !checkpoint.incremental_reconstruction_complete {
            PipelineStage::Reconstructing
        } else {
            PipelineStage::TrainingSplats
        };
    state.reshoot = Some(checkpoint);
    let after = serde_json::to_vec(&(
        &state.reshoot,
        state.brush_complete,
        &state.brush_training,
        &state.gps,
        state.stage,
    ))?;
    Ok(before != after)
}

/// Narrow read-only evidence for V2's historical combined database. It proves
/// the frozen source is a subset of the real feature rows, without relabelling
/// that mutable combined DB as a clean base snapshot or changing any recipe.
fn legacy_v2_combined_database(
    paths: &ProjectPaths,
    state: &PipelineStateFile,
    checkpoint: &crate::project::ReshootState,
    recipe: &crate::pipeline::reshoot::ReshootRecipe,
) -> Result<Option<DatabaseSummary>> {
    if state.inputs.is_some() || recipe.version != 2 || !checkpoint.source_snapshot_complete {
        return Ok(None);
    }
    match std::fs::symlink_metadata(paths.colmap.join("base-database.db")) {
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    usable_validation(
        (|| {
            if checkpoint.colmap_input_version != Some(crate::project::COLMAP_INPUT_VERSION) {
                return Err(untrusted_artifact());
            }
            let mut portable_names = BTreeSet::new();
            let mut source_names = BTreeSet::new();
            let mut all_names = BTreeSet::new();
            for (source, names) in [(true, &recipe.source_names), (false, &recipe.new_names)] {
                for name in names {
                    if name.is_empty()
                        || name.contains(['\\', ':', '\0', '\n', '\r'])
                        || name.chars().any(char::is_control)
                        || Path::new(name)
                            .components()
                            .any(|part| !matches!(part, std::path::Component::Normal(_)))
                        || !portable_names.insert(name.to_ascii_lowercase())
                        || !all_names.insert(name)
                    {
                        return Err(untrusted_artifact());
                    }
                    if source {
                        source_names.insert(name);
                    }
                }
            }
            if source_names.is_empty()
                || source_names.len() as u64 != checkpoint.source_image_count
                || (checkpoint.supplemental_frames_complete
                    && (recipe.new_names.is_empty()
                        || recipe.new_names.len() as u64 != checkpoint.reshoot_frame_count
                        || state
                            .frames
                            .as_ref()
                            .and_then(|frames| frames.extracted_frames)
                            != Some(all_names.len() as u64)))
            {
                return Err(untrusted_artifact());
            }
            let path = paths.colmap.join("database.db");
            // The shared validator rejects pending writers while allowing a
            // native SQLite recovery's cold PERSIST journal. A busy database
            // must not be misclassified as a corrupt legacy source snapshot.
            let summary = validate_database(&path)?;
            let database_names = summary.images.keys().collect::<BTreeSet<_>>();
            if !source_names.is_subset(&database_names)
                || !database_names.is_subset(&all_names)
                || (checkpoint.supplemental_features_complete && database_names != all_names)
            {
                return Err(untrusted_artifact());
            }
            let connection = open_immutable_database(&path)?;
            for (name, (_, camera_id)) in &summary.images {
                let frame = crate::reconstruction::direct_input::image_path(&paths.frames, name)?;
                let rgb =
                    crate::reconstruction::direct_input::image_path(&paths.colmap_frames, name)?;
                let frame_header = crate::video::image_projection::inspect(&frame)?;
                let rgb_header = crate::video::image_projection::inspect(&rgb)?;
                let (_, width, height): (i32, u64, u64) = connection
                    .query_row(
                        "SELECT model,width,height FROM cameras WHERE camera_id=?1",
                        [camera_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .map_err(database_error)?;
                if frame_header.orientation != 1
                    || rgb_header.orientation != 1
                    || (frame_header.width, frame_header.height)
                        != (rgb_header.width, rgb_header.height)
                    || (u64::from(rgb_header.width), u64::from(rgb_header.height))
                        != (width, height)
                {
                    return Err(untrusted_artifact());
                }
                let mask = paths.masks.join(format!("{name}.png"));
                if checkpoint.has_alpha || optional_file(&mask)? {
                    crate::video::image_sequence::validate_binary_mask(
                        &mask,
                        (frame_header.width, frame_header.height),
                    )?;
                }
            }
            let base = read_model(&paths.colmap.join("base-model"))?;
            validate_legacy_v2_model(&connection, &summary, &base, &source_names)?;
            Ok(Some(summary))
        })(),
        &CancellationToken::new(),
    )
}

fn validate_legacy_v2_model(
    connection: &Connection,
    database: &DatabaseSummary,
    model: &crate::reconstruction::direct_input::Model,
    allowed_names: &BTreeSet<&String>,
) -> Result<()> {
    for camera in &model.cameras {
        let geometry: (i32, u64, u64) = connection
            .query_row(
                "SELECT model,width,height FROM cameras WHERE camera_id=?1",
                [camera.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(database_error)?;
        // BA may refine focal/distortion parameters; geometry and camera IDs
        // must still describe the exact feature coordinate system.
        if geometry != (camera.model_id, camera.width, camera.height) {
            return Err(untrusted_artifact());
        }
    }
    for image in &model.images {
        if !allowed_names.contains(&image.pose.name)
            || database.images.get(&image.pose.name)
                != Some(&(i64::from(image.pose.id), i64::from(image.pose.camera_id)))
        {
            return Err(untrusted_artifact());
        }
        let (rows, columns, data): (usize, usize, Option<Vec<u8>>) = connection
            .query_row(
                "SELECT rows,cols,data FROM keypoints WHERE image_id=?1",
                [image.pose.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(database_error)?;
        let data = data.unwrap_or_default();
        if rows != image.observations.len()
            || !matches!(columns, 2 | 4 | 6)
            || rows.checked_mul(columns).and_then(|n| n.checked_mul(4)) != Some(data.len())
        {
            return Err(untrusted_artifact());
        }
        for (observation, bytes) in image
            .observations
            .iter()
            .zip(data.chunks_exact(columns * 4))
        {
            let x = f64::from(f32::from_le_bytes(bytes[..4].try_into().unwrap()));
            let y = f64::from(f32::from_le_bytes(bytes[4..8].try_into().unwrap()));
            if !x.is_finite()
                || !y.is_finite()
                || (x - observation.x).abs() > 0.001
                || (y - observation.y).abs() > 0.001
            {
                return Err(untrusted_artifact());
            }
        }
    }
    Ok(())
}

fn validate_legacy_v2_models(
    root: &Path,
    database_path: &Path,
    database: &DatabaseSummary,
    allowed_names: &BTreeSet<&String>,
) -> Result<()> {
    let connection = open_immutable_database(database_path)?;
    let mut candidates = vec![root.to_path_buf()];
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            candidates.push(entry.path());
        }
    }
    let mut found = false;
    for candidate in candidates {
        let mut contains_model = false;
        for name in ["cameras.bin", "cameras.txt"] {
            contains_model |= optional_file(&candidate.join(name))?;
        }
        if contains_model {
            let model = read_model(&candidate)?;
            validate_legacy_v2_model(&connection, database, &model, allowed_names)?;
            found = true;
        }
    }
    found.then_some(()).ok_or_else(untrusted_artifact)
}

fn prepared_names_exist(paths: &ProjectPaths, names: &[String]) -> Result<bool> {
    if names.is_empty() {
        return Ok(false);
    }
    for name in names {
        for root in [&paths.frames, &paths.colmap_frames] {
            let valid = usable_validation(
                crate::reconstruction::direct_input::image_path(root, name)
                    .and_then(|path| crate::video::image_projection::inspect(&path))
                    .map(Some),
                &CancellationToken::new(),
            )?;
            if valid.is_none() {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn valid_model_root(root: &Path) -> Result<bool> {
    let cancel = CancellationToken::new();
    if usable_validation(read_model(root).map(Some), &cancel)?.is_some() {
        return Ok(true);
    }
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && usable_validation(read_model(&entry.path()).map(Some), &cancel)?.is_some()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn valid_brush_candidate(root: &Path) -> Result<bool> {
    for path in [root.join("final.ply.tmp"), root.join("final.ply.tmp.ply")] {
        if usable_validation(
            inspect_gaussian_ply(&path).map(Some),
            &CancellationToken::new(),
        )?
        .is_some()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn capture_prepared_cancellable(
    paths: &ProjectPaths,
    state: &PipelineStateFile,
    checkpoint: &crate::pipeline::inputs::InputCheckpoint,
    attempt: &str,
    upstream: &str,
    cancel: &CancellationToken,
) -> Result<ArtifactManifest> {
    if checkpoint.sources.is_empty() {
        return Err(untrusted_artifact());
    }
    let mut names = BTreeSet::new();
    let mut files = Vec::new();
    for source in &checkpoint.sources {
        if source.names.is_empty() {
            return Err(untrusted_artifact());
        }
        for name in &source.names {
            check_cancel(cancel)?;
            if !names.insert(name.clone()) {
                return Err(untrusted_artifact());
            }
            for root in [&paths.frames, &paths.colmap_frames] {
                let path = crate::reconstruction::direct_input::image_path(root, name)?;
                crate::video::image_projection::inspect(&path)?;
                files.push(describe_file_cancellable(
                    &paths.project,
                    &path,
                    false,
                    cancel,
                )?);
            }
        }
    }
    if optional_directory(&paths.masks)? {
        for path in regular_files_recursive(&paths.masks)? {
            files.push(describe_file_cancellable(
                &paths.project,
                &path,
                false,
                cancel,
            )?);
        }
    }

    if let Some(reshoot) = state.reshoot.as_ref() {
        let snapshot_root = paths.work.join("reshoot-snapshot-v3");
        if reshoot.snapshot_v3_ready || optional_file(&snapshot_root.join("snapshot.json"))? {
            crate::pipeline::reshoot_snapshot::verify_snapshot(&snapshot_root)?;
            files.push(describe_file_cancellable(
                &paths.project,
                &snapshot_root.join("snapshot.json"),
                true,
                cancel,
            )?);
            let database = snapshot_root.join("colmap/database.db");
            if optional_file(&database)? {
                files.push(describe_file_cancellable(
                    &paths.project,
                    &database,
                    true,
                    cancel,
                )?);
            }
            for path in reusable_model_files(&snapshot_root.join("colmap/sparse"))? {
                files.push(describe_file_cancellable(
                    &paths.project,
                    &path,
                    true,
                    cancel,
                )?);
            }
        }
        if reshoot.source_snapshot_complete {
            let database = paths.colmap.join("base-database.db");
            validate_database_cancellable(&database, cancel)?;
            files.push(describe_file_cancellable(
                &paths.project,
                &database,
                true,
                cancel,
            )?);
            let model = paths.colmap.join("base-model");
            read_model(&model)?;
            for path in reusable_model_files(&model)? {
                files.push(describe_file_cancellable(
                    &paths.project,
                    &path,
                    true,
                    cancel,
                )?);
            }
        }
    }
    make_manifest(attempt, upstream, files.len() as u64, None, files)
}

fn database_manifest_for_file(
    file: ArtifactFile,
    attempt: &str,
    upstream: &str,
    semantic_identity: &str,
    expected_count: u64,
) -> Result<ArtifactManifest> {
    make_manifest(
        attempt,
        upstream,
        expected_count,
        Some(semantic_identity.into()),
        vec![file],
    )
}

fn capture_reconstruction_cancellable(
    paths: &ProjectPaths,
    attempt: &str,
    upstream: &str,
    database: Option<&DatabaseSummary>,
    cancel: &CancellationToken,
) -> Result<ArtifactManifest> {
    check_cancel(cancel)?;
    let model_root = paths.colmap.join("authoritative");
    let model = read_model(&model_root)?;
    if model.cameras.is_empty() || model.images.is_empty() {
        return Err(untrusted_artifact());
    }
    let cameras = model
        .cameras
        .iter()
        .map(|camera| camera.id)
        .collect::<BTreeSet<_>>();
    for image in &model.images {
        check_cancel(cancel)?;
        if !cameras.contains(&image.pose.camera_id) {
            return Err(untrusted_artifact());
        }
        if let Some(database) = database {
            let Some((_, database_camera)) = database.images.get(&image.pose.name) else {
                return Err(untrusted_artifact());
            };
            if u32::try_from(*database_camera).ok() != Some(image.pose.camera_id) {
                return Err(untrusted_artifact());
            }
        }
    }
    let files = reusable_model_files(&model_root)?
        .iter()
        .map(|path| describe_file_cancellable(&paths.project, path, true, cancel))
        .collect::<Result<Vec<_>>>()?;
    make_manifest(attempt, upstream, model.images.len() as u64, None, files)
}

/// Validates the Brush dataset produced by `input_dataset`. Its signature is
/// written last, so the signature plus a complete model/image cross-check is
/// the durable completion evidence for ordinary, ERP and mixed views alike.
fn capture_training_dataset_cancellable(
    paths: &ProjectPaths,
    attempt: &str,
    upstream: &str,
    cancel: &CancellationToken,
) -> Result<ArtifactManifest> {
    check_cancel(cancel)?;
    let root = paths.brush.join("dataset");
    let signature = root.join("inputs-dataset.json");
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&signature)?)?;
    let pair = value
        .as_array()
        .filter(|value| value.len() == 2)
        .ok_or_else(untrusted_artifact)?;
    if pair[0].as_str().is_none_or(str::is_empty) {
        return Err(untrusted_artifact());
    }
    let dataset = pair[1].as_object().ok_or_else(untrusted_artifact)?;
    let recorded_root = dataset
        .get("path")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(untrusted_artifact)?;
    if std::fs::canonicalize(recorded_root)? != std::fs::canonicalize(&root)?
        || dataset
            .get("has_erp")
            .and_then(serde_json::Value::as_bool)
            .is_none()
        || dataset
            .get("max_edge")
            .and_then(serde_json::Value::as_u64)
            .is_none_or(|value| value == 0)
    {
        return Err(untrusted_artifact());
    }
    let reference = dataset
        .get("reference")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(untrusted_artifact)?;
    let model_root = root.join("sparse/0");
    let model = read_model(&model_root)?;
    if model.cameras.is_empty()
        || model.images.is_empty()
        || !model
            .images
            .iter()
            .any(|image| image.pose.name == reference)
    {
        return Err(untrusted_artifact());
    }
    let cameras = model
        .cameras
        .iter()
        .map(|camera| (camera.id, camera))
        .collect::<BTreeMap<_, _>>();
    let mut files = vec![describe_file_cancellable(
        &paths.project,
        &signature,
        true,
        cancel,
    )?];
    for path in reusable_model_files(&model_root)? {
        files.push(describe_file_cancellable(
            &paths.project,
            &path,
            true,
            cancel,
        )?);
    }
    for image in &model.images {
        check_cancel(cancel)?;
        let camera = cameras
            .get(&image.pose.camera_id)
            .ok_or_else(untrusted_artifact)?;
        let path = crate::reconstruction::direct_input::image_path(
            &root.join("images"),
            &image.pose.name,
        )?;
        let dimensions = image::image_dimensions(&path).map_err(image_validation_error)?;
        if (u64::from(dimensions.0), u64::from(dimensions.1)) != (camera.width, camera.height) {
            return Err(untrusted_artifact());
        }
        files.push(describe_file_cancellable(
            &paths.project,
            &path,
            false,
            cancel,
        )?);
    }
    make_manifest(attempt, upstream, model.images.len() as u64, None, files)
}

fn capture_candidate_cancellable(
    paths: &ProjectPaths,
    state: &PipelineStateFile,
    attempt: &str,
    upstream: &str,
    cancel: &CancellationToken,
) -> Result<ArtifactManifest> {
    check_cancel(cancel)?;
    let training_attempt = state
        .reshoot
        .as_ref()
        .and_then(|value| value.recipe.as_ref())
        .map_or(0, |recipe| recipe.training_fallbacks);
    let root = paths
        .brush
        .join(format!("collection-training-{training_attempt}"));
    let mut candidate = None;
    for path in [root.join("final.ply.tmp"), root.join("final.ply.tmp.ply")] {
        if let Some(info) = usable_validation(inspect_gaussian_ply(&path).map(Some), cancel)? {
            candidate = Some((path, info));
            break;
        }
    }
    let (path, info) = candidate.ok_or_else(untrusted_artifact)?;
    make_manifest(
        attempt,
        upstream,
        info.splat_count,
        Some(candidate_recipe_identity(state)?),
        vec![describe_file_cancellable(
            &paths.project,
            &path,
            true,
            cancel,
        )?],
    )
}

fn image_validation_error(error: image::ImageError) -> SplatError {
    match error {
        image::ImageError::IoError(error) => error.into(),
        error => SplatError::Process(error.to_string()),
    }
}

fn optional_file(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(true),
        Ok(_) => Err(SplatError::InvalidPath(path.to_path_buf())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn optional_directory(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(true),
        Ok(_) => Err(SplatError::InvalidPath(path.to_path_buf())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn candidate_recipe_identity(state: &PipelineStateFile) -> Result<String> {
    let initialization = state.reshoot.as_ref().and_then(|reshoot| {
        reshoot.recipe.as_ref().map(|recipe| {
            (
                recipe.actual,
                &recipe.warm_parameters,
                &recipe.init_sha256,
                &recipe.init_georeference,
            )
        })
    });
    let bytes = serde_json::to_vec(&(
        "brush-candidate-recipe-v1",
        state.preset,
        &state.configuration,
        &state.generation_policy,
        &state.resolution_plan,
        state
            .inputs
            .as_ref()
            .map(|inputs| (&inputs.request, &inputs.execution_plan)),
        state
            .inputs
            .as_ref()
            .map(|inputs| source_identity(state, inputs))
            .transpose()?,
        &state.brush_training.resolved,
        &state.brush_training.fine_splat_budget,
        &state.brush_training.dataset_identity,
        state
            .inputs
            .as_ref()
            .and_then(|inputs| inputs.init_sh_degree),
        initialization,
    ))?;
    let legacy_identity = format!("{:x}", Sha256::digest(bytes));
    // Do not append even a serialized None to the old tuple. Its exact hash
    // remains authoritative for existing completed candidates.
    let identity = match (
        state.brush_training.execution_options,
        state.brush_training.legacy_dataset_identity.as_ref(),
    ) {
        (None, None) => legacy_identity,
        (options, legacy_dataset) => {
            if let Some(options) = options {
                options.alpha_mode()?;
            }
            let bytes = serde_json::to_vec(&(
                "brush-candidate-recipe-v2",
                legacy_identity,
                options,
                legacy_dataset,
                &state.brush_training.legacy_initialization_sha256,
            ))?;
            format!("{:x}", Sha256::digest(bytes))
        }
    };
    // None is byte-for-byte compatible with every historical candidate. New
    // camera policies must be bound even for the legacy single-path executor.
    let identity = bind_camera_policy_identity(&identity, state.camera_grouping_policy_version)?;
    bind_ordinary_preparation_identity(&identity, state.ordinary_image_preparation_version)
}

/// Legacy replay uses its already prepared pixels rather than the modern
/// dataset builder. Bind the completed candidate with the same manifest and
/// semantic-recipe machinery before publication can be interrupted.
pub async fn record_saved_legacy_candidate(
    paths: &ProjectPaths,
    state: &mut PipelineStateFile,
    candidate: &Path,
    run_id: Option<Uuid>,
    cancel: &CancellationToken,
) -> Result<()> {
    let paths = paths.clone();
    let snapshot = Box::new(state.clone());
    let candidate = candidate.to_path_buf();
    let token = cancel.clone();
    let manifest = crate::presets::spawn_pipeline_blocking(move || {
        let info = inspect_gaussian_ply(&candidate)?;
        let upstream = snapshot
            .brush_training
            .legacy_dataset_identity
            .as_deref()
            .ok_or_else(untrusted_artifact)?;
        make_manifest(
            &attempt_label(run_id),
            upstream,
            info.splat_count,
            Some(candidate_recipe_identity(&snapshot)?),
            vec![describe_file_cancellable(
                &paths.project,
                &candidate,
                true,
                &token,
            )?],
        )
    })
    .await
    .map_err(|e| SplatError::Process(e.to_string()))??;
    state.artifact_manifests.version = ARTIFACT_MANIFEST_VERSION;
    state
        .artifact_manifests
        .entries
        .insert(BRUSH_CANDIDATE.into(), manifest);
    Ok(())
}

pub async fn recover_saved_legacy_candidate(
    paths: &ProjectPaths,
    state: &PipelineStateFile,
    cancel: &CancellationToken,
) -> Result<Option<PathBuf>> {
    if !state.brush_complete
        || state.brush_training.legacy_dataset_identity.is_none()
        || !state.retry_cache_pending.is_empty()
        || state.database_recovery_pending.is_some()
    {
        return Ok(None);
    }
    let saved = state
        .artifact_manifests
        .entries
        .get(BRUSH_CANDIDATE)
        .cloned()
        .ok_or_else(untrusted_artifact)?;
    if state.artifact_manifests.version != ARTIFACT_MANIFEST_VERSION || saved.files.len() != 1 {
        return Err(untrusted_artifact());
    }
    let relative = Path::new(&saved.files[0].relative_path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(untrusted_artifact());
    }
    let paths = paths.clone();
    let snapshot = Box::new(state.clone());
    let token = cancel.clone();
    crate::presets::spawn_pipeline_blocking(move || {
        check_cancel(&token)?;
        let path = paths.project.join(&saved.files[0].relative_path);
        let output = snapshot
            .validated_output
            .as_ref()
            .ok_or_else(untrusted_artifact)?;
        let recipe = candidate_recipe_identity(&snapshot)?;
        if saved.semantic_identity.as_ref() != Some(&recipe)
            || snapshot.brush_training.legacy_dataset_identity.as_ref()
                != Some(&saved.upstream_identity)
        {
            // Conflicting historical configuration is not a disposable file.
            return Err(untrusted_artifact());
        }
        let Some(info) = usable_validation(inspect_gaussian_ply(&path).map(Some), &token)? else {
            return Ok(None);
        };
        let current = make_manifest(
            &saved.attempt_id,
            snapshot
                .brush_training
                .legacy_dataset_identity
                .as_deref()
                .ok_or_else(untrusted_artifact)?,
            info.splat_count,
            Some(recipe),
            vec![describe_file_cancellable(
                &paths.project,
                &path,
                true,
                &token,
            )?],
        )?;
        if !exact_manifest_match(&saved, &current)
            || output.splat_count != info.splat_count
            || output.file_size != info.file_size
        {
            // Missing/modified output must not skip database recovery based
            // only on old completion flags. Normalization decides which of
            // the remaining, independently verified artifacts can be reused.
            return Ok(None);
        }
        Ok(Some(path))
    })
    .await
    .map_err(|e| SplatError::Process(e.to_string()))?
}

fn candidate_manifest_match(saved: &ArtifactManifest, current: &ArtifactManifest) -> bool {
    if saved.semantic_identity.is_some() {
        return exact_manifest_match(saved, current);
    }
    // Keep old completed datasets/candidates compatible. They cannot support
    // dataset-independent recovery because they lack the recipe identity.
    make_manifest(
        &current.attempt_id,
        &current.upstream_identity,
        current.expected_count,
        None,
        current.files.clone(),
    )
    .is_ok_and(|legacy| exact_manifest_match(saved, &legacy))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DatabaseSummary {
    images: BTreeMap<String, (i64, i64)>,
    feature_identity: String,
    matching_identity: String,
    match_count: u64,
}

fn validate_database(path: &Path) -> Result<DatabaseSummary> {
    validate_database_cancellable(path, &CancellationToken::new())
}

fn validate_database_cancellable(
    path: &Path,
    cancel: &CancellationToken,
) -> Result<DatabaseSummary> {
    check_cancel(cancel)?;
    let initial_stamp = database_stamps(path, cancel)?;
    // Finish all SQLite readers before opening the exclusive-against-writers
    // change-evidence handles. Never relax their sharing rules to hide a reader
    // owned by this validation operation (or a genuinely active external writer).
    let summary = {
        let connection = open_immutable_database(path)?;
        read_database_summary(&connection, cancel)?
    };
    check_cancel(cancel)?;
    if database_stamps(path, cancel)? != initial_stamp {
        return Err(database_in_use("COLMAP database changed during validation"));
    }
    Ok(summary)
}

/// Shared by immutable cache validation and the locked mutable-db recovery.
/// The caller owns connection lifetime and must not expose immutable source
/// snapshots to a writable SQLite connection.
pub(super) fn read_database_summary(
    connection: &Connection,
    cancel: &CancellationToken,
) -> Result<DatabaseSummary> {
    let integrity: String = connection
        .query_row("PRAGMA integrity_check(1)", [], |row| row.get(0))
        .map_err(database_error)?;
    if integrity != "ok" {
        return Err(untrusted_artifact());
    }
    check_cancel(cancel)?;
    let cameras = scalar_set(
        connection,
        "SELECT camera_id FROM cameras ORDER BY camera_id",
    )?;
    if cameras.is_empty() {
        return Err(untrusted_artifact());
    }
    let mut images = BTreeMap::new();
    let mut image_ids = BTreeSet::new();
    let mut statement = connection
        .prepare("SELECT image_id,name,camera_id FROM images ORDER BY image_id")
        .map_err(database_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(database_error)?;
    for row in rows {
        check_cancel(cancel)?;
        let (id, name, camera) = row.map_err(database_error)?;
        if id <= 0
            || name.is_empty()
            || !cameras.contains(&camera)
            || !image_ids.insert(id)
            || images.insert(name, (id, camera)).is_some()
        {
            return Err(untrusted_artifact());
        }
    }
    if images.is_empty() {
        return Err(untrusted_artifact());
    }

    let keypoints = matrix_rows(connection, "keypoints", 4, cancel, |cols| {
        matches!(cols, 2 | 4 | 6)
    })?;
    let descriptors = matrix_rows(connection, "descriptors", 1, cancel, |cols| cols == 128)?;
    if keypoints.len() != images.len()
        || descriptors.len() != images.len()
        || keypoints.keys().any(|id| !image_ids.contains(id))
        || descriptors.keys().any(|id| !image_ids.contains(id))
        || keypoints.iter().any(|(id, row)| {
            descriptors
                .get(id)
                .is_none_or(|descriptor| descriptor.rows != row.rows)
        })
    {
        return Err(untrusted_artifact());
    }

    let mut feature_hash = Sha256::new();
    hash_label(&mut feature_hash, "artifact-features-v1");
    for (name, (id, camera)) in &images {
        check_cancel(cancel)?;
        hash_i64(&mut feature_hash, *id);
        hash_i64(&mut feature_hash, *camera);
        hash_label(&mut feature_hash, name);
        hash_matrix(&mut feature_hash, &keypoints[id]);
        hash_matrix(&mut feature_hash, &descriptors[id]);
    }
    let feature_identity = format!("{:x}", feature_hash.finalize());

    const MAX_IMAGE_ID: i64 = 2_147_483_647;
    let mut matches_hash = Sha256::new();
    hash_label(&mut matches_hash, "artifact-matches-v1");
    hash_label(&mut matches_hash, &feature_identity);
    let mut match_count = 0_u64;
    let mut statement = connection
        .prepare("SELECT pair_id,rows,cols,data FROM matches ORDER BY pair_id")
        .map_err(database_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<Vec<u8>>>(3)?,
            ))
        })
        .map_err(database_error)?;
    for row in rows {
        check_cancel(cancel)?;
        let (pair, row_count, cols, data) = row.map_err(database_error)?;
        let first = pair / MAX_IMAGE_ID;
        let second = pair % MAX_IMAGE_ID;
        let bytes = data.unwrap_or_default();
        if pair <= 0
            || first <= 0
            || second <= 0
            || first == second
            || !image_ids.contains(&first)
            || !image_ids.contains(&second)
            || row_count < 0
            || cols != 2
            || matrix_byte_len(row_count, cols, 4) != Some(bytes.len())
        {
            return Err(untrusted_artifact());
        }
        match_count = match_count.saturating_add(row_count as u64);
        hash_i64(&mut matches_hash, pair);
        hash_i64(&mut matches_hash, row_count);
        hash_i64(&mut matches_hash, cols);
        matches_hash.update(&bytes);
    }
    let raw_matching_identity = format!("{:x}", matches_hash.finalize());
    let matching_identity =
        complete_matching_identity(connection, &raw_matching_identity, &keypoints, cancel)?;
    Ok(DatabaseSummary {
        images,
        feature_identity,
        matching_identity,
        match_count,
    })
}

/// v1 covered only raw matches. Mapper also consumes verified geometry, camera
/// calibration, rigs/frames and position priors. Hash every column of those
/// tables (including schema/presence) in deterministic row order, so a WAL
/// checkpoint may change file layout without concealing changed SfM inputs.
fn complete_matching_identity(
    connection: &Connection,
    raw_matching_identity: &str,
    keypoints: &BTreeMap<i64, MatrixRow>,
    cancel: &CancellationToken,
) -> Result<String> {
    let mut hash = Sha256::new();
    hash_label(&mut hash, MATCHING_SEMANTIC_PREFIX);
    hash_label(&mut hash, raw_matching_identity);
    for table in [
        "cameras",
        "images",
        "rigs",
        "rig_sensors",
        "frames",
        "frame_data",
        "pose_priors",
        "two_view_geometries",
    ] {
        check_cancel(cancel)?;
        hash_label(&mut hash, table);
        let mut schema = connection
            .prepare(&format!("PRAGMA table_info(\"{table}\")"))
            .map_err(database_error)?;
        let columns = schema
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .map_err(database_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(database_error)?;
        if columns.is_empty() {
            hash_label(&mut hash, "absent");
            continue;
        }
        hash_label(&mut hash, &serde_json::to_string(&columns)?);
        let mut keys = columns
            .iter()
            .enumerate()
            .filter(|(_, column)| column.4 > 0)
            .map(|(index, column)| (column.4, index + 1))
            .collect::<Vec<_>>();
        keys.sort_unstable();
        let ordering = if keys.is_empty() {
            (1..=columns.len())
                .map(|index| index.to_string())
                .collect::<Vec<_>>()
        } else {
            keys.into_iter()
                .map(|(_, index)| index.to_string())
                .collect()
        }
        .join(",");
        let mut statement = connection
            .prepare(&format!("SELECT * FROM \"{table}\" ORDER BY {ordering}"))
            .map_err(database_error)?;
        let mut rows = statement.query([]).map_err(database_error)?;
        while let Some(row) = rows.next().map_err(database_error)? {
            check_cancel(cancel)?;
            hash_label(&mut hash, "row");
            if table == "two_view_geometries" {
                validate_geometry_row(row, keypoints)?;
            }
            for index in 0..columns.len() {
                use rusqlite::types::ValueRef;
                match row.get_ref(index).map_err(database_error)? {
                    ValueRef::Null => hash.update([0]),
                    ValueRef::Integer(value) => {
                        hash.update([1]);
                        hash_i64(&mut hash, value);
                    }
                    ValueRef::Real(value) if value.is_finite() => {
                        hash.update([2]);
                        hash.update(value.to_bits().to_le_bytes());
                    }
                    ValueRef::Real(_) => return Err(untrusted_artifact()),
                    ValueRef::Text(value) => {
                        hash.update([3]);
                        hash.update((value.len() as u64).to_le_bytes());
                        hash.update(value);
                    }
                    ValueRef::Blob(value) => {
                        hash.update([4]);
                        hash.update((value.len() as u64).to_le_bytes());
                        hash.update(value);
                    }
                }
            }
        }
    }
    Ok(format!("{MATCHING_SEMANTIC_PREFIX}{:x}", hash.finalize()))
}

fn validate_geometry_row(
    row: &rusqlite::Row<'_>,
    keypoints: &BTreeMap<i64, MatrixRow>,
) -> Result<()> {
    const MAX_IMAGE_ID: i64 = 2_147_483_647;
    let pair = row.get::<_, i64>("pair_id").map_err(database_error)?;
    let count = row.get::<_, i64>("rows").map_err(database_error)?;
    let cols = row.get::<_, i64>("cols").map_err(database_error)?;
    let data = row
        .get::<_, Option<Vec<u8>>>("data")
        .map_err(database_error)?
        .unwrap_or_default();
    let first = pair / MAX_IMAGE_ID;
    let second = pair % MAX_IMAGE_ID;
    let (Some(first_points), Some(second_points)) = (keypoints.get(&first), keypoints.get(&second))
    else {
        return Err(untrusted_artifact());
    };
    if pair <= 0
        || first >= second
        || count < 0
        || (cols != 2 && !(count == 0 && cols == 0))
        || matrix_byte_len(count, cols, 4) != Some(data.len())
    {
        return Err(untrusted_artifact());
    }
    for pair in data.as_chunks::<8>().0 {
        let left = u32::from_le_bytes(pair[..4].try_into().expect("four bytes"));
        let right = u32::from_le_bytes(pair[4..].try_into().expect("four bytes"));
        if i64::from(left) >= first_points.rows || i64::from(right) >= second_points.rows {
            return Err(untrusted_artifact());
        }
    }
    Ok(())
}

#[derive(Debug)]
struct MatrixRow {
    rows: i64,
    cols: i64,
    data: Vec<u8>,
}

fn matrix_rows(
    connection: &Connection,
    table: &str,
    element_size: usize,
    cancel: &CancellationToken,
    valid_columns: impl Fn(i64) -> bool,
) -> Result<BTreeMap<i64, MatrixRow>> {
    let sql = format!("SELECT image_id,rows,cols,data FROM {table} ORDER BY image_id");
    let mut statement = connection.prepare(&sql).map_err(database_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<Vec<u8>>>(3)?,
            ))
        })
        .map_err(database_error)?;
    let mut result = BTreeMap::new();
    for row in rows {
        check_cancel(cancel)?;
        let (image, rows, cols, data) = row.map_err(database_error)?;
        let data = data.unwrap_or_default();
        if image <= 0
            || rows < 0
            || !valid_columns(cols)
            || matrix_byte_len(rows, cols, element_size) != Some(data.len())
            || result
                .insert(image, MatrixRow { rows, cols, data })
                .is_some()
        {
            return Err(untrusted_artifact());
        }
    }
    Ok(result)
}

fn scalar_set(connection: &Connection, sql: &str) -> Result<BTreeSet<i64>> {
    let mut statement = connection.prepare(sql).map_err(database_error)?;
    let rows = statement
        .query_map([], |row| row.get::<_, i64>(0))
        .map_err(database_error)?;
    let mut values = BTreeSet::new();
    for value in rows {
        if !values.insert(value.map_err(database_error)?) {
            return Err(untrusted_artifact());
        }
    }
    Ok(values)
}

fn matrix_byte_len(rows: i64, cols: i64, element_size: usize) -> Option<usize> {
    usize::try_from(rows)
        .ok()?
        .checked_mul(usize::try_from(cols).ok()?)?
        .checked_mul(element_size)
}

fn hash_matrix(hash: &mut Sha256, row: &MatrixRow) {
    hash_i64(hash, row.rows);
    hash_i64(hash, row.cols);
    hash.update(&row.data);
}

fn database_error(error: rusqlite::Error) -> SplatError {
    database_read_error(error)
}

fn make_manifest(
    attempt: &str,
    upstream: &str,
    expected_count: u64,
    semantic_identity: Option<String>,
    mut files: Vec<ArtifactFile>,
) -> Result<ArtifactManifest> {
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    let bytes = serde_json::to_vec(&(upstream, expected_count, &semantic_identity, &files))?;
    Ok(ArtifactManifest {
        attempt_id: attempt.into(),
        upstream_identity: upstream.into(),
        expected_count,
        semantic_identity,
        artifact_identity: format!("{:x}", Sha256::digest(bytes)),
        files,
    })
}

fn exact_manifest_match(saved: &ArtifactManifest, current: &ArtifactManifest) -> bool {
    !saved.artifact_identity.is_empty()
        && saved.artifact_identity == current.artifact_identity
        && saved.upstream_identity == current.upstream_identity
}

/// Reinstalling a verified immutable reshoot base changes file timestamps, not
/// the prepared images or their pixel mapping. Rebind only those strong-hash
/// base entries after the temporary focal policy archives downstream caches.
/// Never establish a fresh weak baseline for other prepared assets here.
pub(crate) fn rebind_restored_focal_base(
    paths: &ProjectPaths,
    state: &mut PipelineStateFile,
    cancel: &CancellationToken,
) -> Result<()> {
    let Some(saved) = state.artifact_manifests.entries.get(PREPARED).cloned() else {
        return Ok(());
    };
    let checkpoint = state.inputs.as_ref().ok_or_else(untrusted_artifact)?;
    if source_identity(state, checkpoint)? != saved.upstream_identity {
        return Err(untrusted_artifact());
    }
    let current = capture_prepared_cancellable(
        paths,
        state,
        checkpoint,
        &saved.attempt_id,
        &saved.upstream_identity,
        cancel,
    )?;
    let base = paths
        .colmap
        .strip_prefix(&paths.project)
        .map_err(|_| untrusted_artifact())?;
    let base_model = base.join("base-model");
    let base_database = base.join("base-database.db");
    if saved.expected_count != current.expected_count
        || saved.files.len() != current.files.len()
        || !saved.files.iter().zip(&current.files).all(|(old, now)| {
            let path = Path::new(&old.relative_path);
            if path.starts_with(&base_model) || path == base_database {
                let mut expected = old.clone();
                expected.modified_nanos = now.modified_nanos;
                old.sha256.is_some() && expected == *now
            } else {
                old == now
            }
        })
    {
        return Err(untrusted_artifact());
    }
    state
        .artifact_manifests
        .entries
        .insert(PREPARED.into(), current);
    Ok(())
}

fn semantic_manifest_match(saved: &ArtifactManifest, current: &ArtifactManifest) -> bool {
    saved.upstream_identity == current.upstream_identity
        && saved.expected_count == current.expected_count
        && saved.semantic_identity.is_some()
        && saved.semantic_identity == current.semantic_identity
        && saved.files.len() == current.files.len()
        && saved.files.iter().zip(&current.files).all(|(left, right)| {
            left.relative_path == right.relative_path && left.file_type == right.file_type
        })
}

fn same_file_content(saved: &ArtifactFile, current: &ArtifactFile) -> bool {
    saved.relative_path == current.relative_path
        && saved.file_type == current.file_type
        && saved.file_size == current.file_size
        && saved.sha256.is_some()
        && saved.sha256 == current.sha256
}

fn matching_manifest_match(saved: &ArtifactManifest, current: &ArtifactManifest) -> bool {
    if saved
        .semantic_identity
        .as_deref()
        .is_some_and(|value| value.starts_with(MATCHING_SEMANTIC_PREFIX))
    {
        return semantic_manifest_match(saved, current);
    }
    // Old raw-match-only summaries do not certify camera/verified geometry.
    // Preserve unchanged content (including mtime-only recovery) but never
    // replace this evidence with a newly measured semantic baseline.
    saved.upstream_identity == current.upstream_identity
        && saved.expected_count == current.expected_count
        && !saved.files.is_empty()
        && saved.files.len() == current.files.len()
        && saved
            .files
            .iter()
            .zip(&current.files)
            .all(|(old, new)| same_file_content(old, new))
}

fn describe_file_cancellable(
    project_root: &Path,
    path: &Path,
    critical: bool,
    cancel: &CancellationToken,
) -> Result<ArtifactFile> {
    check_cancel(cancel)?;
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(SplatError::InvalidPath(path.to_path_buf()));
    }
    let canonical_root = std::fs::canonicalize(project_root)?;
    let canonical_path = std::fs::canonicalize(path)?;
    let relative_path = canonical_path
        .strip_prefix(&canonical_root)
        .map_err(|_| SplatError::InvalidPath(path.to_path_buf()))?
        .to_string_lossy()
        .replace('\\', "/");
    Ok(ArtifactFile {
        relative_path,
        file_type: ArtifactFileType::RegularFile,
        file_size: metadata.len(),
        modified_nanos: metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|value| value.as_nanos()),
        sha256: critical
            .then(|| hash_file_cancellable(path, cancel))
            .transpose()?,
    })
}

fn hash_file_cancellable(path: &Path, cancel: &CancellationToken) -> Result<String> {
    let mut input = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 256 * 1024];
    loop {
        check_cancel(cancel)?;
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn regular_files_recursive(root: &Path) -> Result<Vec<PathBuf>> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let metadata = std::fs::symlink_metadata(entry.path())?;
            if metadata.file_type().is_dir() {
                pending.push(entry.path());
            } else if metadata.file_type().is_file() {
                files.push(entry.path());
            } else {
                return Err(SplatError::InvalidPath(entry.path()));
            }
        }
    }
    files.sort();
    Ok(files)
}

fn source_identity(
    state: &PipelineStateFile,
    checkpoint: &crate::pipeline::inputs::InputCheckpoint,
) -> Result<String> {
    validate_collection_camera_policy(state, checkpoint)?;
    let sources = checkpoint
        .sources
        .iter()
        .map(|source| (&source.id, &source.identity))
        .collect::<Vec<_>>();
    let snapshot = state.reshoot.as_ref().map(|reshoot| {
        (
            reshoot.source_snapshot_complete,
            reshoot.snapshot_v3_ready,
            reshoot.snapshot_v3_identity.as_deref(),
        )
    });
    let bytes = serde_json::to_vec(&(sources, snapshot))?;
    let identity = bind_camera_policy_identity(
        &format!("{:x}", Sha256::digest(bytes)),
        checkpoint.camera_grouping_policy_version,
    )?;
    let identity = bind_ordinary_preparation_identity(
        &identity,
        checkpoint.ordinary_image_preparation_version,
    )?;
    let versions = checkpoint
        .sources
        .iter()
        .filter_map(|source| {
            source
                .ordinary_image_preparation_version
                .map(|version| (&source.id, version))
        })
        .collect::<Vec<_>>();
    if versions.is_empty() {
        Ok(identity)
    } else {
        let bytes = serde_json::to_vec(&("ordinary-source-preparation-v1", identity, versions))?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
}

fn validate_collection_camera_policy(
    state: &PipelineStateFile,
    checkpoint: &crate::pipeline::inputs::InputCheckpoint,
) -> Result<()> {
    if state.ordinary_image_preparation_version != checkpoint.ordinary_image_preparation_version {
        return Err(SplatError::Process(
            "Saved ordinary image preparation policy does not match the input checkpoint".into(),
        ));
    }
    for source in &checkpoint.sources {
        if source.ordinary_image_preparation_version.is_some()
            && (source.id == "__source"
                || source.video.is_some()
                || source.projection != crate::video::panorama::VideoProjection::Perspective
                || source.ordinary_image_preparation_version
                    != checkpoint.ordinary_image_preparation_version)
        {
            return Err(SplatError::Process(
                "Saved source image preparation policy is inconsistent".into(),
            ));
        }
    }
    for version in std::iter::once(checkpoint.ordinary_image_preparation_version)
        .chain(
            checkpoint
                .sources
                .iter()
                .map(|source| source.ordinary_image_preparation_version),
        )
        .flatten()
    {
        if version != super::inputs::ORDINARY_IMAGE_PREPARATION_VERSION {
            return Err(SplatError::Process(
                "Unsupported saved ordinary image preparation version".into(),
            ));
        }
    }
    if state.camera_grouping_policy_version != checkpoint.camera_grouping_policy_version {
        return Err(SplatError::Process(
            "Saved camera grouping policy does not match the input checkpoint".into(),
        ));
    }
    if checkpoint
        .camera_grouping_policy_version
        .is_some_and(|version| version != crate::pipeline::inputs::CAMERA_GROUPING_POLICY_VERSION)
    {
        return Err(SplatError::Process(
            "Unsupported saved camera grouping policy version".into(),
        ));
    }
    Ok(())
}

fn bind_camera_policy_identity(identity: &str, version: Option<u32>) -> Result<String> {
    match version {
        None => Ok(identity.into()),
        Some(version) if version == crate::pipeline::inputs::CAMERA_GROUPING_POLICY_VERSION => {
            let bytes = serde_json::to_vec(&("camera-grouping-policy-v1", identity, version))?;
            Ok(format!("{:x}", Sha256::digest(bytes)))
        }
        Some(_) => Err(SplatError::Process(
            "Unsupported saved camera grouping policy version".into(),
        )),
    }
}

fn bind_ordinary_preparation_identity(identity: &str, version: Option<u32>) -> Result<String> {
    match version {
        None => Ok(identity.into()),
        Some(version) if version == super::inputs::ORDINARY_IMAGE_PREPARATION_VERSION => {
            let bytes = serde_json::to_vec(&("ordinary-image-preparation-v1", identity, version))?;
            Ok(format!("{:x}", Sha256::digest(bytes)))
        }
        Some(_) => Err(SplatError::Process(
            "Unsupported saved ordinary image preparation version".into(),
        )),
    }
}

fn attempt_label(run_id: Option<Uuid>) -> String {
    run_id
        .map(|value| value.to_string())
        .unwrap_or_else(|| "legacy-run".into())
}

fn invalidate_prepared(
    state: &mut PipelineStateFile,
    checkpoint: &mut crate::pipeline::inputs::InputCheckpoint,
) {
    checkpoint.prepared = false;
    if checkpoint.recovery_sources.is_empty() {
        checkpoint.recovery_sources = checkpoint.sources.clone();
    }
    checkpoint.sources.clear();
    checkpoint.unused.clear();
    checkpoint.source_adoption.clear();
    checkpoint.reference_image = None;
    state.frames = None;
    state.actual_transparency = None;
    // Preserve the exact attempt ledger and frozen selections. Recovery may
    // restore completed bridge pixels or continue its existing Running attempt,
    // but must never spend the same budget again with a newly computed plan.
    state.features_complete = false;
    state.matching_complete = false;
    state.reconstruction_complete = false;
    state.brush_complete = false;
}

fn normalization_state(
    state: &PipelineStateFile,
    checkpoint: &crate::pipeline::inputs::InputCheckpoint,
) -> Result<Vec<u8>> {
    // Include the recipe, retry ledger, reference and invalidation barrier as
    // well as completion flags. Omitting one of these can hide a mutation from
    // the caller and make a second normalization non-idempotent.
    Ok(serde_json::to_vec(&(state, checkpoint))?)
}

fn untrusted_artifact() -> SplatError {
    SplatError::Process("Generation artifact is missing, changed, or inconsistent".into())
}

fn hash_i64(hash: &mut Sha256, value: i64) {
    hash.update(value.to_le_bytes());
}

fn hash_label(hash: &mut Sha256, value: &str) {
    hash.update((value.len() as u64).to_le_bytes());
    hash.update(value.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_preparation_identity_preserves_legacy_and_binds_each_saved_source() {
        for reshoot in [false, true] {
            let mut fixture = fixture(reshoot);
            let legacy_source =
                source_identity(&fixture.state, fixture.state.inputs.as_ref().unwrap()).unwrap();
            let legacy_candidate = candidate_recipe_identity(&fixture.state).unwrap();
            assert_eq!(
                bind_ordinary_preparation_identity(&legacy_source, None).unwrap(),
                legacy_source
            );
            fixture.state.ordinary_image_preparation_version = Some(1);
            fixture
                .state
                .inputs
                .as_mut()
                .unwrap()
                .ordinary_image_preparation_version = Some(1);
            let versioned_source =
                source_identity(&fixture.state, fixture.state.inputs.as_ref().unwrap()).unwrap();
            assert_ne!(versioned_source, legacy_source);
            let source = fixture
                .state
                .inputs
                .as_mut()
                .unwrap()
                .sources
                .iter_mut()
                .find(|source| source.id != "__source" && source.video.is_none())
                .unwrap();
            source.ordinary_image_preparation_version = Some(1);
            assert_ne!(
                source_identity(&fixture.state, fixture.state.inputs.as_ref().unwrap()).unwrap(),
                versioned_source
            );
            assert_ne!(
                candidate_recipe_identity(&fixture.state).unwrap(),
                legacy_candidate
            );
            let restored: PipelineStateFile =
                serde_json::from_slice(&serde_json::to_vec(&fixture.state).unwrap()).unwrap();
            assert_eq!(
                candidate_recipe_identity(&fixture.state).unwrap(),
                candidate_recipe_identity(&restored).unwrap()
            );
            fixture.state.ordinary_image_preparation_version = None;
            let checkpoint = fixture.state.inputs.as_mut().unwrap();
            checkpoint.ordinary_image_preparation_version = None;
            for source in &mut checkpoint.sources {
                source.ordinary_image_preparation_version = None;
            }
            assert_eq!(
                source_identity(&fixture.state, fixture.state.inputs.as_ref().unwrap()).unwrap(),
                legacy_source
            );
            assert_eq!(
                candidate_recipe_identity(&fixture.state).unwrap(),
                legacy_candidate
            );
        }
    }

    #[tokio::test]
    async fn contradictory_ordinary_preparation_policies_preserve_saved_results() {
        for (state_version, checkpoint_version) in
            [(None, Some(1)), (Some(1), None), (Some(2), Some(2))]
        {
            let mut fixture = fixture(false);
            fixture.state.ordinary_image_preparation_version = state_version;
            fixture
                .state
                .inputs
                .as_mut()
                .unwrap()
                .ordinary_image_preparation_version = checkpoint_version;
            let before = serde_json::to_value(&fixture.state).unwrap();
            let candidate = std::fs::read(&fixture.candidate).unwrap();
            assert!(
                normalize_collection(&fixture.paths, &mut fixture.state, None)
                    .await
                    .is_err()
            );
            assert_eq!(serde_json::to_value(&fixture.state).unwrap(), before);
            assert_eq!(std::fs::read(&fixture.candidate).unwrap(), candidate);
        }
    }

    #[test]
    fn camera_policy_binds_new_sources_and_candidates_but_preserves_legacy_hashes() {
        for reshoot in [false, true] {
            let mut fixture = fixture(reshoot);
            let checkpoint = fixture.state.inputs.as_ref().unwrap();
            let sources = checkpoint
                .sources
                .iter()
                .map(|source| (&source.id, &source.identity))
                .collect::<Vec<_>>();
            let snapshot = fixture.state.reshoot.as_ref().map(|reshoot| {
                (
                    reshoot.source_snapshot_complete,
                    reshoot.snapshot_v3_ready,
                    reshoot.snapshot_v3_identity.as_deref(),
                )
            });
            let old_hash = format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&(sources, snapshot)).unwrap())
            );
            assert_eq!(
                source_identity(&fixture.state, checkpoint).unwrap(),
                old_hash
            );
            let old_candidate = candidate_recipe_identity(&fixture.state).unwrap();
            fixture.state.camera_grouping_policy_version = Some(1);
            fixture
                .state
                .inputs
                .as_mut()
                .unwrap()
                .camera_grouping_policy_version = Some(1);
            assert_ne!(
                source_identity(&fixture.state, fixture.state.inputs.as_ref().unwrap()).unwrap(),
                old_hash
            );
            assert_ne!(
                candidate_recipe_identity(&fixture.state).unwrap(),
                old_candidate
            );
            let restored: PipelineStateFile =
                serde_json::from_slice(&serde_json::to_vec(&fixture.state).unwrap()).unwrap();
            assert_eq!(
                candidate_recipe_identity(&fixture.state).unwrap(),
                candidate_recipe_identity(&restored).unwrap()
            );
            fixture.state.camera_grouping_policy_version = None;
            fixture
                .state
                .inputs
                .as_mut()
                .unwrap()
                .camera_grouping_policy_version = None;
            assert_eq!(
                source_identity(&fixture.state, fixture.state.inputs.as_ref().unwrap()).unwrap(),
                old_hash
            );
            assert_eq!(
                candidate_recipe_identity(&fixture.state).unwrap(),
                old_candidate
            );
        }
    }

    #[tokio::test]
    async fn contradictory_camera_policies_fail_without_relabelling_or_invalidating_saved_results()
    {
        for (state_version, input_version) in [(None, Some(1)), (Some(1), None), (Some(2), Some(2))]
        {
            let mut fixture = fixture(false);
            fixture.state.camera_grouping_policy_version = state_version;
            fixture
                .state
                .inputs
                .as_mut()
                .unwrap()
                .camera_grouping_policy_version = input_version;
            let before = serde_json::to_value(&fixture.state).unwrap();
            let candidate = std::fs::read(&fixture.candidate).unwrap();
            assert!(
                normalize_collection(&fixture.paths, &mut fixture.state, None)
                    .await
                    .is_err()
            );
            assert_eq!(serde_json::to_value(&fixture.state).unwrap(), before);
            assert_eq!(std::fs::read(&fixture.candidate).unwrap(), candidate);
            assert!(candidate_recipe_identity(&fixture.state).is_err());
        }
    }

    #[tokio::test]
    async fn changed_camera_policy_never_promotes_old_or_unmanifested_feature_caches() {
        for missing_manifest in [false, true] {
            let mut fixture = fixture(false);
            let candidate = std::fs::read(&fixture.candidate).unwrap();
            let database = std::fs::read(fixture.paths.colmap.join("database.db")).unwrap();
            fixture.state.camera_grouping_policy_version = Some(1);
            fixture
                .state
                .inputs
                .as_mut()
                .unwrap()
                .camera_grouping_policy_version = Some(1);
            if missing_manifest {
                fixture.state.artifact_manifests = Default::default();
            }
            assert!(
                normalize_collection(&fixture.paths, &mut fixture.state, None)
                    .await
                    .unwrap()
            );
            assert!(!fixture.state.features_complete);
            assert!(!fixture.state.matching_complete);
            assert!(!fixture.state.reconstruction_complete);
            assert!(!fixture.state.brush_complete);
            assert!(!fixture
                .state
                .artifact_manifests
                .entries
                .contains_key(FEATURES));
            // Normalization marks a durable archive requirement; it never
            // deletes or edits an old candidate/database to fit a new policy.
            assert_eq!(std::fs::read(&fixture.candidate).unwrap(), candidate);
            assert_eq!(
                std::fs::read(fixture.paths.colmap.join("database.db")).unwrap(),
                database
            );
        }
    }

    #[test]
    fn collection_observations_never_change_source_or_candidate_recipe_identity() {
        use crate::pipeline::effectiveness::{
            CollectionTelemetry, PlannerBridgeMetrics, PlannerBridgeStatus,
            PlannerFramePlanMetrics, PlannerGpuVendor,
        };
        for reshoot in [false, true] {
            let mut fixture = fixture(reshoot);
            let source_before =
                source_identity(&fixture.state, fixture.state.inputs.as_ref().unwrap()).unwrap();
            let candidate_before = candidate_recipe_identity(&fixture.state).unwrap();
            fixture.state.inputs.as_mut().unwrap().telemetry = Some(CollectionTelemetry {
                source_id: Some("observation-only".into()),
                source_identity: Some("not-an-execution-input".into()),
                gpu_vendor: Some(PlannerGpuVendor::Nvidia),
                frame_plan: Some(PlannerFramePlanMetrics::images(1920, 1080, 100)),
                bridge: Some(PlannerBridgeMetrics {
                    status: PlannerBridgeStatus::Completed,
                    added_frames: 20,
                    duration_ms: Some(1500),
                    adopted: true,
                    ..Default::default()
                }),
                ..Default::default()
            });
            let saved: PipelineStateFile =
                serde_json::from_slice(&serde_json::to_vec(&fixture.state).unwrap()).unwrap();
            assert_eq!(
                source_identity(&saved, saved.inputs.as_ref().unwrap()).unwrap(),
                source_before
            );
            assert_eq!(candidate_recipe_identity(&saved).unwrap(), candidate_before);
        }
    }

    #[test]
    fn legacy_candidate_hash_stays_stable_but_explicit_alpha_options_bind_new_candidates() {
        let mut state = PipelineStateFile::created(crate::presets::Quality::Balanced);
        let legacy = candidate_recipe_identity(&state).unwrap();
        // The BA scope only selects future reconstruction work. Adding it
        // must not invalidate an already verified training/publication result.
        state.ba_policy_version = Some(crate::pipeline::ba_policy::VERSION);
        assert_eq!(legacy, candidate_recipe_identity(&state).unwrap());
        state.ba_policy_version = None;
        let encoded = serde_json::to_value(&state).unwrap();
        assert!(encoded["brushTraining"].get("executionOptions").is_none());
        let roundtrip: PipelineStateFile = serde_json::from_value(encoded).unwrap();
        assert_eq!(legacy, candidate_recipe_identity(&roundtrip).unwrap());
        state.brush_training.execution_options = Some(Default::default());
        let modern = candidate_recipe_identity(&state).unwrap();
        assert_ne!(legacy, modern);
        state
            .brush_training
            .execution_options
            .as_mut()
            .unwrap()
            .alpha = crate::project::metadata::AlphaInterpretation::Transparent;
        assert_ne!(modern, candidate_recipe_identity(&state).unwrap());
    }

    #[tokio::test]
    async fn saved_legacy_candidate_can_publish_without_upstream_but_not_with_changed_recipe() {
        let mut fixture = fixture(false);
        fixture.state.inputs = None;
        fixture.state.generation_policy = None;
        fixture.state.brush_training.legacy_dataset_identity =
            Some("verified-old-training-pixels".into());
        fixture.state.brush_training.legacy_initialization_sha256 =
            Some("verified-old-init".into());
        fixture.state.brush_training.resolved = Some(
            crate::presets::resolve_brush_training_preset(Quality::Balanced, false, None, 4, 1),
        );
        let info = inspect_gaussian_ply(&fixture.candidate).unwrap();
        fixture.state.validated_output = Some(crate::project::ProjectOutput {
            final_ply: fixture.paths.project.join("final.ply"),
            file_size: info.file_size,
            splat_count: info.splat_count,
            input_images: 1,
            registered_images: 1,
            registered_ratio: 1.,
            points_3d: 1,
        });
        let token = CancellationToken::new();
        record_saved_legacy_candidate(
            &fixture.paths,
            &mut fixture.state,
            &fixture.candidate,
            Some(Uuid::new_v4()),
            &token,
        )
        .await
        .unwrap();
        std::fs::remove_dir_all(&fixture.paths.frames).unwrap();
        std::fs::remove_dir_all(&fixture.paths.colmap).unwrap();
        std::fs::remove_dir_all(fixture.paths.brush.join("dataset")).unwrap();
        assert_eq!(
            recover_saved_legacy_candidate(&fixture.paths, &fixture.state, &token)
                .await
                .unwrap(),
            Some(fixture.candidate.clone())
        );
        let saved = fixture.state.clone();
        fixture
            .state
            .brush_training
            .resolved
            .as_mut()
            .unwrap()
            .preset
            .total_steps += 1;
        assert!(
            recover_saved_legacy_candidate(&fixture.paths, &fixture.state, &token)
                .await
                .is_err()
        );
        fixture.state = saved.clone();
        fixture.state.brush_training.legacy_initialization_sha256 = Some("other-init".into());
        assert!(
            recover_saved_legacy_candidate(&fixture.paths, &fixture.state, &token)
                .await
                .is_err()
        );
        fixture.state = saved;
        std::fs::write(&fixture.candidate, b"corrupt").unwrap();
        assert!(
            recover_saved_legacy_candidate(&fixture.paths, &fixture.state, &token)
                .await
                .unwrap()
                .is_none()
        );
        fixture.state.brush_complete = false;
        assert!(
            recover_saved_legacy_candidate(&fixture.paths, &fixture.state, &token)
                .await
                .unwrap()
                .is_none()
        );
    }
    use crate::{
        pipeline::inputs::*,
        pipeline::reshoot::{
            CameraRelation, ModelCondition, ReshootOptions, ReshootPlan, ReshootRecipe,
            RECIPE_VERSION,
        },
        presets::Quality,
        project::{ImageMatchingStrategy, PipelineStateFile, ReshootState},
        video::panorama::VideoProjection,
    };
    use std::io::Write;

    struct Fixture {
        _root: tempfile::TempDir,
        paths: ProjectPaths,
        state: PipelineStateFile,
        candidate: PathBuf,
    }

    fn fixture(reshoot: bool) -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(Uuid::new_v4(), root.path().join("project"));
        for directory in [
            &paths.frames,
            &paths.colmap_frames,
            &paths.masks,
            &paths.colmap,
            &paths.brush,
        ] {
            std::fs::create_dir_all(directory).unwrap();
        }
        let images = crate::reconstruction::direct_input::tests::fixture(
            &root.path().join("image-fixture"),
            false,
        );
        for destination in [&paths.frames, &paths.colmap_frames] {
            std::fs::copy(
                images.images.join("normal.png"),
                destination.join("normal.png"),
            )
            .unwrap();
        }
        let (width, height) = image::image_dimensions(paths.frames.join("normal.png")).unwrap();
        create_database(&paths.colmap.join("database.db"));
        create_model(&paths.colmap.join("authoritative"), width, height);
        let dataset = paths.brush.join("dataset");
        std::fs::create_dir_all(dataset.join("images")).unwrap();
        std::fs::copy(
            paths.frames.join("normal.png"),
            dataset.join("images/normal.png"),
        )
        .unwrap();
        create_model(&dataset.join("sparse/0"), width, height);
        std::fs::write(
            dataset.join("inputs-dataset.json"),
            serde_json::to_vec(&serde_json::json!([
                "dataset-key",
                {
                    "path": dataset,
                    "has_erp": false,
                    "max_edge": width.max(height),
                    "reference": "normal.png"
                }
            ]))
            .unwrap(),
        )
        .unwrap();
        let candidate = paths.brush.join("collection-training-0/final.ply.tmp");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        write_ply(&candidate);

        let mut checkpoint = InputCheckpoint::new(GenerationInputs::Materials { items: vec![] });
        checkpoint.prepared = true;
        checkpoint.reference_image = Some("normal.png".into());
        checkpoint.sources.push(SourceCheckpoint {
            id: "source".into(),
            identity: "identity".into(),
            ordinary_image_preparation_version: None,
            camera_ids: vec![],
            camera_assignment: None,
            projection: crate::video::panorama::VideoProjection::Perspective,
            names: vec!["normal.png".into()],
            originals: vec![],
            video: None,
            image_sequence: None,
            dimensions: vec![],
            working_dimensions: None,
            frames: None,
            panorama_records: vec![],
            adopted: 1,
            bridge_frames: vec![],
        });
        let mut state = PipelineStateFile::created(Quality::Balanced);
        state.inputs = Some(checkpoint);
        state.features_complete = true;
        state.matching_complete = true;
        state.reconstruction_complete = true;
        state.brush_complete = true;
        state.stage = PipelineStage::TrainingSplats;

        if reshoot {
            std::fs::write(paths.masks.join("normal.png.png"), b"mask").unwrap();
            std::fs::copy(
                paths.colmap.join("database.db"),
                paths.colmap.join("base-database.db"),
            )
            .unwrap();
            create_model(&paths.colmap.join("base-model"), width, height);
            state.reshoot = Some(ReshootState {
                source_snapshot_complete: true,
                ..Default::default()
            });
        }
        record_collection_artifacts(&paths, &mut state, Some(Uuid::new_v4())).unwrap();
        Fixture {
            _root: root,
            paths,
            state,
            candidate,
        }
    }

    fn create_database(path: &Path) {
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE cameras(camera_id INTEGER PRIMARY KEY,model INTEGER,width INTEGER,height INTEGER,params BLOB,prior_focal_length INTEGER);\
                 CREATE TABLE images(image_id INTEGER PRIMARY KEY,name TEXT UNIQUE,camera_id INTEGER);\
                 CREATE TABLE keypoints(image_id INTEGER PRIMARY KEY,rows INTEGER,cols INTEGER,data BLOB);\
                 CREATE TABLE descriptors(image_id INTEGER PRIMARY KEY,rows INTEGER,cols INTEGER,data BLOB);\
                 CREATE TABLE matches(pair_id INTEGER PRIMARY KEY,rows INTEGER,cols INTEGER,data BLOB);\
                 CREATE TABLE two_view_geometries(pair_id INTEGER PRIMARY KEY,rows INTEGER,cols INTEGER,data BLOB);\
                 INSERT INTO cameras VALUES(1,1,4,4,X'0000000000000000',1);\
                 INSERT INTO images VALUES(1,'normal.png',1);",
            )
            .unwrap();
        connection
            .execute("INSERT INTO keypoints VALUES(1,1,2,?1)", [vec![0_u8; 8]])
            .unwrap();
        connection
            .execute(
                "INSERT INTO descriptors VALUES(1,1,128,?1)",
                [vec![0_u8; 128]],
            )
            .unwrap();
    }

    fn create_model(root: &Path, width: u32, height: u32) {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(
            root.join("cameras.txt"),
            format!(
                "1 PINHOLE {width} {height} {width} {width} {} {}\n",
                f64::from(width) / 2.,
                f64::from(height) / 2.
            ),
        )
        .unwrap();
        std::fs::write(
            root.join("images.txt"),
            "1 1 0 0 0 0 0 0 1 normal.png\n2 2 1\n",
        )
        .unwrap();
        std::fs::write(root.join("points3D.txt"), "1 0 0 1 255 255 255 0.1 1 0\n").unwrap();
    }

    fn write_ply(path: &Path) {
        let mut file = File::create(path).unwrap();
        writeln!(file, "ply").unwrap();
        writeln!(file, "format binary_little_endian 1.0").unwrap();
        writeln!(file, "element vertex 1").unwrap();
        for name in [
            "x", "y", "z", "f_dc_0", "f_dc_1", "f_dc_2", "opacity", "scale_0", "scale_1",
            "scale_2", "rot_0", "rot_1", "rot_2", "rot_3",
        ] {
            writeln!(file, "property float {name}").unwrap();
        }
        writeln!(file, "end_header").unwrap();
        file.write_all(&[0_u8; 14 * 4]).unwrap();
    }

    fn legacy_v2_combined_fixture() -> Fixture {
        let mut fixture = fixture(false);
        let paths = &fixture.paths;
        let old_portrait = "reshoot_previous_0000000000.png";
        let new_name = "reshoot_current_0000000000.png";
        for directory in [&paths.frames, &paths.colmap_frames] {
            image::RgbImage::from_pixel(64, 48, image::Rgb([1, 2, 3]))
                .save(directory.join("normal.png"))
                .unwrap();
            image::RgbImage::from_pixel(40, 72, image::Rgb([4, 5, 6]))
                .save(directory.join(old_portrait))
                .unwrap();
            image::RgbImage::from_pixel(64, 48, image::Rgb([7, 8, 9]))
                .save(directory.join(new_name))
                .unwrap();
        }
        let connection = Connection::open(paths.colmap.join("database.db")).unwrap();
        connection.execute_batch("UPDATE cameras SET width=64,height=48 WHERE camera_id=1;
            INSERT INTO cameras VALUES(2,2,40,72,X'0000000000000000',1);
            INSERT INTO images VALUES(2,'reshoot_previous_0000000000.png',2),(3,'reshoot_current_0000000000.png',1);").unwrap();
        for (id, params) in [
            (1, vec![50_f64, 50., 32., 24.]),
            (2, vec![43_f64, 20., 36., 0.001]),
        ] {
            connection
                .execute(
                    "UPDATE cameras SET params=?1 WHERE camera_id=?2",
                    rusqlite::params![
                        params
                            .into_iter()
                            .flat_map(f64::to_le_bytes)
                            .collect::<Vec<_>>(),
                        id
                    ],
                )
                .unwrap();
        }
        for (id, xy) in [(1, 2_f32), (2, 4_f32), (3, 6_f32)] {
            let data = [xy, xy]
                .into_iter()
                .flat_map(f32::to_le_bytes)
                .collect::<Vec<_>>();
            connection
                .execute(
                    "INSERT OR REPLACE INTO keypoints VALUES(?1,1,2,?2)",
                    rusqlite::params![id, data],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT OR REPLACE INTO descriptors VALUES(?1,1,128,?2)",
                    rusqlite::params![id, vec![0_u8; 128]],
                )
                .unwrap();
        }
        drop(connection);
        for (root, include_new) in [
            (paths.colmap.join("base-model"), false),
            (paths.colmap.join("sparse/0"), true),
        ] {
            std::fs::create_dir_all(&root).unwrap();
            // Focal lengths deliberately differ from the DB's initial values:
            // refined intrinsics must not invalidate unchanged feature pixels.
            std::fs::write(
                root.join("cameras.txt"),
                "1 PINHOLE 64 48 55 55 32 24\n2 SIMPLE_RADIAL 40 72 45 20 36 0.002\n",
            )
            .unwrap();
            let mut images = format!("1 1 0 0 0 0 0 0 1 normal.png\n2 2 100\n2 1 0 0 0 0 0 0 2 {old_portrait}\n4 4 100\n");
            let mut point = "100 0 0 5 128 128 128 0.1 1 0 2 0".to_owned();
            if include_new {
                images.push_str(&format!("3 1 0 0 0 0 0 0 1 {new_name}\n6 6 100\n"));
                point.push_str(" 3 0");
            }
            std::fs::write(root.join("images.txt"), images).unwrap();
            std::fs::write(root.join("points3D.txt"), format!("{point}\n")).unwrap();
        }
        let initial = ReshootPlan::resolve(ModelCondition::Good, true, true);
        let actual = ReshootPlan::resolve(ModelCondition::Appearance, true, true);
        fixture.state.inputs = None;
        fixture.state.stage = PipelineStage::TrainingSplats;
        fixture.state.frames = Some(crate::project::FrameState {
            extracted_frames: Some(3),
            colmap_input_version: Some(crate::project::COLMAP_INPUT_VERSION),
            ..Default::default()
        });
        fixture.state.reshoot = Some(ReshootState {
            recipe: Some(ReshootRecipe {
                version: 2,
                normal_parameters_version: None,
                camera_assignment: None,
                options: ReshootOptions {
                    condition: ModelCondition::Good,
                    camera_relation: Some(CameraRelation::Unsure),
                    allow_fallback: true,
                    source_identity: "saved-source-identity".into(),
                    input_identity: "saved-input-identity".into(),
                    projection: VideoProjection::Perspective,
                    image_matching_strategy: ImageMatchingStrategy::Sequential,
                },
                initial,
                actual,
                batch_id: "current".into(),
                reconstruction_fallbacks: 0,
                training_fallbacks: 1,
                expanded_matching: true,
                source_names: vec!["normal.png".into(), old_portrait.into()],
                new_names: vec![new_name.into()],
                warm_parameters: None,
                init_sha256: Some("saved-initialization".into()),
                init_georeference: None,
                stored_input_identity: Some("saved-input".into()),
                panorama_frames: vec![],
            }),
            source_snapshot_complete: true,
            supplemental_frames_complete: true,
            supplemental_features_complete: true,
            incremental_matching_complete: true,
            incremental_reconstruction_complete: true,
            source_image_count: 2,
            reshoot_frame_count: 1,
            colmap_input_version: Some(crate::project::COLMAP_INPUT_VERSION),
            ..Default::default()
        });
        fixture.state.brush_complete = false;
        let initial = crate::presets::resolve_brush_training_preset(
            Quality::Balanced,
            true,
            Some(8192),
            72,
            100,
        );
        let mut resolved = initial;
        resolved.preset.max_splats = Some(800);
        fixture.state.brush_training.initial_resolved = Some(initial);
        fixture.state.brush_training.resolved = Some(resolved);
        fixture.state.brush_training.oom_retry_used = true;
        fixture.state.brush_training.oom_retry_started = true;
        fixture.state.brush_training.oom_retry_source_count = Some(1000);
        fixture
    }

    fn legacy_v2_recipe_bytes(state: &PipelineStateFile) -> Vec<u8> {
        serde_json::to_vec(&(
            &state.generation_policy,
            &state.configuration,
            &state.brush_training,
            &state.reshoot.as_ref().unwrap().recipe,
        ))
        .unwrap()
    }

    #[test]
    fn legacy_v2_combined_database_preserves_mixed_camera_work_and_saved_recipe() {
        let mut fixture = legacy_v2_combined_fixture();
        let database = fixture.paths.colmap.join("database.db");
        let before_database = std::fs::read(&database).unwrap();
        let before_recipe = legacy_v2_recipe_bytes(&fixture.state);
        assert!(!fixture.paths.colmap.join("base-database.db").exists());
        assert!(!normalize_reshoot_v2(&fixture.paths, &mut fixture.state).unwrap());
        let checkpoint = fixture.state.reshoot.as_ref().unwrap();
        assert!(checkpoint.source_snapshot_complete);
        assert!(checkpoint.supplemental_frames_complete);
        assert!(checkpoint.supplemental_features_complete);
        assert!(checkpoint.incremental_matching_complete);
        assert!(checkpoint.incremental_reconstruction_complete);
        assert_eq!(fixture.state.stage, PipelineStage::TrainingSplats);
        assert_eq!(legacy_v2_recipe_bytes(&fixture.state), before_recipe);
        assert_eq!(std::fs::read(&database).unwrap(), before_database);
        assert!(!fixture.paths.colmap.join("base-database.db").exists());
        assert!(!normalize_reshoot_v2(&fixture.paths, &mut fixture.state).unwrap());
    }

    #[test]
    fn legacy_v2_combined_database_does_not_promote_partial_features() {
        for keep_new_rows in [false, true] {
            let mut fixture = legacy_v2_combined_fixture();
            if !keep_new_rows {
                Connection::open(fixture.paths.colmap.join("database.db")).unwrap()
                    .execute_batch("DELETE FROM descriptors WHERE image_id=3;DELETE FROM keypoints WHERE image_id=3;DELETE FROM images WHERE image_id=3;").unwrap();
            }
            let checkpoint = fixture.state.reshoot.as_mut().unwrap();
            checkpoint.supplemental_features_complete = false;
            checkpoint.incremental_matching_complete = false;
            checkpoint.incremental_reconstruction_complete = false;
            fixture.state.stage = PipelineStage::ExtractingFeatures;
            let before_recipe = legacy_v2_recipe_bytes(&fixture.state);
            assert!(!normalize_reshoot_v2(&fixture.paths, &mut fixture.state).unwrap());
            let checkpoint = fixture.state.reshoot.as_ref().unwrap();
            assert!(checkpoint.source_snapshot_complete);
            assert!(checkpoint.supplemental_frames_complete);
            assert!(!checkpoint.supplemental_features_complete);
            assert!(!checkpoint.incremental_matching_complete);
            assert!(!checkpoint.incremental_reconstruction_complete);
            assert_eq!(legacy_v2_recipe_bytes(&fixture.state), before_recipe);
        }
    }

    #[test]
    fn legacy_v2_combined_database_accepts_unregistered_source_images_with_zero_features() {
        let mut fixture = legacy_v2_combined_fixture();
        for directory in [&fixture.paths.frames, &fixture.paths.colmap_frames] {
            std::fs::copy(
                directory.join("normal.png"),
                directory.join("unregistered.png"),
            )
            .unwrap();
        }
        let connection = Connection::open(fixture.paths.colmap.join("database.db")).unwrap();
        connection
            .execute_batch(
                "INSERT INTO images VALUES(4,'unregistered.png',1);
            INSERT INTO keypoints VALUES(4,0,2,X'');
            INSERT INTO descriptors VALUES(4,0,128,X'');",
            )
            .unwrap();
        drop(connection);
        let checkpoint = fixture.state.reshoot.as_mut().unwrap();
        checkpoint.source_image_count += 1;
        checkpoint
            .recipe
            .as_mut()
            .unwrap()
            .source_names
            .push("unregistered.png".into());
        fixture.state.frames.as_mut().unwrap().extracted_frames = Some(4);
        assert!(!normalize_reshoot_v2(&fixture.paths, &mut fixture.state).unwrap());
        assert!(
            fixture
                .state
                .reshoot
                .as_ref()
                .unwrap()
                .incremental_reconstruction_complete
        );
    }

    #[test]
    fn legacy_v2_combined_database_rejects_unsafe_names_versions_and_counts() {
        for case in 0..8 {
            let mut fixture = legacy_v2_combined_fixture();
            let checkpoint = fixture.state.reshoot.as_mut().unwrap();
            match case {
                0 => checkpoint.source_image_count += 1,
                1 => checkpoint.reshoot_frame_count += 1,
                2 => checkpoint.recipe.as_mut().unwrap().source_names[0] = "../normal.png".into(),
                3 => checkpoint.recipe.as_mut().unwrap().source_names[1] = "normal.png".into(),
                4 => checkpoint.recipe.as_mut().unwrap().new_names[0] = "normal.png".into(),
                5 => checkpoint.recipe.as_mut().unwrap().version = 3,
                6 => checkpoint.colmap_input_version = Some(1),
                7 => fixture.state.frames.as_mut().unwrap().extracted_frames = Some(2),
                _ => unreachable!(),
            }
            let before_recipe = legacy_v2_recipe_bytes(&fixture.state);
            assert!(
                normalize_reshoot_v2(&fixture.paths, &mut fixture.state).unwrap(),
                "case {case}"
            );
            assert!(
                !fixture
                    .state
                    .reshoot
                    .as_ref()
                    .unwrap()
                    .source_snapshot_complete,
                "case {case}"
            );
            assert_eq!(legacy_v2_recipe_bytes(&fixture.state), before_recipe);
        }
        let mut fixture = legacy_v2_combined_fixture();
        fixture.state.inputs = Some(InputCheckpoint::new(GenerationInputs::Materials {
            items: vec![],
        }));
        normalize_reshoot_v2(&fixture.paths, &mut fixture.state).unwrap();
        assert!(
            !fixture
                .state
                .reshoot
                .as_ref()
                .unwrap()
                .source_snapshot_complete
        );
    }

    #[test]
    fn legacy_v2_combined_database_rejects_broken_feature_and_camera_correspondence() {
        for sql in [
            "UPDATE images SET name='unlisted.png' WHERE image_id=3",
            "UPDATE images SET image_id=99 WHERE image_id=1;UPDATE keypoints SET image_id=99 WHERE image_id=1;UPDATE descriptors SET image_id=99 WHERE image_id=1",
            "UPDATE cameras SET model=2 WHERE camera_id=1",
            "UPDATE cameras SET width=65 WHERE camera_id=1",
            "UPDATE keypoints SET data=X'0000104100001041' WHERE image_id=1",
            "UPDATE descriptors SET rows=0,data=X'' WHERE image_id=1",
            "UPDATE keypoints SET data=X'00' WHERE image_id=1",
            "INSERT INTO matches VALUES(2147483649,1,2,X'00')",
        ] {
            let mut fixture = legacy_v2_combined_fixture();
            Connection::open(fixture.paths.colmap.join("database.db")).unwrap().execute_batch(sql).unwrap();
            let before_recipe = legacy_v2_recipe_bytes(&fixture.state);
            assert!(normalize_reshoot_v2(&fixture.paths, &mut fixture.state).unwrap(), "{sql}");
            assert!(!fixture.state.reshoot.as_ref().unwrap().source_snapshot_complete, "{sql}");
            assert_eq!(legacy_v2_recipe_bytes(&fixture.state), before_recipe);
        }
    }

    #[test]
    fn legacy_v2_combined_database_rejects_final_model_keypoints_only_downstream() {
        let mut fixture = legacy_v2_combined_fixture();
        let images = fixture.paths.colmap.join("sparse/0/images.txt");
        let text = std::fs::read_to_string(&images).unwrap();
        std::fs::write(images, text.replace("6 6 100", "8 8 100")).unwrap();
        let before_recipe = legacy_v2_recipe_bytes(&fixture.state);
        normalize_reshoot_v2(&fixture.paths, &mut fixture.state).unwrap();
        let checkpoint = fixture.state.reshoot.as_ref().unwrap();
        assert!(checkpoint.source_snapshot_complete);
        assert!(checkpoint.supplemental_frames_complete);
        assert!(checkpoint.supplemental_features_complete);
        assert!(checkpoint.incremental_matching_complete);
        assert!(!checkpoint.incremental_reconstruction_complete);
        assert_eq!(fixture.state.stage, PipelineStage::Reconstructing);
        assert_eq!(legacy_v2_recipe_bytes(&fixture.state), before_recipe);
    }

    #[test]
    fn legacy_v2_existing_bad_base_database_is_never_hidden_by_combined_evidence() {
        let mut fixture = legacy_v2_combined_fixture();
        std::fs::write(
            fixture.paths.colmap.join("base-database.db"),
            b"broken sqlite",
        )
        .unwrap();
        normalize_reshoot_v2(&fixture.paths, &mut fixture.state).unwrap();
        assert!(
            !fixture
                .state
                .reshoot
                .as_ref()
                .unwrap()
                .source_snapshot_complete
        );

        let mut fixture = legacy_v2_combined_fixture();
        std::fs::create_dir(fixture.paths.colmap.join("base-database.db")).unwrap();
        let before = serde_json::to_vec(&fixture.state).unwrap();
        assert!(normalize_reshoot_v2(&fixture.paths, &mut fixture.state).is_err());
        assert_eq!(serde_json::to_vec(&fixture.state).unwrap(), before);
    }

    #[test]
    fn legacy_v2_combined_database_requires_checkpointed_sqlite_and_keeps_candidate() {
        let mut fixture = legacy_v2_combined_fixture();
        let candidate = fixture.paths.brush.join("attempt-0-1/final.ply.tmp");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        write_ply(&candidate);
        fixture.state.brush_complete = true;
        let before = std::fs::read(&candidate).unwrap();
        normalize_reshoot_v2(&fixture.paths, &mut fixture.state).unwrap();
        assert!(fixture.state.brush_complete);
        assert_eq!(std::fs::read(&candidate).unwrap(), before);
        std::fs::write(
            fixture.paths.colmap.join("database.db-wal"),
            b"uncheckpointed",
        )
        .unwrap();
        let saved_state = serde_json::to_value(&fixture.state).unwrap();
        let error = normalize_reshoot_v2(&fixture.paths, &mut fixture.state).unwrap_err();
        assert_eq!(
            error.io_error().unwrap().kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert_eq!(
            serde_json::to_value(&fixture.state).unwrap(),
            saved_state,
            "pending writes require recovery, not a corrupt-cache downgrade"
        );
        assert_eq!(std::fs::read(&candidate).unwrap(), before);
    }

    #[test]
    fn live_stamps_detect_same_named_replacement_with_restored_size_and_mtime() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("asset");
        let replacement = root.path().join("replacement");
        std::fs::write(&path, b"before").unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        let cancel = CancellationToken::new();
        let before = file_stamps(std::slice::from_ref(&path), &cancel).unwrap();
        // Create the replacement while the original still exists so a native
        // file ID cannot be recycled immediately from the old allocation.
        std::fs::write(&replacement, b"after!").unwrap();
        File::options()
            .write(true)
            .open(&replacement)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        let after = file_stamps(std::slice::from_ref(&path), &cancel).unwrap();
        assert_eq!(before[0].length, after[0].length);
        assert_eq!(before[0].modified, after[0].modified);
        assert!(before != after || !reliable_stamps(&before));
    }

    #[test]
    fn live_cache_rechecks_hard_link_writes_even_when_size_and_mtime_are_restored() {
        use std::io::Write;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("asset");
        let alias = root.path().join("alias");
        std::fs::write(&path, b"before").unwrap();
        std::fs::hard_link(&path, &alias).unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        let cancel = CancellationToken::new();
        let mut cache = CaptureCache::default();
        let captures = std::cell::Cell::new(0_u64);
        let capture = || {
            captures.set(captures.get() + 1);
            make_manifest("run", "upstream", captures.get(), None, vec![])
        };
        cache
            .stage(
                PREPARED,
                vec![],
                std::slice::from_ref(&path),
                &cancel,
                capture,
            )
            .unwrap();
        for iteration in 0..16 {
            {
                let mut writer = File::options().write(true).open(&alias).unwrap();
                writer
                    .write_all(format!("{iteration:06}").as_bytes())
                    .unwrap();
                writer.set_modified(modified).unwrap();
            }
            assert_eq!(std::fs::metadata(&path).unwrap().len(), 6);
            assert_eq!(
                std::fs::metadata(&path).unwrap().modified().unwrap(),
                modified
            );
            cache
                .stage(
                    PREPARED,
                    vec![],
                    std::slice::from_ref(&path),
                    &cancel,
                    capture,
                )
                .unwrap();
            assert_eq!(captures.get(), iteration + 2);
        }
    }

    #[test]
    fn a_platform_without_reliable_change_evidence_cannot_reuse_a_live_stamp() {
        assert!(!reliable_stamps(&[]));
        assert!(!reliable_stamps(&[FileStamp {
            path: "asset".into(),
            canonical_path: "asset".into(),
            directory: false,
            length: 10,
            modified: std::time::UNIX_EPOCH,
            change_identity: None,
        }]));
    }

    #[test]
    fn file_usn_records_require_the_requested_version_identity_and_positive_sequence() {
        let mut record = [0_u8; 64];
        record[..4].copy_from_slice(&64_u32.to_le_bytes());
        record[4..6].copy_from_slice(&2_u16.to_le_bytes());
        record[8..16].copy_from_slice(&77_u64.to_le_bytes());
        record[24..32].copy_from_slice(&99_i64.to_le_bytes());
        assert_eq!(file_usn_record(&record, 64, 77), Some(99));
        assert!(file_usn_record(&record, 59, 77).is_none());
        assert!(file_usn_record(&record, 65, 77).is_none());
        assert!(file_usn_record(&record, 64, 78).is_none());
        let original = record;
        for major in [0_u16, 3] {
            record = original;
            record[4..6].copy_from_slice(&major.to_le_bytes());
            assert!(file_usn_record(&record, 64, 77).is_none());
        }
        for sequence in [0_i64, -1] {
            record = original;
            record[24..32].copy_from_slice(&sequence.to_le_bytes());
            assert!(file_usn_record(&record, 64, 77).is_none());
        }
        record = original;
        record[..4].copy_from_slice(&65_u32.to_le_bytes());
        assert!(file_usn_record(&record, 64, 77).is_none());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_database_writer_is_rejected_with_safe_context_without_replacing_state() {
        let mut fixture = fixture(false);
        let recorder = ArtifactRecorder::default();
        let cancel = CancellationToken::new();
        let run = Some(Uuid::new_v4());
        recorder
            .record(&fixture.paths, &mut fixture.state, run, &cancel)
            .await
            .unwrap();
        let before = serde_json::to_value(&fixture.state).unwrap();
        let database = fixture.paths.colmap.join("database.db");
        let writer = File::options().write(true).open(&database).unwrap();
        let error = recorder
            .record(&fixture.paths, &mut fixture.state, run, &cancel)
            .await
            .unwrap_err();
        assert_eq!(error.io_error().unwrap().raw_os_error(), Some(32));
        let detail = error.to_string();
        assert!(detail.contains("artifact_validation_operation=read_change_identity"));
        assert!(detail.contains("path_role=colmap_database"));
        assert!(!detail.contains(&fixture.paths.project.to_string_lossy().to_string()));
        assert!(!invalid_artifact_error(&error));
        assert_eq!(serde_json::to_value(&fixture.state).unwrap(), before);
        drop(writer);
        recorder
            .record(&fixture.paths, &mut fixture.state, run, &cancel)
            .await
            .unwrap();
    }

    #[test]
    fn readonly_database_statements_do_not_prevent_strict_validation() {
        let fixture = fixture(false);
        let database = fixture.paths.colmap.join("database.db");
        let connection = open_immutable_database(&database).unwrap();
        let mut statement = connection.prepare("SELECT image_id FROM images").unwrap();
        let mut rows = statement.query([]).unwrap();
        assert!(rows.next().unwrap().is_some());
        let summary = validate_database(&database).unwrap();
        assert_eq!(summary.images.len(), 1);
        assert!(!database_stamps(&database, &CancellationToken::new())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn change_identity_context_uses_only_safe_artifact_roles() {
        for (name, directory, expected) in [
            ("database.db", false, "colmap_database"),
            ("database.db-wal", false, "database_sidecar"),
            ("database.db-journal", false, "database_sidecar"),
            ("private-image.png", false, "artifact_file"),
            ("private-folder", true, "artifact_directory"),
        ] {
            assert_eq!(artifact_path_role(Path::new(name), directory), expected);
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn an_open_image_writer_keeps_original_io_error_and_never_replaces_saved_state() {
        use std::io::Write;
        let mut fixture = fixture(false);
        let recorder = ArtifactRecorder::default();
        let run = Some(Uuid::new_v4());
        let cancel = CancellationToken::new();
        recorder
            .record(&fixture.paths, &mut fixture.state, run, &cancel)
            .await
            .unwrap();
        let frame = fixture.paths.frames.join("normal.png");
        let pixels = std::fs::read(&frame).unwrap();
        let modified = std::fs::metadata(&frame).unwrap().modified().unwrap();
        let native_before = file_stamps(std::slice::from_ref(&frame), &cancel).unwrap();
        let before = serde_json::to_value(&fixture.state).unwrap();
        let captures = recorder.cache.lock().unwrap().stage_captures[PREPARED];
        let mut writer = File::options().write(true).open(&frame).unwrap();
        writer.write_all(&pixels).unwrap();
        writer.set_modified(modified).unwrap();
        let error = recorder
            .record(&fixture.paths, &mut fixture.state, run, &cancel)
            .await
            .unwrap_err();
        assert_eq!(error.io_error().unwrap().raw_os_error(), Some(32));
        assert!(!invalid_artifact_error(&error));
        assert_eq!(serde_json::to_value(&fixture.state).unwrap(), before);
        drop(writer);
        let native_after = file_stamps(std::slice::from_ref(&frame), &cancel).unwrap();
        if let (Some(before), Some(after)) = (
            &native_before[0].change_identity,
            &native_after[0].change_identity,
        ) {
            assert_ne!(before.usn, after.usn);
        }
        recorder
            .record(&fixture.paths, &mut fixture.state, run, &cancel)
            .await
            .unwrap();
        assert_eq!(
            recorder.cache.lock().unwrap().stage_captures[PREPARED],
            captures + 1
        );
    }

    #[tokio::test]
    async fn pending_database_sidecars_cannot_bypass_a_live_cache_or_replace_saved_evidence() {
        for suffix in ["-wal", "-journal"] {
            for directory in [false, true] {
                let mut fixture = fixture(false);
                let recorder = ArtifactRecorder::default();
                let run = Some(Uuid::new_v4());
                let cancel = CancellationToken::new();
                recorder
                    .record(&fixture.paths, &mut fixture.state, run, &cancel)
                    .await
                    .unwrap();
                let before = serde_json::to_value(&fixture.state).unwrap();
                let sidecar = fixture.paths.colmap.join(format!("database.db{suffix}"));
                if directory {
                    std::fs::create_dir(&sidecar).unwrap();
                } else {
                    std::fs::write(&sidecar, b"pending writes").unwrap();
                }
                let error = recorder
                    .record(&fixture.paths, &mut fixture.state, run, &cancel)
                    .await
                    .unwrap_err();
                assert_eq!(
                    error.io_error().unwrap().kind(),
                    std::io::ErrorKind::WouldBlock
                );
                assert!(!invalid_artifact_error(&error));
                assert_eq!(serde_json::to_value(&fixture.state).unwrap(), before);
            }
        }
    }

    #[tokio::test]
    async fn strict_resume_treats_uncheckpointed_database_as_busy_not_corrupt() {
        let mut fixture = fixture(false);
        fixture.state.brush_complete = false;
        let before = serde_json::to_value(&fixture.state).unwrap();
        std::fs::write(fixture.paths.colmap.join("database.db-wal"), b"pending").unwrap();
        let error = normalize_collection(&fixture.paths, &mut fixture.state, None)
            .await
            .unwrap_err();
        assert_eq!(
            error.io_error().unwrap().kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert_eq!(serde_json::to_value(&fixture.state).unwrap(), before);
    }

    #[tokio::test]
    async fn empty_checkpointed_database_sidecars_are_allowed_without_changing_manifest_identity() {
        let mut fixture = fixture(false);
        let recorder = ArtifactRecorder::default();
        let run = Some(Uuid::new_v4());
        let cancel = CancellationToken::new();
        recorder
            .record(&fixture.paths, &mut fixture.state, run, &cancel)
            .await
            .unwrap();
        let before = fixture.state.artifact_manifests.clone();
        for suffix in ["-wal", "-journal"] {
            std::fs::write(
                fixture.paths.colmap.join(format!("database.db{suffix}")),
                [],
            )
            .unwrap();
        }
        recorder
            .record(&fixture.paths, &mut fixture.state, run, &cancel)
            .await
            .unwrap();
        assert_eq!(fixture.state.artifact_manifests, before);
    }

    #[tokio::test]
    async fn cold_persist_journal_does_not_invalidate_saved_sfm() {
        let mut fixture = fixture(false);
        fixture.state.brush_complete = false;
        std::fs::write(
            fixture.paths.colmap.join("database.db-journal"),
            [0_u8; 1024],
        )
        .unwrap();
        assert_eq!(
            recover_working_database(&fixture.paths, &fixture.state, &CancellationToken::new())
                .unwrap(),
            WorkingDatabaseRecovery::Reusable
        );
        normalize_collection(&fixture.paths, &mut fixture.state, None)
            .await
            .unwrap();
        assert!(fixture.state.matching_complete && fixture.state.reconstruction_complete);
        assert!(fixture.state.retry_cache_pending.is_empty());
    }

    #[test]
    fn cold_persist_journal_with_an_active_writer_is_busy_not_reusable() {
        let fixture = fixture(false);
        let database = fixture.paths.colmap.join("database.db");
        let writer = Connection::open(database).unwrap();
        writer
            .execute_batch("PRAGMA journal_mode=PERSIST; PRAGMA user_version=3; BEGIN IMMEDIATE")
            .unwrap();
        let journal = fixture.paths.colmap.join("database.db-journal");
        let length = std::fs::metadata(&journal).unwrap().len();
        assert!(length > 0 && journal_is_cold(&journal, length).unwrap());
        let error =
            recover_working_database(&fixture.paths, &fixture.state, &CancellationToken::new())
                .unwrap_err();
        assert_eq!(
            error.io_error().unwrap().kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert!(!writer.is_autocommit());
        assert!(fixture.state.features_complete && fixture.state.matching_complete);
        writer.execute_batch("ROLLBACK").unwrap();
    }

    #[tokio::test]
    async fn native_wal_recovery_preserves_complete_semantic_cache_identity() {
        let mut fixture = fixture(false);
        fixture.state.brush_complete = false;
        let before = fixture.state.artifact_manifests.clone();
        super::super::sqlite_recovery::tests::crash_fixture(
            &fixture.paths.colmap.join("database.db"),
            "layout",
        );
        assert_eq!(
            recover_working_database(&fixture.paths, &fixture.state, &CancellationToken::new())
                .unwrap(),
            WorkingDatabaseRecovery::Reusable
        );
        normalize_collection(&fixture.paths, &mut fixture.state, None)
            .await
            .unwrap();
        assert!(
            fixture.state.features_complete
                && fixture.state.matching_complete
                && fixture.state.reconstruction_complete
        );
        assert_eq!(
            fixture.state.artifact_manifests.entries[MATCHING],
            before.entries[MATCHING]
        );
        assert_eq!(
            fixture.state.artifact_manifests.entries[RECONSTRUCTION],
            before.entries[RECONSTRUCTION]
        );
        assert!(fixture.state.retry_cache_pending.is_empty());
    }

    #[test]
    fn old_matching_summary_cannot_rebase_over_changed_database_bytes() {
        let mut fixture = fixture(false);
        fixture.state.brush_complete = false;
        fixture
            .state
            .artifact_manifests
            .entries
            .get_mut(MATCHING)
            .unwrap()
            .semantic_identity = Some("old-raw-matches-digest".into());
        super::super::sqlite_recovery::tests::crash_fixture(
            &fixture.paths.colmap.join("database.db"),
            "layout",
        );
        assert_eq!(
            recover_working_database(&fixture.paths, &fixture.state, &CancellationToken::new())
                .unwrap(),
            WorkingDatabaseRecovery::RebuildRequired {
                reason: "sqlite_recovery_matching_changed"
            }
        );
        // The preflight only reports; no cache or completion bit is mutated.
        assert!(
            fixture.state.matching_complete && fixture.paths.colmap.join("authoritative").exists()
        );
    }

    #[tokio::test]
    async fn old_matching_summary_accepts_unchanged_content_without_promoting_new_evidence() {
        let mut fixture = fixture(false);
        fixture.state.brush_complete = false;
        fixture
            .state
            .artifact_manifests
            .entries
            .get_mut(MATCHING)
            .unwrap()
            .semantic_identity = Some("old-raw-matches-digest".into());
        let saved = fixture.state.artifact_manifests.entries[MATCHING].clone();
        normalize_collection(&fixture.paths, &mut fixture.state, None)
            .await
            .unwrap();
        assert!(fixture.state.matching_complete && fixture.state.reconstruction_complete);
        assert_eq!(fixture.state.artifact_manifests.entries[MATCHING], saved);
    }

    #[tokio::test]
    async fn camera_or_verified_geometry_changes_require_whole_database_archive() {
        for change in ["UPDATE cameras SET params=X'000000000000F03F'", "ALTER TABLE two_view_geometries ADD COLUMN config INTEGER DEFAULT 2", "CREATE TABLE pose_priors(id INTEGER PRIMARY KEY,position BLOB); INSERT INTO pose_priors VALUES(1,X'00')"] {
            let mut fixture = fixture(false);
            fixture.state.brush_complete = false;
            fixture.state.inputs.as_mut().unwrap().bridge_used = true;
            fixture.state.inputs.as_mut().unwrap().expanded_matching = true;
            let db = Connection::open(fixture.paths.colmap.join("database.db")).unwrap();
            db.execute_batch(change).unwrap();
            drop(db);
            normalize_collection(&fixture.paths, &mut fixture.state, None).await.unwrap();
            assert!(!fixture.state.features_complete && !fixture.state.matching_complete && !fixture.state.reconstruction_complete, "{change}");
            assert!(fixture.state.inputs.as_ref().unwrap().prepared);
            assert!(fixture.state.inputs.as_ref().unwrap().bridge_used && fixture.state.inputs.as_ref().unwrap().expanded_matching);
            assert!(fixture.state.database_recovery_pending.is_some());
            assert!(fixture.state.retry_cache_pending.iter().any(|entry| entry == "colmap"));
            assert!(fixture.paths.colmap.join("database.db").exists());
            assert!(fixture.paths.frames.join("normal.png").exists());
        }
    }

    #[test]
    fn semantic_geometry_hash_covers_verified_pairs_and_rejects_bad_feature_indices() {
        let fixture = fixture(false);
        let db = Connection::open(fixture.paths.colmap.join("database.db")).unwrap();
        db.execute_batch("INSERT INTO images VALUES(2,'second.png',1); INSERT INTO keypoints SELECT 2,rows,cols,data FROM keypoints WHERE image_id=1; INSERT INTO descriptors SELECT 2,rows,cols,data FROM descriptors WHERE image_id=1; ALTER TABLE two_view_geometries ADD COLUMN config INTEGER; ALTER TABLE two_view_geometries ADD COLUMN qvec BLOB; INSERT INTO two_view_geometries VALUES(2147483649,1,2,X'0000000000000000',2,X'000000000000F03F');").unwrap();
        let first = read_database_summary(&db, &CancellationToken::new()).unwrap();
        db.execute_batch("UPDATE two_view_geometries SET config=3")
            .unwrap();
        let second = read_database_summary(&db, &CancellationToken::new()).unwrap();
        assert_eq!(first.feature_identity, second.feature_identity);
        assert_ne!(first.matching_identity, second.matching_identity);
        db.execute_batch("UPDATE two_view_geometries SET qvec=X'0000000000000040'")
            .unwrap();
        let third = read_database_summary(&db, &CancellationToken::new()).unwrap();
        assert_ne!(second.matching_identity, third.matching_identity);
        db.execute_batch("UPDATE two_view_geometries SET data=X'0100000000000000'")
            .unwrap();
        assert!(read_database_summary(&db, &CancellationToken::new()).is_err());
    }

    #[test]
    fn recovery_reports_corruption_but_does_not_move_or_replace_database_family() {
        let fixture = fixture(false);
        let database = fixture.paths.colmap.join("database.db");
        let wal = fixture.paths.colmap.join("database.db-wal");
        std::fs::write(&database, b"invalid sqlite database").unwrap();
        std::fs::write(&wal, b"unrecoverable WAL").unwrap();
        assert_eq!(
            recover_working_database(&fixture.paths, &fixture.state, &CancellationToken::new())
                .unwrap(),
            WorkingDatabaseRecovery::RebuildRequired {
                reason: "sqlite_recovery_invalid"
            }
        );
        assert!(database.exists());
        assert!(fixture.state.features_complete);
    }

    #[test]
    fn legacy_clean_database_is_not_forced_into_new_manifest_policy() {
        let mut fixture = fixture(false);
        fixture.state.inputs = None;
        fixture.state.artifact_manifests = ArtifactManifestSet::default();
        assert_eq!(
            recover_working_database(&fixture.paths, &fixture.state, &CancellationToken::new())
                .unwrap(),
            WorkingDatabaseRecovery::NotNeeded
        );
    }

    #[test]
    fn recovery_never_writes_a_database_hard_linked_to_source_snapshot() {
        let fixture = fixture(false);
        let database = fixture.paths.colmap.join("database.db");
        let source = fixture.paths.work.join("immutable-source.db");
        std::fs::hard_link(&database, &source).unwrap();
        let before = std::fs::read(&source).unwrap();
        std::fs::write(fixture.paths.colmap.join("database.db-wal"), b"pending WAL").unwrap();
        let error =
            recover_working_database(&fixture.paths, &fixture.state, &CancellationToken::new())
                .unwrap_err();
        assert_eq!(
            error.io_error().unwrap().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert_eq!(std::fs::read(&source).unwrap(), before);
        assert_eq!(
            std::fs::read(fixture.paths.colmap.join("database.db-wal")).unwrap(),
            b"pending WAL"
        );
    }

    #[test]
    fn recovery_rejects_a_file_changed_after_releasing_the_sqlite_lock() {
        let fixture = fixture(false);
        let path = fixture.paths.colmap.join("database.db");
        let cancel = CancellationToken::new();
        let locked = hash_file_cancellable(&path, &cancel).unwrap();
        let db = Connection::open(&path).unwrap();
        db.execute_batch("PRAGMA user_version=123").unwrap();
        drop(db);
        let changed =
            describe_file_cancellable(&fixture.paths.project, &path, true, &cancel).unwrap();
        let error = ensure_recovered_file_identity(&locked, &changed).unwrap_err();
        assert_eq!(
            error.io_error().unwrap().kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert!(!invalid_artifact_error(&error));
    }

    #[tokio::test]
    async fn recorder_reuses_verified_upstream_and_only_captures_new_dataset() {
        let mut fixture = fixture(false);
        fixture.state.brush_complete = false;
        fixture.state.inputs.as_mut().unwrap().reference_image = None;
        let recorder = ArtifactRecorder::default();
        let run = Some(Uuid::new_v4());
        let cancel = CancellationToken::new();
        recorder
            .record(&fixture.paths, &mut fixture.state, run, &cancel)
            .await
            .unwrap();
        fixture.state.inputs.as_mut().unwrap().reference_image = Some("normal.png".into());
        recorder
            .record(&fixture.paths, &mut fixture.state, run, &cancel)
            .await
            .unwrap();
        recorder
            .record(&fixture.paths, &mut fixture.state, run, &cancel)
            .await
            .unwrap();
        let deep = capture_collection_artifacts(&fixture.paths, &fixture.state, run).unwrap();
        assert_eq!(fixture.state.artifact_manifests, deep);
        let cache = recorder.cache.lock().unwrap();
        assert_eq!(cache.database_captures, 1);
        assert_eq!(cache.stage_captures[PREPARED], 1);
        assert_eq!(cache.stage_captures[RECONSTRUCTION], 1);
        assert_eq!(cache.stage_captures[TRAINING_DATASET], 1);
        assert_eq!(
            deep.entries[FEATURES].files[0],
            deep.entries[MATCHING].files[0]
        );
    }

    #[tokio::test]
    async fn colmap_writers_recheck_their_outputs_without_rescanning_unchanged_prepared_assets() {
        for reshoot in [false, true] {
            let mut fixture = fixture(reshoot);
            let recorder = ArtifactRecorder::default();
            let run = Some(Uuid::new_v4());
            let cancel = CancellationToken::new();
            recorder
                .record(&fixture.paths, &mut fixture.state, run, &cancel)
                .await
                .unwrap();
            let expected = fixture.state.artifact_manifests.clone();
            for (write, expected_database, expected_model, expected_dataset) in [
                (ColmapWrite::Database, 2, 2, 2),
                (ColmapWrite::Reconstruction, 2, 3, 3),
                (ColmapWrite::TrainingDataset, 2, 3, 4),
            ] {
                recorder.invalidate_colmap(write);
                recorder
                    .record(&fixture.paths, &mut fixture.state, run, &cancel)
                    .await
                    .unwrap();
                let (stages, database) = recorder.capture_counts();
                assert_eq!(database, expected_database);
                assert_eq!(stages[PREPARED], 1);
                assert_eq!(stages[RECONSTRUCTION], expected_model);
                assert_eq!(stages[TRAINING_DATASET], expected_dataset);
                assert_eq!(stages[BRUSH_CANDIDATE], expected_dataset);
                assert_eq!(fixture.state.artifact_manifests, expected);
            }
            // An unknown writer still discards all invocation-local evidence.
            recorder.invalidate();
            recorder
                .record(&fixture.paths, &mut fixture.state, run, &cancel)
                .await
                .unwrap();
            assert_eq!(recorder.capture_counts().1, 1);
            assert_eq!(fixture.state.artifact_manifests, expected);
        }
    }

    #[tokio::test]
    async fn retained_prepared_evidence_does_not_hide_a_changed_file_after_colmap() {
        let mut fixture = fixture(false);
        let recorder = ArtifactRecorder::default();
        let run = Some(Uuid::new_v4());
        let cancel = CancellationToken::new();
        recorder
            .record(&fixture.paths, &mut fixture.state, run, &cancel)
            .await
            .unwrap();
        let previous = fixture.state.artifact_manifests.clone();
        recorder.invalidate_colmap(ColmapWrite::Database);
        std::fs::write(fixture.paths.frames.join("normal.png"), b"not an image").unwrap();
        assert!(recorder
            .record(&fixture.paths, &mut fixture.state, run, &cancel)
            .await
            .is_err());
        assert_eq!(fixture.state.artifact_manifests, previous);
    }

    #[tokio::test]
    async fn recorder_never_imports_its_previous_run_or_another_recorders_validation_evidence() {
        let mut fixture = fixture(false);
        let recorder = ArtifactRecorder::default();
        let cancel = CancellationToken::new();
        let first = Some(Uuid::new_v4());
        recorder
            .record(&fixture.paths, &mut fixture.state, first, &cancel)
            .await
            .unwrap();
        let next = Some(Uuid::new_v4());
        recorder
            .record(&fixture.paths, &mut fixture.state, next, &cancel)
            .await
            .unwrap();
        assert_eq!(
            recorder.cache.lock().unwrap().owner.as_ref().unwrap().1,
            next
        );
        let (stages, database) = recorder.capture_counts();
        assert_eq!(stages[PREPARED], 1);
        assert_eq!(database, 1);
        let fresh = ArtifactRecorder::default();
        fresh
            .record(&fixture.paths, &mut fixture.state, next, &cancel)
            .await
            .unwrap();
        assert_eq!(fresh.capture_counts().1, 1);
        assert_eq!(fresh.capture_counts().0[PREPARED], 1);
    }

    #[tokio::test]
    async fn recorder_rechecks_database_when_its_file_identity_changes() {
        let mut fixture = fixture(false);
        let recorder = ArtifactRecorder::default();
        let run = Some(Uuid::new_v4());
        let cancel = CancellationToken::new();
        recorder
            .record(&fixture.paths, &mut fixture.state, run, &cancel)
            .await
            .unwrap();
        let previous = fixture.state.artifact_manifests.entries[FEATURES].clone();
        let path = fixture.paths.colmap.join("database.db");
        let previous_time = std::fs::metadata(&path).unwrap().modified().unwrap();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute("UPDATE descriptors SET data=?1", [vec![1_u8; 128]])
            .unwrap();
        drop(connection);
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(previous_time + std::time::Duration::from_secs(2))
            .unwrap();
        recorder
            .record(&fixture.paths, &mut fixture.state, run, &cancel)
            .await
            .unwrap();
        assert_ne!(
            previous.artifact_identity,
            fixture.state.artifact_manifests.entries[FEATURES].artifact_identity
        );
        let cache = recorder.cache.lock().unwrap();
        assert_eq!(cache.database_captures, 2);
        assert_eq!(cache.stage_captures[PREPARED], 1);
    }

    #[tokio::test]
    async fn known_write_invalidation_rechecks_even_when_timestamp_is_preserved() {
        let mut fixture = fixture(false);
        let recorder = ArtifactRecorder::default();
        let run = Some(Uuid::new_v4());
        let cancel = CancellationToken::new();
        recorder
            .record(&fixture.paths, &mut fixture.state, run, &cancel)
            .await
            .unwrap();
        let previous = fixture.state.artifact_manifests.entries[FEATURES].clone();
        let path = fixture.paths.colmap.join("database.db");
        let previous_metadata = std::fs::metadata(&path).unwrap();
        recorder.invalidate();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute("UPDATE descriptors SET data=?1", [vec![2_u8; 128]])
            .unwrap();
        drop(connection);
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(previous_metadata.modified().unwrap())
            .unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            previous_metadata.len()
        );
        recorder
            .record(&fixture.paths, &mut fixture.state, run, &cancel)
            .await
            .unwrap();
        assert_ne!(
            previous.semantic_identity,
            fixture.state.artifact_manifests.entries[FEATURES].semantic_identity
        );
    }

    #[tokio::test]
    async fn recorder_cancellation_does_not_replace_saved_evidence() {
        let mut fixture = fixture(false);
        let before = fixture.state.artifact_manifests.clone();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let error = ArtifactRecorder::default()
            .record(
                &fixture.paths,
                &mut fixture.state,
                Some(Uuid::new_v4()),
                &cancel,
            )
            .await
            .unwrap_err();
        assert!(matches!(error, SplatError::Cancelled));
        assert_eq!(fixture.state.artifact_manifests, before);
    }

    #[tokio::test]
    async fn normalization_cancel_does_not_downgrade_or_replace_saved_state() {
        let mut fixture = fixture(false);
        std::fs::remove_file(&fixture.candidate).unwrap();
        let before = serde_json::to_vec(&fixture.state).unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let error = normalize_collection_cancellable(
            &fixture.paths,
            &mut fixture.state,
            Some(Uuid::new_v4()),
            &cancel,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, SplatError::Cancelled));
        assert_eq!(serde_json::to_vec(&fixture.state).unwrap(), before);
    }

    #[test]
    fn optional_validation_never_treats_cancellation_as_corrupt_artifact() {
        let cancel = CancellationToken::new();
        assert!(matches!(
            usable_validation::<()>(Err(SplatError::Cancelled), &cancel),
            Err(SplatError::Cancelled)
        ));
        // A cancellation concurrent with an unrelated I/O/validation error also
        // must not turn into None and downgrade the original checkpoint.
        cancel.cancel();
        assert!(matches!(
            usable_validation::<()>(Err(untrusted_artifact()), &cancel),
            Err(SplatError::Cancelled)
        ));
    }

    #[tokio::test]
    async fn fresh_run_never_trusts_previous_recorders_or_persisted_manifests() {
        let mut fixture = fixture(false);
        let recorder = ArtifactRecorder::default();
        let cancel = CancellationToken::new();
        recorder
            .record(
                &fixture.paths,
                &mut fixture.state,
                Some(Uuid::new_v4()),
                &cancel,
            )
            .await
            .unwrap();
        let path = fixture.paths.colmap.join("database.db");
        let previous_time = std::fs::metadata(&path).unwrap().modified().unwrap();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute("UPDATE descriptors SET data=?1", [vec![3_u8; 128]])
            .unwrap();
        drop(connection);
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(previous_time)
            .unwrap();
        // Resume uses the strict normalizer, never the live-run shortcut.
        assert!(
            normalize_collection(&fixture.paths, &mut fixture.state, Some(Uuid::new_v4()))
                .await
                .unwrap()
        );
        assert!(!fixture.state.features_complete);
        assert!(!fixture.state.reconstruction_complete);
        assert!(!fixture.state.brush_complete);
    }

    #[test]
    fn stage_capture_does_not_accept_files_modified_during_validation() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("asset");
        std::fs::write(&path, b"before").unwrap();
        let mut cache = CaptureCache::default();
        let result = cache.stage(
            PREPARED,
            vec![],
            std::slice::from_ref(&path),
            &CancellationToken::new(),
            || {
                std::fs::write(&path, b"changed-size").unwrap();
                make_manifest("run", "upstream", 0, None, vec![])
            },
        );
        assert!(result.is_err());
        assert!(cache.stages.is_empty());
    }

    #[tokio::test]
    async fn same_named_replacement_is_not_promoted_to_a_new_baseline() {
        let mut fixture = fixture(false);
        std::thread::sleep(std::time::Duration::from_millis(20));
        let frame = fixture.paths.frames.join("normal.png");
        let bytes = std::fs::read(&frame).unwrap();
        std::fs::write(&frame, bytes).unwrap();
        assert!(
            normalize_collection(&fixture.paths, &mut fixture.state, Some(Uuid::new_v4()))
                .await
                .unwrap()
        );
        assert!(!fixture.state.inputs.as_ref().unwrap().prepared);
        assert!(!fixture.state.features_complete);
    }

    #[test]
    fn v2_compatibility_normalizer_clears_only_reconstruction_and_training_downstream() {
        let mut fixture = fixture(true);
        create_model(&fixture.paths.colmap.join("sparse/0"), 4, 4);
        let legacy_candidate = fixture.paths.brush.join("attempt-0-0/final.ply.tmp");
        std::fs::create_dir_all(legacy_candidate.parent().unwrap()).unwrap();
        write_ply(&legacy_candidate);
        let plan = ReshootPlan::resolve(ModelCondition::Appearance, true, true);
        fixture.state.inputs = None;
        fixture.state.reshoot = Some(ReshootState {
            recipe: Some(ReshootRecipe {
                version: RECIPE_VERSION,
                normal_parameters_version: Some(1),
                camera_assignment: None,
                options: ReshootOptions {
                    condition: ModelCondition::Appearance,
                    camera_relation: Some(CameraRelation::Same),
                    allow_fallback: true,
                    source_identity: "source".into(),
                    input_identity: "input".into(),
                    projection: VideoProjection::Perspective,
                    image_matching_strategy: ImageMatchingStrategy::Exhaustive,
                },
                initial: plan,
                actual: plan,
                batch_id: "batch".into(),
                reconstruction_fallbacks: 0,
                training_fallbacks: 0,
                expanded_matching: false,
                source_names: vec!["normal.png".into()],
                new_names: vec!["normal.png".into()],
                warm_parameters: None,
                init_sha256: None,
                init_georeference: None,
                stored_input_identity: None,
                panorama_frames: vec![],
            }),
            source_snapshot_complete: true,
            supplemental_frames_complete: true,
            supplemental_features_complete: true,
            incremental_matching_complete: true,
            incremental_reconstruction_complete: true,
            ..Default::default()
        });
        fixture.state.brush_complete = true;

        assert!(!normalize_reshoot_v2(&fixture.paths, &mut fixture.state).unwrap());
        std::fs::remove_dir_all(fixture.paths.colmap.join("sparse")).unwrap();
        assert!(normalize_reshoot_v2(&fixture.paths, &mut fixture.state).unwrap());
        let checkpoint = fixture.state.reshoot.as_ref().unwrap();
        assert!(checkpoint.source_snapshot_complete);
        assert!(checkpoint.supplemental_frames_complete);
        assert!(checkpoint.supplemental_features_complete);
        assert!(checkpoint.incremental_matching_complete);
        assert!(!checkpoint.incremental_reconstruction_complete);
        assert!(!fixture.state.brush_complete);
        assert_eq!(fixture.state.stage, PipelineStage::Reconstructing);
    }

    #[tokio::test]
    async fn missing_prior_manifest_never_promotes_existing_files_to_trusted_cache() {
        let mut fixture = fixture(false);
        fixture.state.artifact_manifests = ArtifactManifestSet::default();
        normalize_collection(&fixture.paths, &mut fixture.state, None)
            .await
            .unwrap();
        assert!(!fixture.state.inputs.as_ref().unwrap().prepared);
        assert!(!fixture.state.features_complete);
        assert!(!fixture.state.matching_complete);
        assert!(!fixture.state.reconstruction_complete);
        assert!(!fixture.state.brush_complete);
    }

    #[tokio::test]
    async fn missing_mask_or_base_database_invalidates_prepared_and_downstream() {
        for remove in ["mask", "database"] {
            let mut fixture = fixture(true);
            if remove == "mask" {
                std::fs::remove_file(fixture.paths.masks.join("normal.png.png")).unwrap();
            } else {
                std::fs::remove_file(fixture.paths.colmap.join("base-database.db")).unwrap();
            }
            normalize_collection(&fixture.paths, &mut fixture.state, None)
                .await
                .unwrap();
            assert!(!fixture.state.inputs.as_ref().unwrap().prepared, "{remove}");
            assert!(!fixture.state.features_complete, "{remove}");
            assert!(!fixture.state.reconstruction_complete, "{remove}");
        }
    }

    #[tokio::test]
    async fn corrupt_database_preserves_prepared_but_invalidates_features_and_downstream() {
        let mut fixture = fixture(false);
        std::fs::write(fixture.paths.colmap.join("database.db"), b"not sqlite").unwrap();
        normalize_collection(&fixture.paths, &mut fixture.state, None)
            .await
            .unwrap();
        assert!(fixture.state.inputs.as_ref().unwrap().prepared);
        assert!(!fixture.state.features_complete);
        assert!(!fixture.state.matching_complete);
        assert!(!fixture.state.reconstruction_complete);
        assert!(!fixture.state.brush_complete);
    }

    #[tokio::test]
    async fn reconstruction_not_backed_by_database_preserves_matching_only() {
        let mut fixture = fixture(false);
        std::fs::write(
            fixture.paths.colmap.join("authoritative/images.txt"),
            "1 1 0 0 0 0 0 0 1 another.png\n2 2 1\n",
        )
        .unwrap();
        normalize_collection(&fixture.paths, &mut fixture.state, None)
            .await
            .unwrap();
        assert!(fixture.state.inputs.as_ref().unwrap().prepared);
        assert!(fixture.state.features_complete);
        assert!(fixture.state.matching_complete);
        assert!(!fixture.state.reconstruction_complete);
        assert!(!fixture.state.brush_complete);
    }

    #[tokio::test]
    async fn dataset_not_yet_prepared_does_not_invalidate_reconstruction() {
        let mut fixture = fixture(false);
        fixture.state.inputs.as_mut().unwrap().reference_image = None;
        fixture.state.brush_complete = false;
        record_collection_artifacts(&fixture.paths, &mut fixture.state, None).unwrap();
        std::fs::remove_dir_all(fixture.paths.brush.join("dataset")).unwrap();
        assert!(
            !normalize_collection(&fixture.paths, &mut fixture.state, None)
                .await
                .unwrap()
        );
        assert!(fixture.state.reconstruction_complete);
        assert!(!fixture
            .state
            .artifact_manifests
            .entries
            .contains_key(TRAINING_DATASET));
    }

    #[tokio::test]
    async fn a_recipe_bound_candidate_can_publish_without_its_disposable_dataset() {
        let mut fixture = fixture(false);
        std::fs::remove_file(fixture.paths.brush.join("dataset/inputs-dataset.json")).unwrap();
        let before = serde_json::to_value(&fixture.state).unwrap();
        assert!(
            !normalize_collection(&fixture.paths, &mut fixture.state, None)
                .await
                .unwrap()
        );
        assert!(fixture.state.inputs.as_ref().unwrap().prepared);
        assert!(fixture.state.features_complete);
        assert!(fixture.state.matching_complete);
        assert!(fixture.state.reconstruction_complete);
        assert!(fixture.state.brush_complete);
        assert!(fixture
            .state
            .artifact_manifests
            .entries
            .contains_key(TRAINING_DATASET));
        assert!(fixture.state.retry_cache_pending.is_empty());
        assert_eq!(serde_json::to_value(&fixture.state).unwrap(), before);
    }

    #[tokio::test]
    async fn validated_recipe_bound_candidate_can_publish_after_all_disposable_upstream_is_missing()
    {
        let mut fixture = fixture(false);
        fixture.state.brush_training.dataset_identity =
            Some("frozen-model-and-pixel-mapping".into());
        let info = inspect_gaussian_ply(&fixture.candidate).unwrap();
        fixture.state.validated_output = Some(crate::project::ProjectOutput {
            final_ply: fixture.paths.project.join("final.ply"),
            file_size: info.file_size,
            splat_count: info.splat_count,
            input_images: 1,
            registered_images: 1,
            registered_ratio: 1.,
            points_3d: 0,
        });
        record_collection_artifacts(&fixture.paths, &mut fixture.state, None).unwrap();
        for path in [
            &fixture.paths.frames,
            &fixture.paths.colmap_frames,
            &fixture.paths.colmap,
            &fixture.paths.brush.join("dataset"),
        ] {
            std::fs::remove_dir_all(path).unwrap();
        }
        let before = serde_json::to_value(&fixture.state).unwrap();
        assert!(
            !normalize_collection(&fixture.paths, &mut fixture.state, None)
                .await
                .unwrap()
        );
        assert!(fixture.state.brush_complete);
        assert_eq!(serde_json::to_value(&fixture.state).unwrap(), before);
        fixture.state.inputs.as_mut().unwrap().sources[0].identity = "changed-source".into();
        normalize_collection(&fixture.paths, &mut fixture.state, None)
            .await
            .unwrap();
        assert!(
            !fixture.state.brush_complete,
            "a byte-identical candidate must still bind to its source recipe"
        );
    }

    #[tokio::test]
    async fn publication_fast_path_proves_candidate_without_opening_pending_database() {
        let mut fixture = fixture(false);
        fixture.state.brush_training.dataset_identity = Some("frozen-dataset".into());
        let info = inspect_gaussian_ply(&fixture.candidate).unwrap();
        fixture.state.validated_output = Some(crate::project::ProjectOutput {
            final_ply: fixture.paths.project.join("final.ply"),
            file_size: info.file_size,
            splat_count: info.splat_count,
            input_images: 1,
            registered_images: 1,
            registered_ratio: 1.,
            points_3d: 0,
        });
        record_collection_artifacts(&fixture.paths, &mut fixture.state, None).unwrap();
        let database = fixture.paths.colmap.join("database.db");
        let wal = fixture.paths.colmap.join("database.db-wal");
        std::fs::write(&database, b"unavailable disposable database").unwrap();
        std::fs::write(&wal, b"pending disposable writes").unwrap();
        let cancel = CancellationToken::new();
        assert_eq!(
            recover_collection_candidate(&fixture.paths, &fixture.state, &cancel)
                .await
                .unwrap(),
            Some(fixture.candidate.clone()),
        );
        assert_eq!(
            std::fs::read(&database).unwrap(),
            b"unavailable disposable database"
        );
        assert_eq!(std::fs::read(&wal).unwrap(), b"pending disposable writes");
        std::fs::remove_file(&fixture.candidate).unwrap();
        assert!(
            recover_collection_candidate(&fixture.paths, &fixture.state, &cancel)
                .await
                .unwrap()
                .is_none(),
            "stale completion flags must not skip database recovery"
        );
    }

    fn set_consumed_training_recipe(state: &mut PipelineStateFile) {
        let initial = crate::presets::resolve_brush_training_preset(
            Quality::High,
            true,
            Some(12_288),
            7_680,
            350_000,
        );
        state.brush_training.initial_resolved = Some(initial);
        state.brush_training.resolved = initial.downgrade_after_oom(350_000);
        state.brush_training.oom_retry_used = true;
        state.brush_training.oom_retry_started = true;
        state.brush_training.oom_retry_source_count = Some(1_000_000);
    }

    #[tokio::test]
    async fn invalid_prepared_images_do_not_regrant_bridge_or_matching_expansion() {
        let mut fixture = fixture(false);
        set_consumed_training_recipe(&mut fixture.state);
        fixture.state.inputs.as_mut().unwrap().bridge_used = true;
        fixture.state.inputs.as_mut().unwrap().expanded_matching = true;
        fixture.state.bridge_backfill = crate::planner::BridgeBackfillCheckpoint {
            status: crate::planner::BridgeBackfillStatus::Completed,
            plan: Some(crate::planner::BridgeBackfillPlan {
                available_budget: 20,
                selected_frame_indices: vec![1, 3, 5],
                ..Default::default()
            }),
            selected_model: Some("old-result".into()),
            ..Default::default()
        };
        let frozen_plan = fixture.state.bridge_backfill.plan.clone();
        let original_source =
            serde_json::to_value(&fixture.state.inputs.as_ref().unwrap().sources).unwrap();
        let recipe = serde_json::to_value(&fixture.state.brush_training).unwrap();
        std::fs::remove_file(fixture.paths.frames.join("normal.png")).unwrap();
        normalize_collection(&fixture.paths, &mut fixture.state, None)
            .await
            .unwrap();
        let checkpoint = fixture.state.inputs.as_ref().unwrap();
        assert!(!checkpoint.prepared);
        assert!(checkpoint.bridge_used && checkpoint.expanded_matching);
        assert_eq!(
            serde_json::to_value(&checkpoint.recovery_sources).unwrap(),
            original_source
        );
        assert_eq!(fixture.state.bridge_backfill.plan, frozen_plan);
        assert_eq!(
            fixture.state.bridge_backfill.status,
            crate::planner::BridgeBackfillStatus::Completed
        );
        assert_eq!(
            fixture.state.bridge_backfill.selected_model.as_deref(),
            Some("old-result")
        );
        assert_eq!(
            serde_json::to_value(&fixture.state.brush_training).unwrap(),
            recipe
        );
        normalize_collection(&fixture.paths, &mut fixture.state, None)
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(&fixture.state.inputs.as_ref().unwrap().recovery_sources).unwrap(),
            original_source,
            "a second normalization must not overwrite the frozen recovery plan"
        );
    }

    #[tokio::test]
    async fn an_uncommitted_completed_flag_does_not_bypass_artifact_validation() {
        let mut fixture = fixture(false);
        fixture.state.stage = PipelineStage::Completed;
        std::fs::remove_file(fixture.paths.frames.join("normal.png")).unwrap();
        assert!(
            normalize_collection(&fixture.paths, &mut fixture.state, None)
                .await
                .unwrap()
        );
        assert!(!fixture.state.inputs.as_ref().unwrap().prepared);
        assert!(!fixture.state.brush_complete);
        assert_ne!(fixture.state.stage, PipelineStage::Completed);
    }

    #[test]
    fn restored_focal_base_only_rebinds_strongly_identical_base_timestamps() {
        let mut fixture = fixture(true);
        let path = fixture.paths.colmap.join("base-database.db");
        let time = std::fs::metadata(&path).unwrap().modified().unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(time + std::time::Duration::from_secs(2))
            .unwrap();
        let previous = fixture.state.artifact_manifests.entries[PREPARED].clone();
        rebind_restored_focal_base(
            &fixture.paths,
            &mut fixture.state,
            &CancellationToken::new(),
        )
        .unwrap();
        assert_ne!(
            fixture.state.artifact_manifests.entries[PREPARED].artifact_identity,
            previous.artifact_identity
        );
        let trusted = fixture.state.artifact_manifests.clone();
        let path = fixture.paths.frames.join("normal.png");
        let time = std::fs::metadata(&path).unwrap().modified().unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(time + std::time::Duration::from_secs(2))
            .unwrap();
        assert!(rebind_restored_focal_base(
            &fixture.paths,
            &mut fixture.state,
            &CancellationToken::new()
        )
        .is_err());
        assert_eq!(fixture.state.artifact_manifests, trusted);
    }

    #[tokio::test]
    async fn unfinished_training_preserves_recipe_and_spent_budget_across_normalization() {
        let mut fixture = fixture(false);
        fixture.state.brush_complete = false;
        set_consumed_training_recipe(&mut fixture.state);
        record_collection_artifacts(&fixture.paths, &mut fixture.state, None).unwrap();
        let saved = fixture.state.brush_training.clone();
        assert!(
            !normalize_collection(&fixture.paths, &mut fixture.state, None)
                .await
                .unwrap()
        );
        assert_eq!(fixture.state.brush_training, saved);
        assert!(
            !normalize_collection(&fixture.paths, &mut fixture.state, None)
                .await
                .unwrap()
        );
        assert_eq!(fixture.state.brush_training, saved);
    }

    #[tokio::test]
    async fn invalid_dataset_records_archive_barrier_without_discarding_saved_recipe() {
        let mut fixture = fixture(false);
        fixture.state.brush_complete = false;
        set_consumed_training_recipe(&mut fixture.state);
        record_collection_artifacts(&fixture.paths, &mut fixture.state, None).unwrap();
        let saved = fixture.state.brush_training.clone();
        let model = fixture.paths.brush.join("dataset/sparse/0/cameras.txt");
        let text = std::fs::read_to_string(&model).unwrap();
        // Keep names, image dimensions, and a fully valid model. A changed
        // calibration must still revoke the old completion signature.
        std::fs::write(&model, text.replace("PINHOLE 4 4 4 4", "PINHOLE 4 4 5 5")).unwrap();
        assert!(
            normalize_collection(&fixture.paths, &mut fixture.state, None)
                .await
                .unwrap()
        );
        assert_eq!(fixture.state.brush_training, saved);
        assert_eq!(fixture.state.retry_cache_pending, ["brush"]);
        assert!(fixture.state.reconstruction_complete);
        assert!(fixture
            .state
            .inputs
            .as_ref()
            .unwrap()
            .reference_image
            .is_none());
        assert!(record_collection_artifacts(&fixture.paths, &mut fixture.state, None).is_err());
        assert!(
            !normalize_collection(&fixture.paths, &mut fixture.state, None)
                .await
                .unwrap()
        );
        assert_eq!(fixture.state.retry_cache_pending, ["brush"]);
        assert_eq!(fixture.state.brush_training, saved);
    }

    #[tokio::test]
    async fn a_missing_dataset_cannot_reuse_a_candidate_for_a_changed_recipe() {
        let mut fixture = fixture(false);
        set_consumed_training_recipe(&mut fixture.state);
        std::fs::remove_file(fixture.paths.brush.join("dataset/inputs-dataset.json")).unwrap();
        normalize_collection(&fixture.paths, &mut fixture.state, None)
            .await
            .unwrap();
        assert!(!fixture.state.brush_complete);
        assert_eq!(fixture.state.retry_cache_pending, ["brush"]);
        assert!(fixture.state.brush_training.oom_retry_used);
    }

    #[tokio::test]
    async fn legacy_candidate_requires_its_dataset_but_remains_compatible_when_present() {
        let mut fixture = fixture(false);
        let current = fixture.state.artifact_manifests.entries[BRUSH_CANDIDATE].clone();
        let legacy = make_manifest(
            &current.attempt_id,
            &current.upstream_identity,
            current.expected_count,
            None,
            current.files,
        )
        .unwrap();
        fixture
            .state
            .artifact_manifests
            .entries
            .insert(BRUSH_CANDIDATE.into(), legacy);
        assert!(
            !normalize_collection(&fixture.paths, &mut fixture.state, None)
                .await
                .unwrap()
        );
        assert!(fixture.state.brush_complete);
        std::fs::remove_file(fixture.paths.brush.join("dataset/inputs-dataset.json")).unwrap();
        normalize_collection(&fixture.paths, &mut fixture.state, None)
            .await
            .unwrap();
        assert!(!fixture.state.brush_complete);
        assert_eq!(fixture.state.retry_cache_pending, ["brush"]);
    }

    #[tokio::test]
    async fn interrupted_matching_does_not_invalidate_unchanged_feature_rows() {
        let mut fixture = fixture(false);
        fixture.state.matching_complete = false;
        fixture.state.reconstruction_complete = false;
        fixture.state.brush_complete = false;
        fixture.state.inputs.as_mut().unwrap().reference_image = None;
        record_collection_artifacts(&fixture.paths, &mut fixture.state, None).unwrap();
        let db = Connection::open(fixture.paths.colmap.join("database.db")).unwrap();
        db.execute_batch("CREATE TABLE matching_progress(batch INTEGER); INSERT INTO matching_progress VALUES(1);")
            .unwrap();
        drop(db);
        normalize_collection(&fixture.paths, &mut fixture.state, None)
            .await
            .unwrap();
        assert!(fixture.state.features_complete);
        assert!(!fixture.state.matching_complete);
    }

    #[test]
    fn operational_io_errors_are_not_downgraded_to_invalid_cache() {
        let cancel = CancellationToken::new();
        for kind in [
            std::io::ErrorKind::PermissionDenied,
            std::io::ErrorKind::WouldBlock,
            std::io::ErrorKind::StorageFull,
            std::io::ErrorKind::Interrupted,
            std::io::ErrorKind::Other,
        ] {
            let error = usable_validation::<()>(
                Err(SplatError::Io(std::io::Error::new(
                    kind,
                    "original failure",
                ))),
                &cancel,
            )
            .unwrap_err();
            assert_eq!(error.io_error().unwrap().kind(), kind);
        }
        let error = usable_validation::<()>(
            Err(SplatError::Io(std::io::Error::from_raw_os_error(5))),
            &cancel,
        )
        .unwrap_err();
        assert_eq!(error.io_error().unwrap().raw_os_error(), Some(5));
        assert!(usable_validation::<()>(
            Err(SplatError::Io(std::io::Error::from(
                std::io::ErrorKind::NotFound
            ))),
            &cancel,
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn sqlite_operational_errors_preserve_their_extended_code() {
        for code in [
            rusqlite::ffi::SQLITE_BUSY,
            rusqlite::ffi::SQLITE_LOCKED,
            rusqlite::ffi::SQLITE_READONLY,
            rusqlite::ffi::SQLITE_CANTOPEN,
            rusqlite::ffi::SQLITE_IOERR_READ,
            rusqlite::ffi::SQLITE_FULL,
        ] {
            let error = database_error(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(code),
                Some("original sqlite failure".into()),
            ));
            assert!(!invalid_artifact_error(&error));
            let sqlite = error
                .io_error()
                .unwrap()
                .get_ref()
                .unwrap()
                .downcast_ref::<rusqlite::Error>()
                .unwrap();
            assert_eq!(sqlite.sqlite_error().unwrap().extended_code, code);
        }
        let corrupt = database_error(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
            None,
        ));
        assert!(invalid_artifact_error(&corrupt));
    }

    #[tokio::test]
    async fn failed_database_access_leaves_the_entire_checkpoint_unchanged() {
        let mut fixture = fixture(false);
        set_consumed_training_recipe(&mut fixture.state);
        let before = serde_json::to_value(&fixture.state).unwrap();
        let database = fixture.paths.colmap.join("database.db");
        std::fs::remove_file(&database).unwrap();
        std::fs::create_dir(&database).unwrap();
        let error = normalize_collection(&fixture.paths, &mut fixture.state, None)
            .await
            .unwrap_err();
        assert!(error.io_error().is_some());
        assert_eq!(serde_json::to_value(&fixture.state).unwrap(), before);
    }

    #[tokio::test]
    async fn missing_candidate_only_invalidates_training_and_publication_dependants() {
        let mut fixture = fixture(false);
        std::fs::remove_file(&fixture.candidate).unwrap();
        normalize_collection(&fixture.paths, &mut fixture.state, None)
            .await
            .unwrap();
        assert!(fixture.state.inputs.as_ref().unwrap().prepared);
        assert!(fixture.state.features_complete);
        assert!(fixture.state.matching_complete);
        assert!(fixture.state.reconstruction_complete);
        assert!(!fixture.state.brush_complete);
        assert!(!fixture
            .state
            .artifact_manifests
            .entries
            .contains_key(BRUSH_CANDIDATE));
    }

    #[tokio::test]
    async fn a_new_run_id_does_not_change_artifact_identity() {
        let mut fixture = fixture(false);
        let before = fixture.state.artifact_manifests.clone();
        let after =
            capture_collection_artifacts(&fixture.paths, &fixture.state, Some(Uuid::new_v4()))
                .unwrap();
        for key in [
            PREPARED,
            FEATURES,
            MATCHING,
            RECONSTRUCTION,
            TRAINING_DATASET,
            BRUSH_CANDIDATE,
        ] {
            assert_eq!(
                before.entries[key].artifact_identity,
                after.entries[key].artifact_identity
            );
            assert_ne!(
                before.entries[key].attempt_id,
                after.entries[key].attempt_id
            );
        }
        assert!(
            !normalize_collection(&fixture.paths, &mut fixture.state, Some(Uuid::new_v4()))
                .await
                .unwrap()
        );
    }
}
