//! Bind staged Gaussian copies to the saved recipe before changing the mounted
//! dataset. The receipt is provenance only, never a completion checkpoint.
use super::*;
use crate::pipeline::brush_initialization::{self, GaussianIdentity, InitializationContract};
use tokio_util::sync::CancellationToken;

const RECEIPT: &str = ".ooosplat-initialization.json";

#[cfg(test)]
#[path = "training_initialization_tests.rs"]
mod tests;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Binding {
    source_sha256: Option<String>,
    coordinate_frame_id: Option<String>,
    transform: Option<serde_json::Value>,
    dataset_identity: Option<String>,
}

impl Binding {
    fn from_state(state: &PipelineStateFile) -> Result<Self> {
        let recipe = state.reshoot.as_ref().and_then(|r| r.recipe.as_ref());
        Ok(Self {
            source_sha256: recipe.and_then(|r| r.init_sha256.clone()),
            coordinate_frame_id: recipe.and_then(|r| r.init_georeference.clone()),
            transform: state
                .gps
                .as_ref()
                .filter(|g| g.status == "complete")
                .and_then(|g| g.transform)
                .map(serde_json::to_value)
                .transpose()?,
            dataset_identity: state
                .brush_training
                .dataset_identity
                .clone()
                .or_else(|| state.brush_training.legacy_dataset_identity.clone()),
        })
    }
}

#[derive(Serialize, Deserialize)]
struct Receipt {
    version: u32,
    binding: Binding,
    owned: Vec<GaussianIdentity>,
}

fn invalid() -> SplatError {
    SplatError::Process("Training initialization provenance is missing or incompatible".into())
}

/// Check every component, including missing destinations' existing parents.
pub(super) fn owned_path(root: &Path, path: &Path) -> Result<()> {
    let relative = path.strip_prefix(root).map_err(|_| invalid())?;
    if !root.is_absolute()
        || relative
            .components()
            .any(|p| !matches!(p, std::path::Component::Normal(_)))
    {
        return Err(invalid());
    }
    let mut current = root.to_owned();
    let components = std::iter::once(None).chain(relative.components().map(Some));
    for component in components {
        if let Some(component) = component {
            current.push(component);
        }
        let metadata = match std::fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        #[cfg(windows)]
        let redirected = {
            use std::os::windows::fs::MetadataExt;
            metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
        };
        #[cfg(not(windows))]
        let redirected = metadata.file_type().is_symlink();
        if redirected {
            return Err(invalid());
        }
    }
    Ok(())
}

pub(super) struct StagedInitialization {
    path: PathBuf,
}

impl StagedInitialization {
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StagedInitialization {
    fn drop(&mut self) {
        // Only our exclusively created file and now-empty UUID directory.
        // Cleanup failure must never replace the main I/O/cancellation result.
        let _ = std::fs::remove_file(&self.path);
        if let Some(directory) = self.path.parent() {
            let _ = std::fs::remove_dir(directory);
        }
    }
}

pub(super) fn staging_path(paths: &ProjectPaths) -> Result<StagedInitialization> {
    let parent = paths.work.join("initialization-staging");
    owned_path(&paths.project, &parent)?;
    std::fs::create_dir_all(&parent)?;
    let directory = parent.join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir(&directory)?;
    let path = directory.join("init.ply");
    // The transform writer requires an existing output. Never truncate an old
    // model or write through a hard link when preparing the fresh copy.
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    Ok(StagedInitialization { path })
}

pub(super) fn identity(
    info: &crate::reconstruction::ply::PlyInfo,
    sh: u32,
    hash: String,
) -> GaussianIdentity {
    GaussianIdentity {
        sha256: hash,
        splat_count: info.splat_count,
        sh_degree: sh,
    }
}

async fn known_owned(
    paths: &ProjectPaths,
    state: &PipelineStateFile,
    dataset: &Path,
    extra: &[GaussianIdentity],
    cancel: &CancellationToken,
) -> Result<Vec<GaussianIdentity>> {
    owned_path(&paths.project, dataset)?;
    let binding = Binding::from_state(state)?;
    let receipt_path = dataset.join(RECEIPT);
    owned_path(&paths.project, &receipt_path)?;
    let mut owned = extra.to_vec();
    match tokio::fs::metadata(&receipt_path).await {
        Ok(meta) => {
            if !meta.is_file() || meta.len() > 64 * 1024 {
                return Err(invalid());
            }
            let receipt: Receipt = serde_json::from_slice(&tokio::fs::read(&receipt_path).await?)?;
            if receipt.version != 1 || receipt.binding != binding {
                return Err(invalid());
            }
            owned.extend(receipt.owned);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    // Older tasks have no receipt. A saved transformed identity is strong
    // evidence; an original hash is usable only when no GPS transform applied.
    let saved_hash = state.brush_training.legacy_initialization_sha256.as_deref();
    let original_hash = binding
        .source_sha256
        .as_deref()
        .filter(|_| binding.transform.is_none());
    for entry in std::fs::read_dir(dataset)? {
        if cancel.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name != "init.ply" && !name.starts_with("init.warm-") {
            continue;
        }
        owned_path(&paths.project, &entry.path())?;
        let (info, sh, hash) = super::reshoot_runner::validate_ply(&entry.path(), cancel).await?;
        if saved_hash == Some(hash.as_str()) || original_hash == Some(hash.as_str()) {
            owned.push(identity(&info, sh, hash));
        }
    }
    owned.sort_by(|a, b| a.sha256.cmp(&b.sha256));
    owned.dedup();
    if owned.len() > 16 {
        return Err(invalid());
    }
    Ok(owned)
}

async fn persist_receipt(
    paths: &ProjectPaths,
    state: &PipelineStateFile,
    dataset: &Path,
    owned: Vec<GaussianIdentity>,
) -> Result<()> {
    let receipt = dataset.join(RECEIPT);
    owned_path(&paths.project, &receipt)?;
    crate::project::manager::atomic_write_json(
        &receipt,
        &Receipt {
            version: 1,
            binding: Binding::from_state(state)?,
            owned,
        },
    )
    .await
}

pub(super) async fn install_staged(
    paths: &ProjectPaths,
    state: &PipelineStateFile,
    dataset: &Path,
    staged: &Path,
    identity: GaussianIdentity,
    coordinate_frame_id: String,
    cancel: &CancellationToken,
) -> Result<InitializationContract> {
    owned_path(&paths.project, staged)?;
    let owned = known_owned(
        paths,
        state,
        dataset,
        std::slice::from_ref(&identity),
        cancel,
    )
    .await?;
    // Durable evidence is installed before any move; failed/partial isolation
    // can be repeated without refreshing the saved fallback allowance.
    persist_receipt(paths, state, dataset, owned.clone()).await?;
    let contract = InitializationContract::GaussianSnapshot {
        identity: identity.clone(),
        coordinate_frame_id,
    };
    let destination = dataset.join("init.ply");
    owned_path(&paths.project, &destination)?;
    if tokio::fs::try_exists(&destination).await? {
        let (info, sh, hash) = super::reshoot_runner::validate_ply(&destination, cancel).await?;
        if GaussianIdentity::from_validated(&info, sh, hash) == identity {
            // A resumed Warm task need not accumulate another copy of the same
            // large PLY. Still isolate any content-proven extra initializations.
            brush_initialization::prepare_owned_dataset(
                &paths.project,
                dataset,
                &contract,
                &owned,
                cancel,
            )
            .await?;
            return Ok(contract);
        }
    }
    brush_initialization::archive_owned_initializations(&paths.project, dataset, &owned, cancel)
        .await?;
    if cancel.is_cancelled() {
        return Err(SplatError::Cancelled);
    }
    owned_path(&paths.project, &destination)?;
    if tokio::fs::try_exists(&destination).await? {
        return Err(invalid());
    }
    // Rename our unique staged file, never overwrite an unverified init file.
    super::retry_archive::rename_entry(staged, &destination, cancel, |_| async {}).await?;
    verify_start(paths, dataset, &contract, cancel).await?;
    Ok(contract)
}

pub(super) async fn prepare_saved(
    paths: &ProjectPaths,
    state: &PipelineStateFile,
    dataset: &Path,
    contract: &InitializationContract,
    cancel: &CancellationToken,
) -> Result<()> {
    let extra = match contract {
        InitializationContract::GaussianSnapshot { identity, .. } => vec![identity.clone()],
        InitializationContract::SparsePoints => Vec::new(),
    };
    let owned = known_owned(paths, state, dataset, &extra, cancel).await?;
    if !owned.is_empty() {
        persist_receipt(paths, state, dataset, owned.clone()).await?;
    }
    brush_initialization::prepare_owned_dataset(&paths.project, dataset, contract, &owned, cancel)
        .await
}

pub(super) async fn verify_start(
    paths: &ProjectPaths,
    dataset: &Path,
    contract: &InitializationContract,
    cancel: &CancellationToken,
) -> Result<()> {
    let root = paths.project.clone();
    let dataset = dataset.to_owned();
    let contract = contract.clone();
    let cancel = cancel.clone();
    crate::presets::spawn_pipeline_blocking(move || {
        brush_initialization::verify_before_start(&root, &dataset, &contract, &cancel)
    })
    .await
    .map_err(|error| SplatError::Process(format!("Initialization validation worker: {error}")))?
}
