//! Immutable, content-addressed source snapshots for reshoot execution.
//!
//! This module deliberately does not know about the legacy or collection
//! runners. Those paths resolve their source project and feed the same builder,
//! leaving the execution pipeline with one durable, read-only source contract.

use crate::{
    error::{Result, SplatError},
    project::manager::atomic_write_json,
    reconstruction::{
        colmap_model::{read_cameras, read_registered_images, reusable_model_files},
        ply::inspect_gaussian_ply,
    },
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub const RESHOOT_SNAPSHOT_VERSION: u32 = 3;
pub const RESHOOT_SNAPSHOT_MANIFEST: &str = "snapshot.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SnapshotAssetRole {
    Image,
    Mask,
    ColmapDatabase,
    SparseModel,
    SourcePly,
    Gps,
    Transform,
    Other,
}

#[derive(Clone, Debug)]
pub struct SnapshotAssetInput {
    pub role: SnapshotAssetRole,
    pub source: PathBuf,
    /// Portable path relative to the snapshot root.
    pub relative_path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotAsset {
    pub role: SnapshotAssetRole,
    pub relative_path: String,
    pub sha256: String,
    pub file_size: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotCamera {
    pub id: u32,
    pub model_id: i32,
    pub model: String,
    pub width: u64,
    pub height: u64,
    pub params: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotRegisteredImage {
    pub id: u32,
    pub camera_id: u32,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotPly {
    pub relative_path: String,
    pub sha256: String,
    pub file_size: u64,
    pub splat_count: u64,
    pub sh_degree: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReshootSnapshotV3 {
    pub version: u32,
    pub snapshot_id: Uuid,
    pub source_project_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub content_identity: String,
    pub coordinate_frame_id: String,
    pub has_erp: bool,
    pub cameras: Vec<SnapshotCamera>,
    pub registered_images: Vec<SnapshotRegisteredImage>,
    pub assets: Vec<SnapshotAsset>,
    pub source_ply: Option<SnapshotPly>,
}

#[derive(Clone, Debug)]
pub struct ReshootSnapshotBuilder {
    ready_root: PathBuf,
    source_project_id: Option<Uuid>,
    coordinate_frame_id: Option<String>,
    model: Option<PathBuf>,
    database: Option<PathBuf>,
    source_ply: Option<PathBuf>,
    assets: Vec<SnapshotAssetInput>,
}

impl ReshootSnapshotBuilder {
    pub fn new(ready_root: impl Into<PathBuf>) -> Self {
        Self {
            ready_root: ready_root.into(),
            source_project_id: None,
            coordinate_frame_id: None,
            model: None,
            database: None,
            source_ply: None,
            assets: Vec::new(),
        }
    }

    pub fn source_project_id(mut self, value: Uuid) -> Self {
        self.source_project_id = Some(value);
        self
    }

    pub fn coordinate_frame_id(mut self, value: impl Into<String>) -> Self {
        self.coordinate_frame_id = Some(value.into());
        self
    }

    pub fn model(mut self, path: impl Into<PathBuf>) -> Self {
        self.model = Some(path.into());
        self
    }

    pub fn database(mut self, path: impl Into<PathBuf>) -> Self {
        self.database = Some(path.into());
        self
    }

    pub fn source_ply(mut self, path: impl Into<PathBuf>) -> Self {
        self.source_ply = Some(path.into());
        self
    }

    pub fn image(mut self, source: impl Into<PathBuf>, relative_path: impl Into<PathBuf>) -> Self {
        self.assets.push(SnapshotAssetInput {
            role: SnapshotAssetRole::Image,
            source: source.into(),
            relative_path: PathBuf::from("images").join(relative_path.into()),
        });
        self
    }

    pub fn mask(mut self, source: impl Into<PathBuf>, relative_path: impl Into<PathBuf>) -> Self {
        self.assets.push(SnapshotAssetInput {
            role: SnapshotAssetRole::Mask,
            source: source.into(),
            relative_path: PathBuf::from("masks").join(relative_path.into()),
        });
        self
    }

    pub fn asset(mut self, value: SnapshotAssetInput) -> Self {
        self.assets.push(value);
        self
    }

    pub async fn build(self, cancel: &CancellationToken) -> Result<ReshootSnapshotV3> {
        self.build_observed(cancel, None).await
    }

    pub(crate) async fn build_observed(
        self,
        cancel: &CancellationToken,
        observer: Option<&(dyn Fn(u64, u64) + Send + Sync)>,
    ) -> Result<ReshootSnapshotV3> {
        let parent = self.ready_root.parent().ok_or_else(|| {
            SplatError::Process("Reshoot snapshot path has no parent directory".into())
        })?;
        tokio::fs::create_dir_all(parent).await?;
        let staging = parent.join(format!(".reshoot-snapshot-{}.staging", Uuid::new_v4()));
        tokio::fs::create_dir(&staging).await?;

        let result = self.build_in(&staging, cancel, observer).await;
        let manifest = match result {
            Ok(value) => value,
            Err(error) => {
                let _ = tokio::fs::remove_dir_all(&staging).await;
                return Err(error);
            }
        };

        if self.ready_root.exists() {
            let existing = load_snapshot(&self.ready_root)?;
            let _ = tokio::fs::remove_dir_all(&staging).await;
            if existing.content_identity != manifest.content_identity
                || existing.coordinate_frame_id != manifest.coordinate_frame_id
                || existing.source_project_id != manifest.source_project_id
            {
                return Err(SplatError::Process(
                    "Reshoot snapshot already exists with different source content".into(),
                ));
            }
            make_snapshot_read_only(&self.ready_root).await?;
            return Ok(existing);
        }

        tokio::fs::rename(&staging, &self.ready_root).await?;
        make_snapshot_read_only(&self.ready_root).await?;
        Ok(manifest)
    }

    async fn build_in(
        &self,
        staging: &Path,
        cancel: &CancellationToken,
        observer: Option<&(dyn Fn(u64, u64) + Send + Sync)>,
    ) -> Result<ReshootSnapshotV3> {
        let model = self.model.as_deref().ok_or_else(|| {
            SplatError::Process("Reshoot snapshot is missing a COLMAP model".into())
        })?;
        let mut inputs = self.assets.clone();
        for path in reusable_model_files(model)? {
            let name = path
                .file_name()
                .ok_or_else(|| SplatError::InvalidPath(path.clone()))?
                .to_os_string();
            inputs.push(SnapshotAssetInput {
                role: SnapshotAssetRole::SparseModel,
                source: path,
                relative_path: PathBuf::from("colmap").join("sparse").join(name),
            });
        }
        if let Some(database) = &self.database {
            inputs.push(SnapshotAssetInput {
                role: SnapshotAssetRole::ColmapDatabase,
                source: database.clone(),
                relative_path: PathBuf::from("colmap").join("database.db"),
            });
        }
        if let Some(ply) = &self.source_ply {
            inputs.push(SnapshotAssetInput {
                role: SnapshotAssetRole::SourcePly,
                source: ply.clone(),
                relative_path: PathBuf::from("training").join("init-source.ply"),
            });
        }
        validate_inputs(&inputs)?;

        let mut assets = Vec::with_capacity(inputs.len());
        for (index, input) in inputs.iter().enumerate() {
            if cancel.is_cancelled() {
                return Err(SplatError::Cancelled);
            }
            let portable = portable_relative_path(&input.relative_path)?;
            let target = staging.join(&input.relative_path);
            if let Some(parent) = target.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            let (sha256, file_size) = copy_and_hash(&input.source, &target, cancel).await?;
            assets.push(SnapshotAsset {
                role: input.role,
                relative_path: portable,
                sha256,
                file_size,
            });
            if let Some(observer) = observer {
                observer(index as u64 + 1, inputs.len() as u64);
            }
        }
        assets.sort_by(|a, b| {
            (a.role, a.relative_path.as_str()).cmp(&(b.role, b.relative_path.as_str()))
        });

        // Parse the staged model rather than the live source. This makes the
        // manifest describe exactly the immutable bytes that were copied even
        // if an external process changes the source project during snapshotting.
        let copied_model = staging.join("colmap").join("sparse");
        let (cameras, registered_images, has_erp) = snapshot_model_details(&copied_model)?;

        let source_ply = assets
            .iter()
            .find(|asset| asset.role == SnapshotAssetRole::SourcePly)
            .map(|asset| {
                let path = staging.join(&asset.relative_path);
                let info = inspect_gaussian_ply(&path)?;
                Ok::<SnapshotPly, SplatError>(SnapshotPly {
                    relative_path: asset.relative_path.clone(),
                    sha256: asset.sha256.clone(),
                    file_size: info.file_size,
                    splat_count: info.splat_count,
                    sh_degree: gaussian_sh_degree(&path)?,
                })
            })
            .transpose()?;

        let content_identity =
            snapshot_content_identity(&assets, &cameras, &registered_images, source_ply.as_ref())?;
        let coordinate_frame_id = match self.coordinate_frame_id.as_deref() {
            Some(value) if !value.trim().is_empty() => value.to_owned(),
            Some(_) => {
                return Err(SplatError::Process(
                    "Reshoot snapshot coordinate frame id is empty".into(),
                ))
            }
            None => coordinate_frame_identity(&assets),
        };
        let manifest = ReshootSnapshotV3 {
            version: RESHOOT_SNAPSHOT_VERSION,
            snapshot_id: Uuid::new_v4(),
            source_project_id: self.source_project_id,
            created_at: Utc::now(),
            content_identity,
            coordinate_frame_id,
            has_erp,
            cameras,
            registered_images,
            assets,
            source_ply,
        };
        atomic_write_json(&staging.join(RESHOOT_SNAPSHOT_MANIFEST), &manifest).await?;
        Ok(manifest)
    }
}

pub fn load_snapshot(root: &Path) -> Result<ReshootSnapshotV3> {
    let bytes = std::fs::read(root.join(RESHOOT_SNAPSHOT_MANIFEST))?;
    let manifest: ReshootSnapshotV3 = serde_json::from_slice(&bytes)?;
    if manifest.version != RESHOOT_SNAPSHOT_VERSION
        || manifest.assets.is_empty()
        || manifest.coordinate_frame_id.trim().is_empty()
        || manifest.content_identity.len() != 64
    {
        return Err(SplatError::Process(
            "Reshoot snapshot manifest is invalid or unsupported".into(),
        ));
    }
    validate_manifest_assets(&manifest)?;
    Ok(manifest)
}

/// Authenticate only the small manifest against a previously saved identity.
/// This is not asset validation: input preflight uses it to read frozen source
/// policies without decoding or hashing every image in the source project.
pub(crate) fn load_bound_snapshot_metadata(
    root: &Path,
    expected_identity: &str,
) -> Result<ReshootSnapshotV3> {
    let manifest = load_snapshot(root)?;
    let actual = snapshot_content_identity(
        &manifest.assets,
        &manifest.cameras,
        &manifest.registered_images,
        manifest.source_ply.as_ref(),
    )?;
    if actual != manifest.content_identity || actual != expected_identity {
        return Err(SplatError::Process(
            "Reshoot snapshot content identity does not match its manifest".into(),
        ));
    }
    Ok(manifest)
}

pub fn verify_snapshot(root: &Path) -> Result<ReshootSnapshotV3> {
    verify_snapshot_observed(root, &CancellationToken::new(), None)
}

pub(crate) fn verify_snapshot_observed(
    root: &Path,
    cancel: &CancellationToken,
    observer: Option<&(dyn Fn(u64, u64) + Send + Sync)>,
) -> Result<ReshootSnapshotV3> {
    let manifest = load_snapshot(root)?;
    let mut assets = Vec::with_capacity(manifest.assets.len());
    for (index, expected) in manifest.assets.iter().enumerate() {
        if cancel.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        let path = root.join(&expected.relative_path);
        let metadata = std::fs::symlink_metadata(&path)?;
        if !metadata.file_type().is_file() || metadata.len() != expected.file_size {
            return Err(SplatError::Process(
                "Reshoot snapshot asset is missing or changed".into(),
            ));
        }
        let actual = crate::video::panorama::fingerprint_cancellable(&path, cancel)?;
        if actual != expected.sha256 {
            return Err(SplatError::Process(
                "Reshoot snapshot asset content has changed".into(),
            ));
        }
        assets.push(expected.clone());
        if let Some(observer) = observer {
            observer(index as u64 + 1, manifest.assets.len() as u64);
        }
    }
    let identity = snapshot_content_identity(
        &assets,
        &manifest.cameras,
        &manifest.registered_images,
        manifest.source_ply.as_ref(),
    )?;
    if identity != manifest.content_identity {
        return Err(SplatError::Process(
            "Reshoot snapshot content identity does not match its manifest".into(),
        ));
    }
    let (cameras, registered_images, has_erp) =
        snapshot_model_details(&root.join("colmap").join("sparse"))?;
    if cameras != manifest.cameras
        || registered_images != manifest.registered_images
        || has_erp != manifest.has_erp
    {
        return Err(SplatError::Process(
            "Reshoot snapshot COLMAP model does not match its manifest".into(),
        ));
    }
    if let Some(ply) = &manifest.source_ply {
        let info = inspect_gaussian_ply(&root.join(&ply.relative_path))?;
        if info.file_size != ply.file_size || info.splat_count != ply.splat_count {
            return Err(SplatError::Process(
                "Reshoot snapshot source PLY no longer matches its manifest".into(),
            ));
        }
    }
    Ok(manifest)
}

fn validate_inputs(inputs: &[SnapshotAssetInput]) -> Result<()> {
    let mut destinations = BTreeSet::new();
    for input in inputs {
        let portable = portable_relative_path(&input.relative_path)?;
        if !destinations.insert(portable.to_lowercase()) {
            return Err(SplatError::Process(
                "Reshoot snapshot contains duplicate destination paths".into(),
            ));
        }
        let metadata = std::fs::symlink_metadata(&input.source)?;
        if !metadata.file_type().is_file() {
            return Err(SplatError::InvalidPath(input.source.clone()));
        }
    }
    Ok(())
}

fn validate_manifest_assets(manifest: &ReshootSnapshotV3) -> Result<()> {
    let mut destinations = BTreeSet::new();
    for asset in &manifest.assets {
        let portable = portable_relative_path(Path::new(&asset.relative_path))?;
        if portable != asset.relative_path
            || asset.sha256.len() != 64
            || !asset.sha256.bytes().all(|value| value.is_ascii_hexdigit())
            || !destinations.insert(portable.to_lowercase())
        {
            return Err(SplatError::Process(
                "Reshoot snapshot manifest contains an invalid asset".into(),
            ));
        }
    }
    if let Some(ply) = &manifest.source_ply {
        let asset = manifest.assets.iter().find(|asset| {
            asset.role == SnapshotAssetRole::SourcePly && asset.relative_path == ply.relative_path
        });
        if !matches!(asset, Some(asset) if asset.sha256 == ply.sha256 && asset.file_size == ply.file_size)
        {
            return Err(SplatError::Process(
                "Reshoot snapshot source PLY identity is inconsistent".into(),
            ));
        }
    }
    Ok(())
}

fn snapshot_model_details(
    model: &Path,
) -> Result<(Vec<SnapshotCamera>, Vec<SnapshotRegisteredImage>, bool)> {
    let cameras = read_cameras(model)?
        .into_iter()
        .map(|camera| SnapshotCamera {
            id: camera.id,
            model_id: camera.model_id,
            model: camera.model,
            width: camera.width,
            height: camera.height,
            params: camera.params,
        })
        .collect::<Vec<_>>();
    let registered_images = read_registered_images(model)?
        .into_iter()
        .map(|image| SnapshotRegisteredImage {
            id: image.id,
            camera_id: image.camera_id,
            name: image.name,
        })
        .collect::<Vec<_>>();
    let has_erp = cameras.iter().any(|camera| camera.model_id == 17);
    Ok((cameras, registered_images, has_erp))
}

fn portable_relative_path(path: &Path) -> Result<String> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(SplatError::InvalidPath(path.to_path_buf()));
    }
    let value = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    if value.contains(['\\', ':', '\0', '\n', '\r']) {
        return Err(SplatError::InvalidPath(path.to_path_buf()));
    }
    Ok(value)
}

async fn copy_and_hash(
    source: &Path,
    target: &Path,
    cancel: &CancellationToken,
) -> Result<(String, u64)> {
    let mut input = tokio::fs::File::open(source).await?;
    let mut output = tokio::fs::File::create(target).await?;
    let mut hash = Sha256::new();
    let mut file_size = 0_u64;
    let mut buffer = vec![0_u8; 256 * 1024];
    loop {
        if cancel.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        let count = input.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        output.write_all(&buffer[..count]).await?;
        hash.update(&buffer[..count]);
        file_size = file_size.saturating_add(count as u64);
    }
    output.flush().await?;
    output.sync_all().await?;
    Ok((format!("{:x}", hash.finalize()), file_size))
}

#[cfg(test)]
fn hash_file(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0_u8; 256 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn snapshot_content_identity(
    assets: &[SnapshotAsset],
    cameras: &[SnapshotCamera],
    registered_images: &[SnapshotRegisteredImage],
    source_ply: Option<&SnapshotPly>,
) -> Result<String> {
    let bytes = serde_json::to_vec(&(assets, cameras, registered_images, source_ply))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn coordinate_frame_identity(assets: &[SnapshotAsset]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"ooosplat-reshoot-coordinate-frame-v1");
    for asset in assets
        .iter()
        .filter(|asset| asset.role == SnapshotAssetRole::SparseModel)
    {
        hash.update(asset.relative_path.as_bytes());
        hash.update(asset.sha256.as_bytes());
    }
    format!("colmap-sha256:{:x}", hash.finalize())
}

fn gaussian_sh_degree(path: &Path) -> Result<Option<u8>> {
    use std::io::{BufRead, BufReader};
    let file = std::fs::File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let mut rest = BTreeSet::new();
    let mut bytes = 0_u64;
    loop {
        line.clear();
        let count = reader.read_line(&mut line)?;
        if count == 0 {
            return Err(SplatError::Process("PLY is missing end_header".into()));
        }
        bytes = bytes.saturating_add(count as u64);
        if bytes > 256 * 1024 {
            return Err(SplatError::Process("PLY header is too large".into()));
        }
        let trimmed = line.trim();
        if trimmed == "end_header" {
            break;
        }
        if let Some(value) = trimmed
            .strip_prefix("property ")
            .and_then(|value| value.split_whitespace().last())
            .and_then(|name| name.strip_prefix("f_rest_"))
            .and_then(|index| index.parse::<usize>().ok())
        {
            rest.insert(value);
        }
    }
    if rest.is_empty() {
        return Ok(Some(0));
    }
    let count = rest.len();
    if rest.iter().copied().eq(0..count) {
        for (degree, expected) in [(1, 9), (2, 24), (3, 45)] {
            if count == expected {
                return Ok(Some(degree));
            }
        }
    }
    Ok(None)
}

async fn make_snapshot_read_only(root: &Path) -> Result<()> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let mut entries = tokio::fs::read_dir(&path).await?;
        while let Some(entry) = entries.next_entry().await? {
            let metadata = entry.metadata().await?;
            if metadata.is_dir() {
                stack.push(entry.path());
            } else if metadata.is_file() {
                let mut permissions = metadata.permissions();
                permissions.set_readonly(true);
                tokio::fs::set_permissions(entry.path(), permissions).await?;
            } else {
                return Err(SplatError::Process(
                    "Reshoot snapshot contains an unsupported filesystem entry".into(),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_model(root: &Path, model_id: i32) {
        std::fs::create_dir_all(root).unwrap();
        let model = if model_id == 17 {
            "EQUIRECTANGULAR"
        } else {
            "PINHOLE"
        };
        let params = if model_id == 17 {
            "4096 2048"
        } else {
            "1000 1000 500 250"
        };
        std::fs::write(
            root.join("cameras.txt"),
            format!("1 {model} 1000 500 {params}\n"),
        )
        .unwrap();
        std::fs::write(root.join("images.txt"), "1 1 0 0 0 0 0 0 1 frame.jpg\n\n").unwrap();
        std::fs::write(root.join("points3D.txt"), "").unwrap();
    }

    fn write_ply(path: &Path) {
        let mut file = std::fs::File::create(path).unwrap();
        writeln!(file, "ply").unwrap();
        writeln!(file, "format binary_little_endian 1.0").unwrap();
        writeln!(file, "element vertex 1").unwrap();
        for name in [
            "x", "y", "z", "f_dc_0", "f_dc_1", "f_dc_2", "opacity", "scale_0", "scale_1",
            "scale_2", "rot_0", "rot_1", "rot_2", "rot_3",
        ] {
            writeln!(file, "property float {name}").unwrap();
        }
        for index in 0..45 {
            writeln!(file, "property float f_rest_{index}").unwrap();
        }
        writeln!(file, "end_header").unwrap();
        file.write_all(&vec![0_u8; (14 + 45) * 4]).unwrap();
    }

    #[tokio::test]
    async fn builds_an_immutable_content_addressed_erp_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let model = source.join("sparse");
        std::fs::create_dir_all(&source).unwrap();
        write_model(&model, 17);
        std::fs::write(source.join("frame.jpg"), b"image").unwrap();
        std::fs::write(source.join("database.db"), b"database").unwrap();
        write_ply(&source.join("final.ply"));
        let original = hash_file(&source.join("final.ply")).unwrap();
        let ready = temp.path().join("derived").join("snapshot");
        let observed = std::sync::Mutex::new(Vec::new());
        let manifest = ReshootSnapshotBuilder::new(&ready)
            .source_project_id(Uuid::new_v4())
            .model(&model)
            .database(source.join("database.db"))
            .source_ply(source.join("final.ply"))
            .image(source.join("frame.jpg"), "frame.jpg")
            .build_observed(
                &CancellationToken::new(),
                Some(&|current, total| observed.lock().unwrap().push((current, total))),
            )
            .await
            .unwrap();

        assert!(manifest.has_erp);
        assert_eq!(manifest.source_ply.as_ref().unwrap().splat_count, 1);
        assert_eq!(manifest.source_ply.as_ref().unwrap().sh_degree, Some(3));
        assert_eq!(manifest.content_identity.len(), 64);
        assert!(manifest.coordinate_frame_id.starts_with("colmap-sha256:"));
        assert_eq!(hash_file(&source.join("final.ply")).unwrap(), original);
        let counts = observed.lock().unwrap();
        assert_eq!(counts.len(), manifest.assets.len());
        assert_eq!(
            counts.last(),
            Some(&(manifest.assets.len() as u64, manifest.assets.len() as u64))
        );
        drop(counts);
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(matches!(
            verify_snapshot_observed(&ready, &cancel, None),
            Err(SplatError::Cancelled)
        ));
        assert_eq!(verify_snapshot(&ready).unwrap(), manifest);
        assert!(std::fs::metadata(ready.join("training/init-source.ply"))
            .unwrap()
            .permissions()
            .readonly());
    }

    #[tokio::test]
    async fn existing_snapshot_is_idempotent_but_rejects_changed_content() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let model = source.join("sparse");
        std::fs::create_dir_all(&source).unwrap();
        write_model(&model, 1);
        let image = source.join("frame.jpg");
        std::fs::write(&image, b"first").unwrap();
        let ready = temp.path().join("snapshot");
        let build = || {
            ReshootSnapshotBuilder::new(&ready)
                .model(&model)
                .image(&image, "frame.jpg")
        };
        let first = build().build(&CancellationToken::new()).await.unwrap();
        let second = build().build(&CancellationToken::new()).await.unwrap();
        assert_eq!(first.content_identity, second.content_identity);
        std::fs::write(&image, b"other").unwrap();
        let error = build().build(&CancellationToken::new()).await.unwrap_err();
        assert!(error.to_string().contains("different source content"));
    }

    #[tokio::test]
    async fn rejects_unsafe_or_duplicate_destinations_without_touching_sources() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let model = source.join("sparse");
        std::fs::create_dir_all(&source).unwrap();
        write_model(&model, 1);
        let image = source.join("frame.jpg");
        std::fs::write(&image, b"image").unwrap();
        let ready = temp.path().join("snapshot");
        let error = ReshootSnapshotBuilder::new(&ready)
            .model(&model)
            .image(&image, "../escape.jpg")
            .build(&CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(error, SplatError::InvalidPath(_)));
        assert_eq!(std::fs::read(&image).unwrap(), b"image");
        assert!(!temp.path().join("escape.jpg").exists());

        let duplicate_error = ReshootSnapshotBuilder::new(&ready)
            .model(&model)
            .image(&image, "Frame.jpg")
            .image(&image, "frame.jpg")
            .build(&CancellationToken::new())
            .await
            .unwrap_err();
        assert!(duplicate_error
            .to_string()
            .contains("duplicate destination paths"));
        assert_eq!(std::fs::read(&image).unwrap(), b"image");
    }
}
