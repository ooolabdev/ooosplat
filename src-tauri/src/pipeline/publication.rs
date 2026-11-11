//! Crash-safe, non-destructive publication of a validated Gaussian result.
//!
//! The transaction file is the durable source of truth for publication. The
//! project metadata, pipeline state, and task registry remain caller-owned
//! views that can be reconciled from a committed transaction.

use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    error::{Result, SplatError},
    project::{
        manager::{atomic_replace_file, atomic_write_json},
        ProjectOutput,
    },
    reconstruction::ply::inspect_gaussian_ply,
};

pub const PUBLICATION_VERSION: u32 = 1;
pub const TRANSACTION_FILE_NAME: &str = "publish-transaction.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PublicationPhase {
    Prepared,
    OutputInstalled,
    Committed,
    /// Project metadata and pipeline state are durable. An optional task
    /// registry entry still has to be persisted by TaskService.
    ProjectViewsReconciled,
    /// Every applicable view, including the task registry, is durable.
    Reconciled,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicationIdentity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<Uuid>,
    pub attempt_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration_identity: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicationTransaction {
    pub version: u32,
    pub transaction_id: Uuid,
    pub identity: PublicationIdentity,
    pub phase: PublicationPhase,
    pub candidate_relative_path: PathBuf,
    pub destination_relative_path: PathBuf,
    pub candidate_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_sha256: Option<String>,
    pub output: ProjectOutput,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub committed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_views_reconciled_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconciled_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct PublicationOutcome {
    pub transaction: PublicationTransaction,
    pub output: ProjectOutput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileDisposition {
    ViewsRequired,
    AlreadyReconciled,
    CandidateRebuildRequired,
}

#[derive(Debug, Clone)]
pub struct ReconcileOutcome {
    pub disposition: ReconcileDisposition,
    pub publication: PublicationOutcome,
}

/// A pending publication with neither payload is only an invalidation intent:
/// reconciliation cannot publish it and must be allowed to request rebuilding.
pub(crate) async fn has_publication_payload(
    project_root: &Path,
    transaction: &PublicationTransaction,
) -> Result<bool> {
    validate_transaction(transaction)?;
    let candidate = resolve_project_path(project_root, &transaction.candidate_relative_path)?;
    let destination = resolve_project_path(project_root, &transaction.destination_relative_path)?;
    Ok(path_exists(&candidate).await? || path_exists(&destination).await?)
}

#[allow(clippy::too_many_arguments)]
pub async fn publish_candidate(
    project_root: &Path,
    transaction_path: &Path,
    candidate: &Path,
    destination: &Path,
    mut output: ProjectOutput,
    identity: PublicationIdentity,
    cancel: &CancellationToken,
) -> Result<PublicationOutcome> {
    let candidate_relative_path = project_relative_path(project_root, candidate)?;
    let destination_relative_path = project_relative_path(project_root, destination)?;
    ensure_transaction_path(project_root, transaction_path)?;
    if candidate == destination {
        return Err(SplatError::Process(
            "Publication candidate and destination must be different files".into(),
        ));
    }
    let candidate_info = inspect_ply(candidate).await?;
    let candidate_sha256 = hash_file(candidate).await?;
    output.final_ply = resolve_project_path(project_root, &destination_relative_path)?;
    output.file_size = candidate_info.file_size;
    output.splat_count = candidate_info.splat_count;
    let mut transaction = PublicationTransaction {
        version: PUBLICATION_VERSION,
        transaction_id: Uuid::new_v4(),
        identity,
        phase: PublicationPhase::Prepared,
        candidate_relative_path,
        destination_relative_path,
        candidate_sha256,
        output_sha256: None,
        output,
        started_at: Utc::now(),
        committed_at: None,
        project_views_reconciled_at: None,
        reconciled_at: None,
    };
    write_transaction(transaction_path, &transaction).await?;
    install_and_commit(project_root, transaction_path, &mut transaction, cancel).await?;
    outcome(project_root, transaction)
}

pub async fn load_transaction(
    project_root: &Path,
    transaction_path: &Path,
) -> Result<Option<PublicationTransaction>> {
    ensure_transaction_path(project_root, transaction_path)?;
    let bytes = match tokio::fs::read(transaction_path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut transaction: PublicationTransaction = serde_json::from_slice(&bytes)?;
    validate_transaction(&transaction)?;
    transaction.output.final_ply =
        resolve_project_path(project_root, &transaction.destination_relative_path)?;
    Ok(Some(transaction))
}

/// Read-only durable completion check used by the project catalog. Prepared or
/// merely installed output is not a completed project; only a verified Commit
/// is authoritative.
pub async fn committed_publication(
    project_root: &Path,
    transaction_path: &Path,
) -> Result<Option<PublicationOutcome>> {
    let Some(transaction) = load_transaction(project_root, transaction_path).await? else {
        return Ok(None);
    };
    if !matches!(
        transaction.phase,
        PublicationPhase::Committed
            | PublicationPhase::ProjectViewsReconciled
            | PublicationPhase::Reconciled
    ) {
        return Ok(None);
    }
    verify_installed_output(project_root, &transaction).await?;
    Ok(Some(outcome(project_root, transaction)?))
}

/// Replays an interrupted publication. A committed result never needs Brush
/// to run again; callers only need to reconcile their metadata/state/task
/// views and then mark the transaction reconciled.
pub async fn reconcile_publication(
    project_root: &Path,
    transaction_path: &Path,
    cancel: &CancellationToken,
) -> Result<Option<ReconcileOutcome>> {
    let Some(mut transaction) = load_transaction(project_root, transaction_path).await? else {
        return Ok(None);
    };
    match transaction.phase {
        PublicationPhase::Prepared => {
            let candidate =
                resolve_project_path(project_root, &transaction.candidate_relative_path)?;
            if path_exists(&candidate).await? {
                install_and_commit(project_root, transaction_path, &mut transaction, cancel)
                    .await?;
            } else {
                let destination =
                    resolve_project_path(project_root, &transaction.destination_relative_path)?;
                if path_exists(&destination).await? {
                    // A final file already present under a Prepared transaction
                    // must prove it belongs to this exact attempt. Otherwise it
                    // is an explicit conflict, never a cache hit.
                    let output_sha256 = verify_output_against_candidate(
                        &destination,
                        &transaction.candidate_sha256,
                        transaction.output.file_size,
                        transaction.output.splat_count,
                    )
                    .await?;
                    transaction.output_sha256 = Some(output_sha256);
                    transaction.phase = PublicationPhase::OutputInstalled;
                    write_transaction(transaction_path, &transaction).await?;
                    commit_installed_output(project_root, transaction_path, &mut transaction)
                        .await?;
                } else {
                    // Keep the forensic transaction in place. The caller can
                    // distinguish this from an absent transaction and clear
                    // only the Brush candidate checkpoint before retraining.
                    return Ok(Some(ReconcileOutcome {
                        disposition: ReconcileDisposition::CandidateRebuildRequired,
                        publication: outcome(project_root, transaction)?,
                    }));
                }
            }
        }
        PublicationPhase::OutputInstalled => {
            commit_installed_output(project_root, transaction_path, &mut transaction).await?;
        }
        PublicationPhase::Committed
        | PublicationPhase::ProjectViewsReconciled
        | PublicationPhase::Reconciled => {
            verify_installed_output(project_root, &transaction).await?;
        }
    }
    let disposition = if transaction.phase == PublicationPhase::Reconciled {
        ReconcileDisposition::AlreadyReconciled
    } else {
        ReconcileDisposition::ViewsRequired
    };
    Ok(Some(ReconcileOutcome {
        disposition,
        publication: outcome(project_root, transaction)?,
    }))
}

pub async fn mark_reconciled(
    project_root: &Path,
    transaction_path: &Path,
    transaction_id: Uuid,
) -> Result<PublicationOutcome> {
    let mut transaction = load_transaction(project_root, transaction_path)
        .await?
        .ok_or_else(|| SplatError::Process("Publication transaction is missing".into()))?;
    if transaction.transaction_id != transaction_id {
        return Err(SplatError::Process(
            "Publication transaction identity changed before reconciliation".into(),
        ));
    }
    if !matches!(
        transaction.phase,
        PublicationPhase::Committed
            | PublicationPhase::ProjectViewsReconciled
            | PublicationPhase::Reconciled
    ) {
        return Err(SplatError::Process(
            "Publication cannot be reconciled before its result is committed".into(),
        ));
    }
    verify_installed_output(project_root, &transaction).await?;
    transaction.phase = PublicationPhase::Reconciled;
    transaction.reconciled_at.get_or_insert_with(Utc::now);
    write_transaction(transaction_path, &transaction).await?;
    outcome(project_root, transaction)
}

/// Mark the project metadata and pipeline state as reconciled without
/// claiming that an optional TaskService registry entry is already durable.
pub async fn mark_project_views_reconciled(
    project_root: &Path,
    transaction_path: &Path,
    transaction_id: Uuid,
) -> Result<PublicationOutcome> {
    let mut transaction = load_transaction(project_root, transaction_path)
        .await?
        .ok_or_else(|| SplatError::Process("Publication transaction is missing".into()))?;
    if transaction.transaction_id != transaction_id {
        return Err(SplatError::Process(
            "Publication transaction identity changed before reconciliation".into(),
        ));
    }
    if !matches!(
        transaction.phase,
        PublicationPhase::Committed
            | PublicationPhase::ProjectViewsReconciled
            | PublicationPhase::Reconciled
    ) {
        return Err(SplatError::Process(
            "Project views cannot be reconciled before publication commits".into(),
        ));
    }
    verify_installed_output(project_root, &transaction).await?;
    if transaction.phase == PublicationPhase::Committed {
        transaction.phase = PublicationPhase::ProjectViewsReconciled;
        transaction
            .project_views_reconciled_at
            .get_or_insert_with(Utc::now);
        write_transaction(transaction_path, &transaction).await?;
    }
    outcome(project_root, transaction)
}

async fn install_and_commit(
    project_root: &Path,
    transaction_path: &Path,
    transaction: &mut PublicationTransaction,
    cancel: &CancellationToken,
) -> Result<()> {
    validate_transaction(transaction)?;
    let candidate = resolve_project_path(project_root, &transaction.candidate_relative_path)?;
    let destination = resolve_project_path(project_root, &transaction.destination_relative_path)?;
    let current_candidate_hash = hash_file(&candidate).await?;
    if current_candidate_hash != transaction.candidate_sha256 {
        return Err(SplatError::Process(
            "Publication candidate changed after it was prepared".into(),
        ));
    }
    let current_info = inspect_ply(&candidate).await?;
    if current_info.file_size != transaction.output.file_size
        || current_info.splat_count != transaction.output.splat_count
    {
        return Err(SplatError::Process(
            "Publication candidate no longer matches its prepared result summary".into(),
        ));
    }
    if cancel.is_cancelled() {
        return Err(SplatError::Cancelled);
    }
    let parent = destination
        .parent()
        .ok_or_else(|| SplatError::InvalidPath(destination.clone()))?;
    tokio::fs::create_dir_all(parent).await?;
    let temporary = destination.with_file_name(format!(
        ".publication-{}-{}.tmp",
        transaction.transaction_id,
        destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("final.ply")
    ));
    let install = async {
        let copied_sha256 = copy_and_hash(&candidate, &temporary, cancel).await?;
        if copied_sha256 != transaction.candidate_sha256 {
            return Err(SplatError::Process(
                "Publication candidate changed while it was being copied".into(),
            ));
        }
        if cancel.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        atomic_replace_file(&temporary, &destination).await
    }
    .await;
    if install.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
        return install;
    }
    let output_sha256 = verify_output_against_candidate(
        &destination,
        &transaction.candidate_sha256,
        transaction.output.file_size,
        transaction.output.splat_count,
    )
    .await?;
    transaction.output_sha256 = Some(output_sha256);
    transaction.phase = PublicationPhase::OutputInstalled;
    write_transaction(transaction_path, transaction).await?;
    commit_installed_output(project_root, transaction_path, transaction).await
}

async fn commit_installed_output(
    project_root: &Path,
    transaction_path: &Path,
    transaction: &mut PublicationTransaction,
) -> Result<()> {
    verify_installed_output(project_root, transaction).await?;
    transaction.phase = PublicationPhase::Committed;
    transaction.committed_at.get_or_insert_with(Utc::now);
    write_transaction(transaction_path, transaction).await
}

async fn verify_installed_output(
    project_root: &Path,
    transaction: &PublicationTransaction,
) -> Result<()> {
    validate_transaction(transaction)?;
    let destination = resolve_project_path(project_root, &transaction.destination_relative_path)?;
    let expected_hash = transaction.output_sha256.as_deref().ok_or_else(|| {
        SplatError::Process("Committed publication is missing its output fingerprint".into())
    })?;
    verify_output_against_candidate(
        &destination,
        expected_hash,
        transaction.output.file_size,
        transaction.output.splat_count,
    )
    .await?;
    Ok(())
}

async fn verify_output_against_candidate(
    output: &Path,
    expected_hash: &str,
    expected_size: u64,
    expected_splats: u64,
) -> Result<String> {
    let info = inspect_ply(output).await?;
    if info.file_size != expected_size || info.splat_count != expected_splats {
        return Err(SplatError::Process(
            "Published final.ply does not match its committed result summary".into(),
        ));
    }
    let actual_hash = hash_file(output).await?;
    if actual_hash != expected_hash {
        return Err(SplatError::Process(
            "Published final.ply does not belong to this publication attempt".into(),
        ));
    }
    Ok(actual_hash)
}

async fn inspect_ply(path: &Path) -> Result<crate::reconstruction::ply::PlyInfo> {
    let path = path.to_path_buf();
    crate::presets::spawn_pipeline_blocking(move || inspect_gaussian_ply(&path))
        .await
        .map_err(|error| SplatError::Process(format!("PLY validation task failed: {error}")))?
}

async fn hash_file(path: &Path) -> Result<String> {
    let mut input = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = input.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

async fn copy_and_hash(source: &Path, target: &Path, cancel: &CancellationToken) -> Result<String> {
    let mut input = tokio::fs::File::open(source).await?;
    let mut output = tokio::fs::File::create(target).await?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        if cancel.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        let read = input.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        output.write_all(&buffer[..read]).await?;
        hasher.update(&buffer[..read]);
    }
    output.flush().await?;
    output.sync_all().await?;
    Ok(format!("{:x}", hasher.finalize()))
}

async fn write_transaction(
    transaction_path: &Path,
    transaction: &PublicationTransaction,
) -> Result<()> {
    if let Some(parent) = transaction_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    atomic_write_json(transaction_path, transaction).await
}

async fn path_exists(path: &Path) -> Result<bool> {
    match tokio::fs::metadata(path).await {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn outcome(
    project_root: &Path,
    mut transaction: PublicationTransaction,
) -> Result<PublicationOutcome> {
    transaction.output.final_ply =
        resolve_project_path(project_root, &transaction.destination_relative_path)?;
    Ok(PublicationOutcome {
        output: transaction.output.clone(),
        transaction,
    })
}

fn validate_transaction(transaction: &PublicationTransaction) -> Result<()> {
    if transaction.version != PUBLICATION_VERSION {
        return Err(SplatError::Process(format!(
            "Unsupported publication transaction version {}",
            transaction.version
        )));
    }
    validate_relative_path(&transaction.candidate_relative_path)?;
    validate_relative_path(&transaction.destination_relative_path)?;
    if transaction.identity.attempt_id.trim().is_empty() {
        return Err(SplatError::Process(
            "Publication attempt identity is empty".into(),
        ));
    }
    if transaction.candidate_sha256.len() != 64
        || transaction
            .output_sha256
            .as_ref()
            .is_some_and(|hash| hash.len() != 64)
    {
        return Err(SplatError::Process(
            "Publication fingerprint is malformed".into(),
        ));
    }
    Ok(())
}

fn project_relative_path(project_root: &Path, path: &Path) -> Result<PathBuf> {
    let relative = path
        .strip_prefix(project_root)
        .map_err(|_| SplatError::InvalidPath(path.to_path_buf()))?
        .to_path_buf();
    validate_relative_path(&relative)?;
    Ok(relative)
}

fn resolve_project_path(project_root: &Path, relative: &Path) -> Result<PathBuf> {
    validate_relative_path(relative)?;
    Ok(project_root.join(relative))
}

fn ensure_transaction_path(project_root: &Path, path: &Path) -> Result<()> {
    let relative = path
        .strip_prefix(project_root)
        .map_err(|_| SplatError::InvalidPath(path.to_path_buf()))?;
    validate_relative_path(relative)
}

fn validate_relative_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(SplatError::InvalidPath(path.to_path_buf()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_valid_ply(path: &Path, points: u64) {
        std::fs::write(
            path,
            format!(
                "ply\nformat binary_little_endian 1.0\nelement vertex {points}\nproperty float x\nproperty float y\nproperty float z\nproperty float f_dc_0\nproperty float opacity\nproperty float scale_0\nproperty float rot_0\nend_header\n"
            ),
        )
        .unwrap();
    }

    fn output() -> ProjectOutput {
        ProjectOutput {
            input_images: 10,
            registered_images: 8,
            registered_ratio: 0.8,
            points_3d: 42,
            ..Default::default()
        }
    }

    fn identity() -> PublicationIdentity {
        PublicationIdentity {
            run_id: Some(Uuid::new_v4()),
            attempt_id: "brush-0".into(),
            source_identity: Some("source".into()),
            configuration_identity: Some("config".into()),
        }
    }

    #[tokio::test]
    async fn publish_preserves_candidate_and_commits_verified_output() {
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("work/brush/final.ply.tmp");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        write_valid_ply(&candidate, 7);
        let final_ply = root.path().join("final.ply");
        let transaction_path = root.path().join("work").join(TRANSACTION_FILE_NAME);
        let result = publish_candidate(
            root.path(),
            &transaction_path,
            &candidate,
            &final_ply,
            output(),
            identity(),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(candidate.is_file());
        assert!(final_ply.is_file());
        assert_eq!(result.output.splat_count, 7);
        assert_eq!(result.transaction.phase, PublicationPhase::Committed);
        assert_eq!(
            result.transaction.candidate_sha256,
            result.transaction.output_sha256.unwrap()
        );
    }

    #[tokio::test]
    async fn committed_publication_reconciles_without_republishing() {
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("work/brush/final.ply.tmp");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        write_valid_ply(&candidate, 3);
        let final_ply = root.path().join("final.ply");
        let transaction_path = root.path().join("work").join(TRANSACTION_FILE_NAME);
        let published = publish_candidate(
            root.path(),
            &transaction_path,
            &candidate,
            &final_ply,
            output(),
            identity(),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        std::fs::remove_file(&candidate).unwrap();
        let recovered =
            reconcile_publication(root.path(), &transaction_path, &CancellationToken::new())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(recovered.disposition, ReconcileDisposition::ViewsRequired);
        assert_eq!(
            recovered.publication.transaction.transaction_id,
            published.transaction.transaction_id
        );
        assert!(!candidate.exists());
        assert!(final_ply.is_file());
    }

    #[tokio::test]
    async fn prepared_publication_installs_and_verifies_before_commit() {
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("work/brush/final.ply.tmp");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        write_valid_ply(&candidate, 4);
        let final_ply = root.path().join("final.ply");
        write_valid_ply(&final_ply, 2);
        let transaction_path = root.path().join("work").join(TRANSACTION_FILE_NAME);
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert!(matches!(
            publish_candidate(
                root.path(),
                &transaction_path,
                &candidate,
                &final_ply,
                output(),
                identity(),
                &cancelled,
            )
            .await,
            Err(SplatError::Cancelled)
        ));
        let prepared = load_transaction(root.path(), &transaction_path)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(prepared.phase, PublicationPhase::Prepared);

        let recovered =
            reconcile_publication(root.path(), &transaction_path, &CancellationToken::new())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            recovered.publication.transaction.phase,
            PublicationPhase::Committed
        );
        assert_eq!(recovered.publication.output.splat_count, 4);
        assert_eq!(
            hash_file(&final_ply).await.unwrap(),
            recovered
                .publication
                .transaction
                .output_sha256
                .as_deref()
                .unwrap()
        );
    }

    #[tokio::test]
    async fn output_installed_is_reverified_before_commit() {
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("work/brush/final.ply.tmp");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        write_valid_ply(&candidate, 6);
        let final_ply = root.path().join("final.ply");
        let transaction_path = root.path().join("work").join(TRANSACTION_FILE_NAME);
        publish_candidate(
            root.path(),
            &transaction_path,
            &candidate,
            &final_ply,
            output(),
            identity(),
            &CancellationToken::new(),
        )
        .await
        .unwrap();

        let mut interrupted = load_transaction(root.path(), &transaction_path)
            .await
            .unwrap()
            .unwrap();
        interrupted.phase = PublicationPhase::OutputInstalled;
        interrupted.committed_at = None;
        write_transaction(&transaction_path, &interrupted)
            .await
            .unwrap();

        let recovered =
            reconcile_publication(root.path(), &transaction_path, &CancellationToken::new())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            recovered.publication.transaction.phase,
            PublicationPhase::Committed
        );
        assert!(recovered.publication.transaction.committed_at.is_some());
    }

    #[tokio::test]
    async fn prepared_without_candidate_commits_an_exact_installed_output() {
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("work/brush/final.ply.tmp");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        write_valid_ply(&candidate, 6);
        let final_ply = root.path().join("final.ply");
        std::fs::copy(&candidate, &final_ply).unwrap();
        let transaction_path = root.path().join("work").join(TRANSACTION_FILE_NAME);
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert!(matches!(
            publish_candidate(
                root.path(),
                &transaction_path,
                &candidate,
                &final_ply,
                output(),
                identity(),
                &cancelled,
            )
            .await,
            Err(SplatError::Cancelled)
        ));
        std::fs::remove_file(&candidate).unwrap();

        let recovered =
            reconcile_publication(root.path(), &transaction_path, &CancellationToken::new())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(recovered.disposition, ReconcileDisposition::ViewsRequired);
        assert_eq!(
            recovered.publication.transaction.phase,
            PublicationPhase::Committed
        );
        assert_eq!(
            recovered.publication.transaction.output_sha256.as_deref(),
            Some(recovered.publication.transaction.candidate_sha256.as_str())
        );
    }

    #[tokio::test]
    async fn prepared_without_candidate_or_output_requires_candidate_rebuild() {
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("work/brush/final.ply.tmp");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        write_valid_ply(&candidate, 6);
        let final_ply = root.path().join("final.ply");
        let transaction_path = root.path().join("work").join(TRANSACTION_FILE_NAME);
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert!(matches!(
            publish_candidate(
                root.path(),
                &transaction_path,
                &candidate,
                &final_ply,
                output(),
                identity(),
                &cancelled,
            )
            .await,
            Err(SplatError::Cancelled)
        ));
        std::fs::remove_file(&candidate).unwrap();

        let recovered =
            reconcile_publication(root.path(), &transaction_path, &CancellationToken::new())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            recovered.disposition,
            ReconcileDisposition::CandidateRebuildRequired
        );
        assert_eq!(
            recovered.publication.transaction.phase,
            PublicationPhase::Prepared
        );
        assert!(transaction_path.is_file());
    }

    #[tokio::test]
    async fn prepared_without_candidate_rejects_a_different_installed_output() {
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("work/brush/final.ply.tmp");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        write_valid_ply(&candidate, 6);
        let final_ply = root.path().join("final.ply");
        write_valid_ply(&final_ply, 9);
        let transaction_path = root.path().join("work").join(TRANSACTION_FILE_NAME);
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert!(matches!(
            publish_candidate(
                root.path(),
                &transaction_path,
                &candidate,
                &final_ply,
                output(),
                identity(),
                &cancelled,
            )
            .await,
            Err(SplatError::Cancelled)
        ));
        std::fs::remove_file(&candidate).unwrap();

        let error =
            reconcile_publication(root.path(), &transaction_path, &CancellationToken::new())
                .await
                .unwrap_err();
        assert!(error
            .to_string()
            .contains("does not match its committed result summary"));
        assert!(transaction_path.is_file());
    }

    #[tokio::test]
    async fn reconciliation_rebuilds_output_path_after_project_move() {
        let container = tempfile::tempdir().unwrap();
        let original_root = container.path().join("original-project");
        std::fs::create_dir_all(&original_root).unwrap();
        let candidate = original_root.join("work/brush/final.ply.tmp");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        write_valid_ply(&candidate, 6);
        let final_ply = original_root.join("final.ply");
        let transaction_path = original_root.join("work").join(TRANSACTION_FILE_NAME);
        let published = publish_candidate(
            &original_root,
            &transaction_path,
            &candidate,
            &final_ply,
            output(),
            identity(),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(published.output.final_ply, final_ply);

        let moved_root = container.path().join("moved-project");
        std::fs::rename(&original_root, &moved_root).unwrap();
        let moved_transaction = moved_root.join("work").join(TRANSACTION_FILE_NAME);
        let recovered =
            reconcile_publication(&moved_root, &moved_transaction, &CancellationToken::new())
                .await
                .unwrap()
                .unwrap();
        let expected_final = moved_root.join("final.ply");
        assert_eq!(recovered.publication.output.final_ply, expected_final);
        assert_eq!(
            recovered.publication.transaction.output.final_ply,
            expected_final
        );
    }

    #[tokio::test]
    async fn reconciled_transaction_is_idempotent() {
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("work/brush/final.ply.tmp");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        write_valid_ply(&candidate, 5);
        let final_ply = root.path().join("final.ply");
        let transaction_path = root.path().join("work").join(TRANSACTION_FILE_NAME);
        let published = publish_candidate(
            root.path(),
            &transaction_path,
            &candidate,
            &final_ply,
            output(),
            identity(),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        mark_reconciled(
            root.path(),
            &transaction_path,
            published.transaction.transaction_id,
        )
        .await
        .unwrap();
        let recovered =
            reconcile_publication(root.path(), &transaction_path, &CancellationToken::new())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            recovered.disposition,
            ReconcileDisposition::AlreadyReconciled
        );
    }

    #[tokio::test]
    async fn project_and_task_views_are_reconciled_in_separate_durable_phases() {
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("work/brush/final.ply.tmp");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        write_valid_ply(&candidate, 5);
        let final_ply = root.path().join("final.ply");
        let transaction_path = root.path().join("work").join(TRANSACTION_FILE_NAME);
        let published = publish_candidate(
            root.path(),
            &transaction_path,
            &candidate,
            &final_ply,
            output(),
            identity(),
            &CancellationToken::new(),
        )
        .await
        .unwrap();

        let project_views = mark_project_views_reconciled(
            root.path(),
            &transaction_path,
            published.transaction.transaction_id,
        )
        .await
        .unwrap();
        assert_eq!(
            project_views.transaction.phase,
            PublicationPhase::ProjectViewsReconciled
        );
        assert!(project_views
            .transaction
            .project_views_reconciled_at
            .is_some());
        assert!(project_views.transaction.reconciled_at.is_none());
        let pending_task =
            reconcile_publication(root.path(), &transaction_path, &CancellationToken::new())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            pending_task.disposition,
            ReconcileDisposition::ViewsRequired
        );

        let reconciled = mark_reconciled(
            root.path(),
            &transaction_path,
            published.transaction.transaction_id,
        )
        .await
        .unwrap();
        assert_eq!(reconciled.transaction.phase, PublicationPhase::Reconciled);
        assert!(reconciled.transaction.reconciled_at.is_some());
    }

    #[tokio::test]
    async fn rejects_output_from_another_attempt() {
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("work/brush/final.ply.tmp");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        write_valid_ply(&candidate, 5);
        let final_ply = root.path().join("final.ply");
        let transaction_path = root.path().join("work").join(TRANSACTION_FILE_NAME);
        publish_candidate(
            root.path(),
            &transaction_path,
            &candidate,
            &final_ply,
            output(),
            identity(),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        write_valid_ply(&final_ply, 9);
        let error =
            reconcile_publication(root.path(), &transaction_path, &CancellationToken::new())
                .await
                .unwrap_err();
        assert!(error
            .to_string()
            .contains("does not match its committed result summary"));
    }

    #[tokio::test]
    async fn cancellation_before_install_keeps_existing_output() {
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("work/brush/final.ply.tmp");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        write_valid_ply(&candidate, 5);
        let final_ply = root.path().join("final.ply");
        write_valid_ply(&final_ply, 2);
        let old = std::fs::read(&final_ply).unwrap();
        let transaction_path = root.path().join("work").join(TRANSACTION_FILE_NAME);
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(matches!(
            publish_candidate(
                root.path(),
                &transaction_path,
                &candidate,
                &final_ply,
                output(),
                identity(),
                &cancel,
            )
            .await,
            Err(SplatError::Cancelled)
        ));
        assert_eq!(std::fs::read(&final_ply).unwrap(), old);
        assert!(candidate.is_file());
    }
}
