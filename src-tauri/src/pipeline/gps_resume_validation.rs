//! Opt-in, isolated recovery check for an existing GPS project.
//! The source is read-only; every mutating pipeline operation uses a copy.

use super::*;
use std::{collections::BTreeMap, fs, fs::FileTimes};

fn files_under(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let metadata = fs::symlink_metadata(root)?;
    if metadata.file_type().is_symlink() {
        return Err(std::io::Error::other("Validation refuses symbolic links"));
    }
    if metadata.is_file() {
        return Ok(vec![root.to_path_buf()]);
    }
    let mut entries = fs::read_dir(root)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    let mut files = Vec::new();
    for entry in entries {
        files.extend(files_under(&entry.path())?);
    }
    Ok(files)
}

fn copy_asset(source: &Path, target: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        return Err(std::io::Error::other("Validation refuses symbolic links"));
    }
    if metadata.is_dir() {
        fs::create_dir_all(target)?;
        let mut entries = fs::read_dir(source)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            copy_asset(&entry.path(), &target.join(entry.file_name()))?;
        }
    } else {
        fs::create_dir_all(target.parent().unwrap())?;
        fs::copy(source, target)?;
        // Prepared manifests intentionally bind mtimes as well as contents.
        // Set only the copy's timestamp, never a source file's metadata.
        fs::OpenOptions::new()
            .write(true)
            .open(target)?
            .set_times(FileTimes::new().set_modified(metadata.modified()?))?;
    }
    Ok(())
}

fn asset_hashes(source: &Path, assets: &[&str]) -> BTreeMap<PathBuf, String> {
    let mut hashes = BTreeMap::new();
    for asset in assets {
        let root = source.join(asset);
        if !root.exists() {
            continue;
        }
        for file in files_under(&root).unwrap() {
            hashes.insert(
                file.strip_prefix(source).unwrap().to_path_buf(),
                crate::video::panorama::fingerprint(&file).unwrap(),
            );
        }
    }
    hashes
}

/// Best-effort, read-only filesystem evidence. No GPS values, original absolute
/// paths, ACL changes, attribute changes, or attempts to unlock files.
fn archive_metadata(root: &Path) -> serde_json::Value {
    let mut paths = [
        root.to_path_buf(),
        root.join("work"),
        root.join("work/brush"),
        root.join("work/brush/dataset"),
        root.join("work/brush/dataset/images"),
        root.join("work/brush/dataset/sparse"),
        root.join("work/cache-archive"),
    ]
    .to_vec();
    if let Ok(files) = files_under(&root.join("work/brush")) {
        paths.extend(files);
    }
    let entries = paths
        .into_iter()
        .map(|path| {
            let role = path
                .strip_prefix(root)
                .unwrap_or(Path::new("unavailable"))
                .to_string_lossy()
                .replace('\\', "/");
            match fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    #[cfg(windows)]
                    let windows_attributes = {
                        use std::os::windows::fs::MetadataExt;
                        Some(metadata.file_attributes())
                    };
                    #[cfg(not(windows))]
                    let windows_attributes: Option<u32> = None;
                    serde_json::json!({
                        "role": role,
                        "exists": true,
                        "directory": metadata.is_dir(),
                        "symlink": metadata.file_type().is_symlink(),
                        "readonly": metadata.permissions().readonly(),
                        "windowsAttributes": windows_attributes,
                        "length": metadata.len(),
                    })
                }
                Err(error) => serde_json::json!({
                    "role": role,
                    "exists": false,
                    "errorKind": format!("{:?}", error.kind()),
                    "osError": error.raw_os_error(),
                }),
            }
        })
        .collect::<Vec<_>>();
    serde_json::json!({ "entries": entries })
}

#[tokio::test]
#[ignore = "Set OOOSPLAT_VALIDATION_RESUME_PROJECT to a GPS project; only an isolated workspace copy is mutated"]
async fn real_gps_project_continues_to_training_without_rerunning_engines() {
    use crate::pipeline::work_order::{next_action, GenerationAction, Readiness};

    let source = PathBuf::from(
        std::env::var_os("OOOSPLAT_VALIDATION_RESUME_PROJECT")
            .expect("OOOSPLAT_VALIDATION_RESUME_PROJECT is required"),
    )
    .canonicalize()
    .unwrap();
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    // The dev server already excludes src-tauri. Keep generated validation
    // trees there even when an older Vite instance is still watching .cache:
    // per-directory Windows watchers can themselves deny cache renames.
    let cache = repo.join("src-tauri/target/validation-cache");
    fs::create_dir_all(&cache).unwrap();
    let isolated = tempfile::Builder::new()
        .prefix("gps-resume-validation-")
        .tempdir_in(&cache)
        .unwrap();
    let target = isolated.path().join("project");
    assert!(!target.starts_with(&source));
    assert!(!source.starts_with(isolated.path()));

    let assets = [
        "project.json",
        "state.json",
        "work/frames",
        "work/colmap_frames",
        "work/masks",
        "work/colmap/database.db",
        "work/colmap/authoritative",
        "work/colmap/gps-v1/model",
        "work/gps",
        "work/brush/dataset",
    ];
    let protected = asset_hashes(&source, &assets);
    assert!(!protected.is_empty());
    for asset in &assets {
        let source_asset = source.join(asset);
        if source_asset.exists() {
            copy_asset(&source_asset, &target.join(asset)).unwrap();
        }
    }

    let mut metadata: ProjectMetadata =
        serde_json::from_slice(&fs::read(target.join("project.json")).unwrap()).unwrap();
    metadata.project_path = target.clone();
    let paths = ProjectPaths::existing(metadata.id, target);
    let manager = ProjectManager::with_root(isolated.path().into());
    let mut state: PipelineStateFile =
        serde_json::from_slice(&fs::read(&paths.state).unwrap()).unwrap();
    assert!(
        metadata.reshoot.is_none(),
        "This fixture validates a normal GPS project"
    );
    assert!(state.features_complete && state.matching_complete && state.reconstruction_complete);
    assert!(!state.brush_complete);
    assert_eq!(state.gps.as_ref().unwrap().status, "complete");
    let saved_recipe = serde_json::to_value(&state.brush_training).unwrap();
    let saved_policy = state.generation_policy;
    let saved_bridge = serde_json::to_value(&state.bridge_backfill).unwrap();
    let saved_reshoot = serde_json::to_value(&state.reshoot).unwrap();
    let saved_gps = serde_json::to_value(&state.gps).unwrap();

    // The executable paths do not exist: any SfM, GPS or Brush invocation fails
    // this test instead of accidentally performing a real reconstruction.
    let runner = PipelineRunner::new(
        EnginePaths::from_root(isolated.path().join("engines-must-not-run")),
        |_| {},
    );
    assert!(
        crate::pipeline::artifacts::normalize_collection(&paths, &mut state, None)
            .await
            .unwrap()
    );
    assert!(state.features_complete && state.matching_complete && state.reconstruction_complete);
    assert!(state.inputs.as_ref().unwrap().prepared);
    assert!(state.inputs.as_ref().unwrap().reference_image.is_none());
    assert_eq!(state.retry_cache_pending, ["brush"]);
    manager.write_state(&paths.state, &state).await.unwrap();
    let continuation = runner
        .prepare_continuation(&manager, &paths, &mut state)
        .await;
    if let Err(error) = continuation {
        // Keep only this test-owned workspace copy. Never archive, remove,
        // change permissions on, or otherwise repair the original project.
        let retained = isolated.keep();
        let source_unchanged = asset_hashes(&source, &assets) == protected;
        let diagnostic = serde_json::json!({
            "operation": "archive_retry_cache",
            "error": error.to_string(),
            "osError": error.io_error().and_then(std::io::Error::raw_os_error),
            "sourceHashesUnchanged": source_unchanged,
            "originalMetadata": archive_metadata(&source),
            "isolatedMetadata": archive_metadata(&paths.project),
        });
        let diagnostic_path = retained.join("archive-validation-diagnostics.json");
        if let Err(log_error) = serde_json::to_vec_pretty(&diagnostic)
            .map_err(std::io::Error::other)
            .and_then(|bytes| fs::write(&diagnostic_path, bytes))
        {
            eprintln!("Could not write isolated validation metadata: {log_error}");
        }
        eprintln!("Retained failing validation copy: {}", retained.display());
        eprintln!("Local metadata diagnostic: {}", diagnostic_path.display());
        eprintln!(
            "Local retry preparation log: {}",
            paths.logs.join("retry-preparation.log").display()
        );
        assert!(
            source_unchanged,
            "Original project hashes changed during read-only validation"
        );
        panic!("Isolated continuation failed without modifying the source: {error}");
    }
    assert!(state.retry_cache_pending.is_empty());

    let mut checkpoint = state.inputs.clone().unwrap();
    let gps_model = runner
        .collection_gps(
            &manager,
            &paths,
            &metadata,
            &mut state,
            &mut checkpoint,
            &paths.colmap.join("authoritative"),
        )
        .await
        .unwrap();
    assert_eq!(gps_model, paths.colmap.join("gps-v1/model"));
    assert!(
        !paths.logs.join("gps.log").exists(),
        "GPS must reuse its validated cache"
    );
    assert_eq!(serde_json::to_value(&state.gps).unwrap(), saved_gps);

    let names = checkpoint
        .sources
        .iter()
        .flat_map(|s| s.names.clone())
        .collect::<Vec<_>>();
    let records = checkpoint
        .sources
        .iter()
        .flat_map(|s| s.panorama_records.clone())
        .collect::<Vec<_>>();
    let dataset = runner
        .input_dataset_with_native_runtime(
            &paths,
            &gps_model,
            metadata.quality,
            &records,
            &names,
            true,
        )
        .await
        .unwrap();
    let identity = read_training_dataset_identity(&dataset.path).await.unwrap();
    assert!(crate::pipeline::artifacts::validated_training_dataset_rebuild(&state));
    bind_collection_training_recipe(&mut state, identity, false).unwrap();
    checkpoint.reference_image = Some(dataset.reference);
    state.inputs = Some(checkpoint);
    assert_eq!(
        next_action(
            &state,
            Readiness {
                reconstruction: true,
                training_dataset: true,
                candidate: false
            },
        ),
        GenerationAction::Train,
    );
    let mut rebound_recipe = serde_json::to_value(&state.brush_training).unwrap();
    rebound_recipe["datasetIdentity"] = saved_recipe["datasetIdentity"].clone();
    assert_eq!(
        rebound_recipe, saved_recipe,
        "Rebinding must not change training parameters or OOM state"
    );
    assert_eq!(state.generation_policy, saved_policy);
    assert_eq!(
        serde_json::to_value(&state.bridge_backfill).unwrap(),
        saved_bridge
    );
    assert_eq!(serde_json::to_value(&state.reshoot).unwrap(), saved_reshoot);
    assert_eq!(
        asset_hashes(&source, &assets),
        protected,
        "Source project and images must remain unchanged"
    );
    eprintln!("Read-only source verified; isolated GPS recovery reached Train without launching an engine");
}
