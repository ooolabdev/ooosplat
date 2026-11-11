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

#[derive(PartialEq, Eq)]
struct FileStamp {
    path: PathBuf,
    canonical_path: PathBuf,
    directory: bool,
    length: u64,
    modified: std::time::SystemTime,
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
            if cached.key == key && cached.stamp == stamp {
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
        let roots = [path.clone()];
        let stamp = file_stamps(&roots, cancel)?;
        if let Some(cached) = &self.database {
            if cached.stamp == stamp {
                return Ok((cached.summary.clone(), cached.file.clone()));
            }
        }
        let summary = validate_database_cancellable(&path, cancel)?;
        let file = describe_file_cancellable(&paths.project, &path, true, cancel)?;
        check_cancel(cancel)?;
        if file_stamps(&roots, cancel)? != stamp {
            self.database = None;
            return Err(untrusted_artifact());
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
    let before = normalization_state(state, &checkpoint)?;
    let previous = state.artifact_manifests.clone();
    let usable_previous = previous.version == ARTIFACT_MANIFEST_VERSION;
    let attempt = attempt_label(run_id);
    // A fully validated immutable candidate is sufficient for publication.
    // Its saved source/config/recipe identity and dataset key must match, but
    // regenerable upstream images/databases no longer need to be on disk.
    if usable_previous
        && state.brush_complete
        && state.retry_cache_pending.is_empty()
        && checkpoint.temporary_png_focal_pending_records.is_none()
        && state.brush_training.dataset_identity.is_some()
    {
        if let (Some(output), Some(saved), Some(dataset)) = (
            state.validated_output.as_ref(),
            previous.entries.get(BRUSH_CANDIDATE),
            previous.entries.get(TRAINING_DATASET),
        ) {
            if saved.semantic_identity.is_some()
                && saved.upstream_identity == dataset.artifact_identity
            {
                let current = usable_validation(
                    capture_candidate_cancellable(
                        paths,
                        state,
                        &attempt,
                        &saved.upstream_identity,
                        cancel,
                    )
                    .map(Some),
                    cancel,
                )?;
                if current.as_ref().is_some_and(|current| {
                    candidate_manifest_match(saved, current)
                        && current.expected_count == output.splat_count
                        && current.files.len() == 1
                        && current.files[0].file_size == output.file_size
                }) {
                    return Ok(false);
                }
            }
        }
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
                .is_some_and(|saved| exact_manifest_match(saved, current))
        });
    state.matching_complete &= matching;
    if matching {
        retained
            .entries
            .insert(MATCHING.into(), previous.entries[MATCHING].clone());
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
            // immutable=1 cannot account for uncheckpointed external writers.
            for suffix in ["-wal", "-journal"] {
                let sidecar = path.with_file_name(format!("database.db{suffix}"));
                match std::fs::symlink_metadata(sidecar) {
                    Ok(metadata) if !metadata.is_file() || metadata.len() != 0 => {
                        return Err(untrusted_artifact());
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
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
    match (
        state.brush_training.execution_options,
        state.brush_training.legacy_dataset_identity.as_ref(),
    ) {
        (None, None) => Ok(legacy_identity),
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
            Ok(format!("{:x}", Sha256::digest(bytes)))
        }
    }
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
    if !state.brush_complete || state.brush_training.legacy_dataset_identity.is_none() {
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
        let info = inspect_gaussian_ply(&path)?;
        let current = make_manifest(
            &saved.attempt_id,
            snapshot
                .brush_training
                .legacy_dataset_identity
                .as_deref()
                .ok_or_else(untrusted_artifact)?,
            info.splat_count,
            Some(candidate_recipe_identity(&snapshot)?),
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
            return Err(untrusted_artifact());
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

#[derive(Debug, Clone)]
struct DatabaseSummary {
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
    let connection = open_immutable_database(path)?;
    let integrity: String = connection
        .query_row("PRAGMA integrity_check(1)", [], |row| row.get(0))
        .map_err(database_error)?;
    if integrity != "ok" {
        return Err(untrusted_artifact());
    }
    check_cancel(cancel)?;
    let cameras = scalar_set(
        &connection,
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

    let keypoints = matrix_rows(&connection, "keypoints", 4, cancel, |cols| {
        matches!(cols, 2 | 4 | 6)
    })?;
    let descriptors = matrix_rows(&connection, "descriptors", 1, cancel, |cols| cols == 128)?;
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
    Ok(DatabaseSummary {
        images,
        feature_identity,
        matching_identity: format!("{:x}", matches_hash.finalize()),
        match_count,
    })
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
    Ok(format!("{:x}", Sha256::digest(bytes)))
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
                .is_err()
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
        normalize_reshoot_v2(&fixture.paths, &mut fixture.state).unwrap();
        assert!(
            !fixture
                .state
                .reshoot
                .as_ref()
                .unwrap()
                .source_snapshot_complete
        );
        assert_eq!(std::fs::read(&candidate).unwrap(), before);
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
