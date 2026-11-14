//! Task-owned database recovery. Source projects and immutable snapshots are
//! never opened for writing. The persisted barrier precedes every archive move.
use super::*;

impl PipelineRunner {
    pub(super) async fn recover_work_order_database(
        &self,
        manager: &ProjectManager,
        paths: &ProjectPaths,
        state: &mut PipelineStateFile,
    ) -> Result<()> {
        if self.process_manager.has_active_processes() {
            return Err(SplatError::Io(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "COLMAP database recovery cannot run while an engine is active",
            )));
        }
        if state.database_recovery_pending.is_some() {
            self.finish_database_rebuild(manager, paths, state).await?;
        }
        let owned_paths = paths.clone();
        let saved = Box::new(state.clone());
        let cancel = self.process_manager.child_token();
        let result = self.retry_preparation_step(
            paths, RetryPreparationOperation::RecoverWorkingDatabase,
            "colmap_database", Some("colmap"), async move {
                crate::presets::spawn_pipeline_blocking(move || {
                    crate::pipeline::artifacts::recover_working_database(&owned_paths, &saved, &cancel)
                }).await.map_err(|error| SplatError::Process(error.to_string()))?
                    .map_err(|error| {
                        let reason = if error.io_error().is_some_and(|io| io.kind() == std::io::ErrorKind::WouldBlock) {
                            "exclusive_lock_unavailable_or_database_changed"
                        } else { "recovery_failed" };
                        error.with_io_diagnostic(format!(
                            "recovery_operation=recover_working_database\npath_role=colmap_database\ndatabase_recovery_result={reason}"
                        ))
                    })
            },
        ).await?;
        let (outcome, reason) = match result {
            crate::pipeline::artifacts::WorkingDatabaseRecovery::NotNeeded => {
                ("not_needed", "no_pending_writes")
            }
            crate::pipeline::artifacts::WorkingDatabaseRecovery::Reusable => {
                ("recovered", "verified_cache")
            }
            crate::pipeline::artifacts::WorkingDatabaseRecovery::RebuildRequired { reason } => {
                ("cache_rebuild_required", reason)
            }
        };
        self.retry_preparation_context(paths)
            .record_database_recovery(outcome, reason)
            .await;
        if let crate::pipeline::artifacts::WorkingDatabaseRecovery::RebuildRequired { reason } =
            result
        {
            self.begin_database_rebuild(manager, paths, state, reason)
                .await?;
        }
        Ok(())
    }

    pub(super) async fn begin_database_rebuild(
        &self,
        manager: &ProjectManager,
        paths: &ProjectPaths,
        state: &mut PipelineStateFile,
        reason: &str,
    ) -> Result<()> {
        self.validate_database_rebuild_base(paths, state).await?;
        crate::pipeline::artifacts::invalidate_database_for_rebuild(state);
        state.database_recovery_pending = Some(reason.to_owned());
        // prepare_continuation writes the durable barrier before any move.
        self.finish_database_rebuild(manager, paths, state).await
    }

    pub(super) async fn validate_database_rebuild_base(
        &self,
        paths: &ProjectPaths,
        state: &PipelineStateFile,
    ) -> Result<()> {
        let owned_paths = paths.clone();
        let gps = state.gps.clone();
        crate::presets::spawn_pipeline_blocking(move || {
            validate_recovery_gps_manifest(&owned_paths, gps.as_ref())
        })
        .await
        .map_err(|error| SplatError::Process(error.to_string()))??;
        if state.reshoot.is_none()
            && !state
                .inputs
                .as_ref()
                .is_some_and(inputs_runner::execution_plan_is_reshoot)
        {
            return Ok(());
        }
        let expected = state
            .reshoot
            .as_ref()
            .filter(|saved| saved.snapshot_v3_ready)
            .and_then(|saved| saved.snapshot_v3_identity.clone())
            .ok_or_else(|| SplatError::Process(legacy_work_order::UNRESTORABLE_RECIPE.into()))?;
        let root = paths.work.join("reshoot-snapshot-v3");
        crate::presets::spawn_pipeline_blocking(move || {
            let manifest = crate::pipeline::reshoot_snapshot::verify_snapshot(&root)?;
            if manifest.content_identity != expected {
                return Err(SplatError::Process(
                    "Reshoot source snapshot identity changed".into(),
                ));
            }
            Ok(())
        })
        .await
        .map_err(|error| SplatError::Process(error.to_string()))?
    }

    pub(super) async fn finish_database_rebuild(
        &self,
        manager: &ProjectManager,
        paths: &ProjectPaths,
        state: &mut PipelineStateFile,
    ) -> Result<()> {
        if state.database_recovery_pending.is_none() {
            return Ok(());
        }
        // Also checked on resumed barriers: a damaged/missing base is not
        // permission to re-extract old features against old point tracks.
        self.validate_database_rebuild_base(paths, state).await?;
        self.prepare_continuation(manager, paths, state).await?;
        tokio::fs::create_dir_all(&paths.colmap).await?;
        tokio::fs::create_dir_all(&paths.brush).await?;
        self.restore_collection_database_base(paths, state).await?;
        state.database_recovery_pending = None;
        self.retry_preparation_step(
            paths,
            RetryPreparationOperation::WriteRetryArchiveProgress,
            "project_state",
            Some("colmap"),
            manager.write_state_for_preparation(&paths.state, state),
        )
        .await
    }

    pub(super) async fn restore_collection_database_base(
        &self,
        paths: &ProjectPaths,
        state: &mut PipelineStateFile,
    ) -> Result<()> {
        if !state
            .inputs
            .as_ref()
            .is_some_and(inputs_runner::execution_plan_is_reshoot)
        {
            return Ok(());
        }
        let expected = state
            .reshoot
            .as_ref()
            .and_then(|reshoot| reshoot.snapshot_v3_identity.clone());
        let owned_paths = paths.clone();
        let cancel = self.process_manager.child_token();
        crate::presets::spawn_pipeline_blocking(move || {
            restore_private_collection_base(&owned_paths, expected.as_deref(), &cancel)
        })
        .await
        .map_err(|error| SplatError::Process(error.to_string()))??;
        let owned_paths = paths.clone();
        let mut restored = Box::new(state.clone());
        let cancel = self.process_manager.child_token();
        let restored = crate::presets::spawn_pipeline_blocking(move || {
            crate::pipeline::artifacts::rebind_restored_focal_base(
                &owned_paths,
                &mut restored,
                &cancel,
            )?;
            Ok::<_, SplatError>(restored)
        })
        .await
        .map_err(|error| SplatError::Process(error.to_string()))??;
        *state = *restored;
        Ok(())
    }
}

/// work/gps contains input observations, not generated geometry (which lives
/// below colmap/gps-v1). Preserve only an identity-verified local manifest.
fn validate_recovery_gps_manifest(
    paths: &ProjectPaths,
    checkpoint: Option<&crate::reconstruction::gps::Checkpoint>,
) -> Result<()> {
    let Some(checkpoint) = checkpoint else {
        return Ok(());
    };
    let path = paths.work.join("gps/manifest.json");
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(SplatError::Process(
                "GPS recovery manifest is not a regular file".into(),
            ));
        }
        Ok(_) => {}
    }
    crate::pipeline::artifacts::validate_recovery_path(&paths.project, &path)?;
    let manifest: crate::reconstruction::gps::Manifest =
        serde_json::from_slice(&std::fs::read(path)?)?;
    if manifest.version != crate::reconstruction::gps::POLICY_VERSION
        || checkpoint.fingerprint != crate::reconstruction::gps::fingerprint(&manifest.records)?
        || checkpoint.manifest_identity.as_deref()
            != Some(crate::reconstruction::gps::manifest_identity(&manifest)?.as_str())
    {
        return Err(SplatError::Process(
            "GPS recovery manifest does not match saved input identity".into(),
        ));
    }
    Ok(())
}

fn private_base_digest(
    path: &Path,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut source = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        if cancel.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        let length = source.read(&mut buffer)?;
        if length == 0 {
            break;
        }
        hash.update(&buffer[..length]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

/// An interrupted restore may already have installed a verified target. Read it
/// without rewriting; any unknown, redirected or shared destination is refused.
fn private_base_target_matches(
    paths: &ProjectPaths,
    target: &Path,
    expected: &str,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<bool> {
    match std::fs::symlink_metadata(target) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(SplatError::Process(
                "Unsafe reshoot database recovery target".into(),
            ));
        }
        Ok(_) => {}
    }
    crate::pipeline::artifacts::validate_recovery_path(&paths.project, target)?;
    crate::pipeline::artifacts::reject_shared_recovery_file(target)?;
    if private_base_digest(target, cancel)? != expected {
        return Err(SplatError::Process(
            "Reshoot database recovery target has unexpected content".into(),
        ));
    }
    Ok(true)
}

#[cfg(windows)]
fn install_private_base_file(source: &Path, target: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};
    let source = std::fs::canonicalize(source)?;
    let parent = target
        .parent()
        .ok_or_else(|| SplatError::Process("Invalid private database target".into()))?;
    let name = target
        .file_name()
        .ok_or_else(|| SplatError::Process("Invalid private database target".into()))?;
    let target = std::fs::canonicalize(parent)?.join(name);
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let target = target
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    // Both NUL-terminated buffers stay alive. Deliberately omit REPLACE_EXISTING
    // (std::fs::rename does not provide a no-clobber guarantee on Windows).
    if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(not(windows))]
fn install_private_base_file(source: &Path, target: &Path) -> Result<()> {
    // Never link to the original snapshot: source is a private staged copy.
    std::fs::hard_link(source, target)?;
    std::fs::remove_file(source)?;
    Ok(())
}

fn restore_private_collection_base(
    paths: &ProjectPaths,
    expected_identity: Option<&str>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<()> {
    use std::io::{Read, Write};
    let snapshot = paths.work.join("reshoot-snapshot-v3");
    let manifest = crate::pipeline::reshoot_snapshot::verify_snapshot(&snapshot)?;
    if expected_identity != Some(manifest.content_identity.as_str()) {
        return Err(SplatError::Process(
            "Reshoot source snapshot identity changed".into(),
        ));
    }
    let expected_database = manifest
        .assets
        .iter()
        .find(|asset| asset.relative_path == "colmap/database.db")
        .ok_or_else(|| SplatError::Process("Reshoot source snapshot has no database".into()))?;
    crate::pipeline::artifacts::validate_recovery_path(&paths.project, &paths.colmap)?;
    // Never open a pre-existing final destination for writing. This also makes
    // an interrupted restore unable to modify a linked source/snapshot file.
    let staging = paths
        .colmap
        .join(format!(".database-base-recovery-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&staging)?;
    let result = (|| -> Result<()> {
        let model = staging.join("base-model");
        crate::reconstruction::direct_input::read_model(&snapshot.join("colmap/sparse"))?
            .write(&model)?;
        let database = staging.join("base-database.db");
        let mut input = std::fs::File::open(snapshot.join("colmap/database.db"))?;
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&database)?;
        let mut buffer = vec![0u8; 1024 * 1024];
        loop {
            if cancel.is_cancelled() {
                return Err(SplatError::Cancelled);
            }
            let length = input.read(&mut buffer)?;
            if length == 0 {
                break;
            }
            output.write_all(&buffer[..length])?;
        }
        output.sync_all()?;
        drop(output);
        if private_base_digest(&database, cancel)? != expected_database.sha256 {
            return Err(SplatError::Process(
                "Reshoot database snapshot changed during copy".into(),
            ));
        }
        let final_database = paths.colmap.join("base-database.db");
        for suffix in ["-wal", "-journal", "-shm"] {
            let path = paths.colmap.join(format!("base-database.db{suffix}"));
            match std::fs::symlink_metadata(path) {
                Ok(_) => {
                    return Err(SplatError::Process(
                        "Reshoot base database has an unexpected sidecar".into(),
                    ))
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        let database_installed =
            private_base_target_matches(paths, &final_database, &expected_database.sha256, cancel)?;
        let final_model = paths.colmap.join("base-model");
        let model_installed = match std::fs::symlink_metadata(&final_model) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
            Ok(metadata) if !metadata.file_type().is_dir() => {
                return Err(SplatError::Process(
                    "Unsafe reshoot model recovery target".into(),
                ))
            }
            Ok(_) => {
                crate::pipeline::artifacts::validate_recovery_path(&paths.project, &final_model)?;
                let entries =
                    std::fs::read_dir(&final_model)?.collect::<std::io::Result<Vec<_>>>()?;
                if entries.len() != 3 {
                    return Err(SplatError::Process(
                        "Reshoot model recovery target has unexpected content".into(),
                    ));
                }
                for name in ["cameras.bin", "images.bin", "points3D.bin"] {
                    let expected = private_base_digest(&model.join(name), cancel)?;
                    if !private_base_target_matches(
                        paths,
                        &final_model.join(name),
                        &expected,
                        cancel,
                    )? {
                        return Err(SplatError::Process(
                            "Reshoot model recovery target is incomplete".into(),
                        ));
                    }
                }
                true
            }
        };
        // Revalidate copied model against the frozen source before publishing.
        if crate::pipeline::reshoot_snapshot::verify_snapshot(&snapshot)?.content_identity
            != manifest.content_identity
        {
            return Err(SplatError::Process(
                "Reshoot source snapshot identity changed".into(),
            ));
        }
        for name in ["cameras.bin", "images.bin", "points3D.bin"] {
            std::fs::OpenOptions::new()
                .write(true)
                .open(model.join(name))?
                .sync_all()?;
        }
        if !model_installed {
            std::fs::rename(&model, &final_model)?;
        }
        if !database_installed {
            install_private_base_file(&database, &final_database)?;
        }
        Ok(())
    })();
    // This UUID directory was created exclusively above and contains only this
    // call's private files. Cleanup never takes precedence over the main error.
    let _ = std::fs::remove_dir_all(&staging);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::execution_plan::{ExecutionPlan, MatchPlan};
    use crate::pipeline::inputs::{GenerationInputs, InputCheckpoint, SourceCheckpoint};
    use crate::project::{FrameState, ReshootState};
    use crate::video::{panorama::VideoProjection, PlannedFrame};

    struct Fixture {
        _root: tempfile::TempDir,
        paths: ProjectPaths,
        manager: ProjectManager,
        runner: PipelineRunner,
        state: PipelineStateFile,
    }

    fn fixture() -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("project"));
        for directory in [
            &paths.frames,
            &paths.colmap_frames,
            &paths.masks,
            &paths.colmap,
            &paths.brush,
            &paths.logs,
        ] {
            std::fs::create_dir_all(directory).unwrap();
            std::fs::write(directory.join("sentinel"), b"keep-or-archive").unwrap();
        }
        for name in [
            "database.db",
            "database.db-wal",
            "database.db-journal",
            "database.db-shm",
        ] {
            std::fs::write(paths.colmap.join(name), name.as_bytes()).unwrap();
        }
        std::fs::write(paths.project.join("final.ply"), b"published model").unwrap();
        std::fs::write(paths.project.join("editing.json"), b"saved edits").unwrap();
        let manager = ProjectManager::with_root(root.path().into());
        let runner =
            PipelineRunner::new(EnginePaths::from_root(root.path().join("engines")), |_| {});
        let mut state = PipelineStateFile::created(Quality::Balanced);
        state.features_complete = true;
        state.matching_complete = true;
        state.reconstruction_complete = true;
        state.generation_policy = Some(crate::pipeline::generation_policy::GenerationPolicy {
            version: 5,
            ..Default::default()
        });
        state.brush_training.resolved = Some(crate::presets::resolve_brush_training_preset(
            Quality::Balanced,
            true,
            None,
            1024,
            10,
        ));
        state.brush_training.initial_resolved = state.brush_training.resolved;
        state.brush_training.oom_retry_used = true;
        state.brush_training.oom_retry_started = true;
        state.gps = Some(crate::reconstruction::gps::Checkpoint {
            status: "fallback".into(),
            ..Default::default()
        });
        state.bridge_backfill.status = BridgeBackfillStatus::Running;
        let mut checkpoint = InputCheckpoint::new(GenerationInputs::Materials { items: vec![] });
        checkpoint.prepared = true;
        checkpoint.bridge_used = true;
        checkpoint.expanded_matching = true;
        let mut plan = ExecutionPlan::normal();
        plan.fallback.reconstruction_fallbacks_used = 1;
        plan.fallback.training_fallbacks_used = 1;
        checkpoint.execution_plan = Some(plan);
        checkpoint.sources.push(SourceCheckpoint {
            id: "new-video".into(),
            identity: "frozen-source".into(),
            ordinary_image_preparation_version: None,
            camera_ids: vec![1],
            camera_assignment: None,
            projection: VideoProjection::Perspective,
            names: vec!["frame.png".into()],
            originals: vec![],
            video: None,
            image_sequence: None,
            dimensions: vec![],
            working_dimensions: None,
            frames: Some(FrameState {
                initial_extracted_frames: 100,
                rescue_max_frames: 120,
                ..Default::default()
            }),
            panorama_records: vec![],
            adopted: 0,
            bridge_frames: vec![
                PlannedFrame {
                    source_frame_index: 7,
                    timestamp_seconds: 0.7,
                },
                PlannedFrame {
                    source_frame_index: 17,
                    timestamp_seconds: 1.7,
                },
            ],
        });
        state.inputs = Some(checkpoint);
        Fixture {
            _root: root,
            paths,
            manager,
            runner,
            state,
        }
    }

    fn assert_pixels_and_published_files_unchanged(paths: &ProjectPaths) {
        for directory in [&paths.frames, &paths.colmap_frames, &paths.masks] {
            assert_eq!(
                std::fs::read(directory.join("sentinel")).unwrap(),
                b"keep-or-archive"
            );
        }
        assert_eq!(
            std::fs::read(paths.project.join("final.ply")).unwrap(),
            b"published model"
        );
        assert_eq!(
            std::fs::read(paths.project.join("editing.json")).unwrap(),
            b"saved edits"
        );
    }

    #[tokio::test]
    async fn unproven_legacy_reshoot_base_rejects_before_any_archive_or_state_change() {
        let mut fixture = fixture();
        fixture.state.reshoot = Some(ReshootState {
            source_snapshot_complete: true,
            ..Default::default()
        });
        let before = serde_json::to_value(&fixture.state).unwrap();
        let error = fixture
            .runner
            .begin_database_rebuild(
                &fixture.manager,
                &fixture.paths,
                &mut fixture.state,
                "test-corrupt",
            )
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains(legacy_work_order::UNRESTORABLE_RECIPE));
        assert_eq!(serde_json::to_value(&fixture.state).unwrap(), before);
        assert!(!fixture.paths.work.join("cache-archive").exists());
        assert!(fixture.paths.colmap.join("database.db-wal").exists());
        assert_pixels_and_published_files_unchanged(&fixture.paths);
    }

    #[tokio::test]
    async fn failed_durable_database_barrier_moves_nothing() {
        let mut fixture = fixture();
        std::fs::create_dir(&fixture.paths.state).unwrap();
        let error = fixture
            .runner
            .begin_database_rebuild(
                &fixture.manager,
                &fixture.paths,
                &mut fixture.state,
                "test-corrupt",
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("write_retry_reuse_barrier"));
        assert!(error.io_error().is_some());
        assert!(fixture.state.database_recovery_pending.is_some());
        assert!(!fixture.paths.work.join("cache-archive").exists());
        for name in [
            "database.db",
            "database.db-wal",
            "database.db-journal",
            "database.db-shm",
        ] {
            assert_eq!(
                std::fs::read(fixture.paths.colmap.join(name)).unwrap(),
                name.as_bytes()
            );
        }
        assert_pixels_and_published_files_unchanged(&fixture.paths);
    }

    #[tokio::test]
    async fn rebuild_archives_whole_family_and_preserves_recipe_rescue_and_bridge_selection() {
        let mut fixture = fixture();
        let recipe = fixture.state.brush_training.clone();
        let policy = fixture.state.generation_policy;
        let mut gps = fixture.state.gps.clone();
        gps.as_mut().unwrap().database_recovery_visual_only = true;
        let before = fixture.state.inputs.clone().unwrap();
        fixture
            .runner
            .begin_database_rebuild(
                &fixture.manager,
                &fixture.paths,
                &mut fixture.state,
                "test-corrupt",
            )
            .await
            .unwrap();
        let archives = std::fs::read_dir(fixture.paths.work.join("cache-archive"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(archives.len(), 1);
        for name in [
            "database.db",
            "database.db-wal",
            "database.db-journal",
            "database.db-shm",
        ] {
            assert_eq!(
                std::fs::read(archives[0].join("colmap").join(name)).unwrap(),
                name.as_bytes()
            );
            assert!(!fixture.paths.colmap.join(name).exists());
        }
        assert!(fixture.state.database_recovery_pending.is_none());
        assert!(fixture.state.retry_cache_pending.is_empty());
        assert!(
            !fixture.state.features_complete
                && !fixture.state.matching_complete
                && !fixture.state.reconstruction_complete
        );
        assert_eq!(fixture.state.brush_training, recipe);
        assert_eq!(fixture.state.generation_policy, policy);
        assert_eq!(fixture.state.gps, gps);
        assert_eq!(
            fixture.state.bridge_backfill.status,
            BridgeBackfillStatus::Running
        );
        let after = fixture.state.inputs.as_ref().unwrap();
        assert!(after.prepared && after.bridge_used && after.expanded_matching);
        assert_eq!(after.execution_plan, before.execution_plan);
        assert_eq!(
            after.sources[0].bridge_frames,
            before.sources[0].bridge_frames
        );
        assert_eq!(
            after.sources[0].frames.as_ref().unwrap().rescue_max_frames,
            120
        );
        assert_eq!(
            after.sources[0]
                .frames
                .as_ref()
                .unwrap()
                .initial_extracted_frames,
            100
        );
        assert_pixels_and_published_files_unchanged(&fixture.paths);
        let restored = fixture
            .manager
            .read_state(&fixture.paths.state)
            .await
            .unwrap();
        assert!(
            restored.database_recovery_pending.is_none() && restored.retry_cache_pending.is_empty()
        );
        assert_eq!(restored.brush_training, recipe);
    }

    #[tokio::test]
    async fn pending_archive_continues_without_reclaiming_moved_database_or_retry_budget() {
        let mut fixture = fixture();
        crate::pipeline::artifacts::invalidate_database_for_rebuild(&mut fixture.state);
        fixture
            .manager
            .write_state(&fixture.paths.state, &fixture.state)
            .await
            .unwrap();
        let first_archive = fixture.paths.work.join("previous-archive");
        std::fs::create_dir(&first_archive).unwrap();
        std::fs::rename(&fixture.paths.colmap, first_archive.join("colmap")).unwrap();
        let saved_recipe = fixture.state.brush_training.clone();
        let mut resumed = fixture
            .manager
            .read_state(&fixture.paths.state)
            .await
            .unwrap();
        fixture
            .runner
            .recover_work_order_database(&fixture.manager, &fixture.paths, &mut resumed)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(first_archive.join("colmap/database.db-wal")).unwrap(),
            b"database.db-wal"
        );
        assert!(!fixture.paths.colmap.join("database.db").exists());
        assert_eq!(resumed.brush_training, saved_recipe);
        assert!(
            resumed.database_recovery_pending.is_none() && resumed.retry_cache_pending.is_empty()
        );
        let stable = serde_json::to_value(&resumed).unwrap();
        fixture
            .runner
            .recover_work_order_database(&fixture.manager, &fixture.paths, &mut resumed)
            .await
            .unwrap();
        assert_eq!(serde_json::to_value(&resumed).unwrap(), stable);
        assert_pixels_and_published_files_unchanged(&fixture.paths);
    }

    #[tokio::test]
    async fn exhausted_archive_barrier_still_requires_trusted_reshoot_base() {
        let mut fixture = fixture();
        fixture.state.reshoot = Some(ReshootState {
            source_snapshot_complete: true,
            ..Default::default()
        });
        fixture.state.database_recovery_pending = Some("interrupted-base-restoration".into());
        assert!(fixture.state.retry_cache_pending.is_empty());
        assert!(fixture
            .runner
            .finish_database_rebuild(&fixture.manager, &fixture.paths, &mut fixture.state)
            .await
            .is_err());
        assert!(fixture.state.database_recovery_pending.is_some());
        assert!(fixture.paths.colmap.join("database.db").exists());
    }

    #[tokio::test]
    async fn unified_reshoot_plan_without_snapshot_record_is_not_treated_as_normal() {
        let mut fixture = fixture();
        fixture
            .state
            .inputs
            .as_mut()
            .unwrap()
            .execution_plan
            .as_mut()
            .unwrap()
            .matching = MatchPlan::ReshootCrossSource;
        assert!(fixture.state.reshoot.is_none());
        assert!(fixture
            .runner
            .begin_database_rebuild(
                &fixture.manager,
                &fixture.paths,
                &mut fixture.state,
                "test-corrupt"
            )
            .await
            .is_err());
        assert!(!fixture.paths.work.join("cache-archive").exists());
        assert!(fixture.paths.colmap.join("database.db").exists());
    }

    #[tokio::test]
    async fn trusted_reshoot_base_is_restored_by_copy_without_mutating_frozen_source() {
        let mut fixture = fixture();
        let source = fixture._root.path().join("source");
        let model = source.join("sparse");
        std::fs::create_dir_all(&model).unwrap();
        std::fs::write(model.join("cameras.txt"), "1 PINHOLE 16 16 10 10 8 8\n").unwrap();
        std::fs::write(
            model.join("images.txt"),
            "1 1 0 0 0 0 0 0 1 frame.png\n1 1 1\n",
        )
        .unwrap();
        std::fs::write(model.join("points3D.txt"), "1 0 0 1 128 128 128 0.1 1 0\n").unwrap();
        let source_db = source.join("database.db");
        let db = rusqlite::Connection::open(&source_db).unwrap();
        db.execute_batch("CREATE TABLE cameras(camera_id INTEGER PRIMARY KEY,model INTEGER,width INTEGER,height INTEGER,params BLOB,prior_focal_length INTEGER); CREATE TABLE images(image_id INTEGER PRIMARY KEY,name TEXT,camera_id INTEGER); CREATE TABLE keypoints(image_id INTEGER PRIMARY KEY,rows INTEGER,cols INTEGER,data BLOB); CREATE TABLE descriptors(image_id INTEGER PRIMARY KEY,rows INTEGER,cols INTEGER,data BLOB); CREATE TABLE matches(pair_id INTEGER PRIMARY KEY,rows INTEGER,cols INTEGER,data BLOB); CREATE TABLE two_view_geometries(pair_id INTEGER PRIMARY KEY,rows INTEGER,cols INTEGER,data BLOB); INSERT INTO images VALUES(1,'frame.png',1); INSERT INTO keypoints VALUES(1,1,2,X'0000803F0000803F'); INSERT INTO descriptors VALUES(1,1,128,zeroblob(128));").unwrap();
        let params = [10_f64, 10., 8., 8.]
            .into_iter()
            .flat_map(f64::to_le_bytes)
            .collect::<Vec<_>>();
        db.execute("INSERT INTO cameras VALUES(1,1,16,16,?1,1)", [params])
            .unwrap();
        drop(db);
        let image = source.join("frame.png");
        image::RgbImage::new(16, 16).save(&image).unwrap();
        let snapshot_root = fixture.paths.work.join("reshoot-snapshot-v3");
        let manifest =
            crate::pipeline::reshoot_snapshot::ReshootSnapshotBuilder::new(&snapshot_root)
                .model(&model)
                .database(&source_db)
                .image(&image, "frame.png")
                .build(&fixture.runner.process_manager.child_token())
                .await
                .unwrap();
        let frozen_before = std::fs::read(snapshot_root.join("colmap/database.db")).unwrap();
        let source_before = std::fs::read(&source_db).unwrap();
        fixture.state.reshoot = Some(ReshootState {
            source_snapshot_complete: true,
            snapshot_v3_ready: true,
            snapshot_v3_identity: Some(manifest.content_identity.clone()),
            ..Default::default()
        });
        fixture
            .state
            .inputs
            .as_mut()
            .unwrap()
            .execution_plan
            .as_mut()
            .unwrap()
            .matching = MatchPlan::ReshootCrossSource;
        fixture
            .runner
            .begin_database_rebuild(
                &fixture.manager,
                &fixture.paths,
                &mut fixture.state,
                "test-corrupt",
            )
            .await
            .unwrap();
        let copied = fixture.paths.colmap.join("base-database.db");
        assert_eq!(std::fs::read(&copied).unwrap(), frozen_before);
        assert_eq!(
            crate::reconstruction::direct_input::read_model(
                &fixture.paths.colmap.join("base-model")
            )
            .unwrap()
            .images
            .len(),
            1
        );
        // An interrupted restore can reuse identical private destinations.
        restore_private_collection_base(
            &fixture.paths,
            Some(&manifest.content_identity),
            &fixture.runner.process_manager.child_token(),
        )
        .unwrap();
        let db = rusqlite::Connection::open(&copied).unwrap();
        db.execute_batch("PRAGMA user_version=9").unwrap();
        drop(db);
        assert_eq!(
            std::fs::read(snapshot_root.join("colmap/database.db")).unwrap(),
            frozen_before
        );
        assert_eq!(std::fs::read(&source_db).unwrap(), source_before);
        assert_eq!(
            crate::pipeline::reshoot_snapshot::verify_snapshot(&snapshot_root)
                .unwrap()
                .content_identity,
            manifest.content_identity
        );
        assert_pixels_and_published_files_unchanged(&fixture.paths);
        let changed_before = std::fs::read(&copied).unwrap();
        assert!(restore_private_collection_base(
            &fixture.paths,
            Some(&manifest.content_identity),
            &fixture.runner.process_manager.child_token(),
        )
        .is_err());
        assert_eq!(std::fs::read(&copied).unwrap(), changed_before);
        // Even byte-identical hardlinks to the immutable source are refused;
        // nobody may later rebind final camera parameters through that link.
        std::fs::remove_file(&copied).unwrap();
        std::fs::hard_link(snapshot_root.join("colmap/database.db"), &copied).unwrap();
        assert!(restore_private_collection_base(
            &fixture.paths,
            Some(&manifest.content_identity),
            &fixture.runner.process_manager.child_token(),
        )
        .is_err());
        assert_eq!(
            std::fs::read(snapshot_root.join("colmap/database.db")).unwrap(),
            frozen_before
        );
    }

    #[tokio::test]
    async fn database_rebuild_keeps_verified_gps_inputs_but_not_gps_output() {
        let mut fixture = fixture();
        let manifest = crate::reconstruction::gps::Manifest {
            version: crate::reconstruction::gps::POLICY_VERSION,
            records: vec![],
            origin: None,
        };
        let path = fixture.paths.work.join("gps/manifest.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let bytes = serde_json::to_vec(&manifest).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let old_output = fixture.paths.colmap.join("gps-v1/model");
        std::fs::create_dir_all(&old_output).unwrap();
        std::fs::write(old_output.join("sentinel"), b"old result").unwrap();
        fixture.state.gps = Some(crate::reconstruction::gps::Checkpoint {
            status: "fallback".into(),
            fingerprint: crate::reconstruction::gps::fingerprint(&manifest.records).unwrap(),
            manifest_identity: Some(
                crate::reconstruction::gps::manifest_identity(&manifest).unwrap(),
            ),
            ..crate::reconstruction::gps::Checkpoint::pending()
        });
        fixture
            .runner
            .begin_database_rebuild(
                &fixture.manager,
                &fixture.paths,
                &mut fixture.state,
                "changed-db",
            )
            .await
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert!(!old_output.exists());
        assert!(
            fixture
                .state
                .gps
                .as_ref()
                .unwrap()
                .database_recovery_visual_only
        );
        let restored = fixture
            .manager
            .read_state(&fixture.paths.state)
            .await
            .unwrap();
        assert!(restored.gps.unwrap().database_recovery_visual_only);
    }

    #[tokio::test]
    async fn mismatched_gps_manifest_stops_before_database_archival() {
        let mut fixture = fixture();
        let path = fixture.paths.work.join("gps/manifest.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, br#"{"version":1,"records":[],"origin":null}"#).unwrap();
        let before = serde_json::to_value(&fixture.state).unwrap();
        assert!(fixture
            .runner
            .begin_database_rebuild(
                &fixture.manager,
                &fixture.paths,
                &mut fixture.state,
                "changed-db"
            )
            .await
            .is_err());
        assert!(!fixture.paths.work.join("cache-archive").exists());
        assert!(fixture.paths.colmap.join("database.db-wal").exists());
        assert_eq!(serde_json::to_value(&fixture.state).unwrap(), before);
    }
}
