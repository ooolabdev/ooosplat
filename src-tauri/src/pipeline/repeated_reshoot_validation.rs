//! Regression coverage for completed Reshoot V2 projects used as reshoot sources.
//! Synthetic fixtures run normally. The opt-in real-project test is read-only;
//! it never creates a task or derived project or starts COLMAP or Brush.

use super::*;
use std::{collections::BTreeMap, fs, time::SystemTime};

fn image_file_metadata(root: &Path) -> BTreeMap<PathBuf, (u64, SystemTime)> {
    let mut files = BTreeMap::new();
    for role in ["frames", "colmap_frames", "masks"] {
        let directory = root.join("work").join(role);
        for entry in fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            assert!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "The validation fixture must contain regular local image files"
            );
            files.insert(
                path.strip_prefix(root).unwrap().to_path_buf(),
                (metadata.len(), metadata.modified().unwrap()),
            );
        }
    }
    files
}

fn critical_hashes(root: &Path, files: &[PathBuf]) -> BTreeMap<PathBuf, String> {
    files
        .iter()
        .map(|path| {
            (
                path.strip_prefix(root).unwrap().to_path_buf(),
                crate::video::panorama::fingerprint(path).unwrap(),
            )
        })
        .collect()
}

fn repeated_snapshot_fixture(has_alpha: bool) -> (tempfile::TempDir, PipelineStateFile) {
    let root = tempfile::tempdir().unwrap();
    for directory in ["frames", "colmap_frames", "masks"] {
        fs::create_dir_all(root.path().join("work").join(directory)).unwrap();
    }
    let names = [
        "frame_000001.png",
        "reshoot_previous_batch_000001.png",
        "reshoot_current_batch_000001.png",
    ];
    for (name, (width, height)) in names.iter().zip([(4, 3), (6, 4), (3, 7)]) {
        let frame = root.path().join("work/frames").join(name);
        if has_alpha {
            image::RgbaImage::from_pixel(width, height, image::Rgba([11, 22, 33, 0]))
                .save(&frame)
                .unwrap();
            image::GrayImage::from_pixel(width, height, image::Luma([0]))
                .save(root.path().join("work/masks").join(format!("{name}.png")))
                .unwrap();
        } else {
            image::RgbImage::from_pixel(width, height, image::Rgb([11, 22, 33]))
                .save(&frame)
                .unwrap();
        }
        fs::copy(&frame, root.path().join("work/colmap_frames").join(name)).unwrap();
    }
    let recipe = serde_json::from_value(serde_json::json!({
        "version": 2,
        "options": {
            "condition": "good",
            "allowFallback": false,
            "sourceIdentity": "saved-source",
            "inputIdentity": "saved-input",
            "projection": "perspective",
            "imageMatchingStrategy": "exhaustive"
        },
        "initial": { "reconstruction": "incremental", "training": "warm" },
        "actual": { "reconstruction": "incremental", "training": "warm" },
        "batchId": "current_batch",
        "sourceNames": [names[0], names[1]],
        "newNames": [names[2]]
    }))
    .unwrap();
    let mut state = PipelineStateFile::created(Quality::Fast);
    state.frames = Some(FrameState {
        estimated_frames: 3,
        extracted_frames: Some(3),
        has_alpha,
        mask_count: Some(if has_alpha { 3 } else { 0 }),
        colmap_input_version: Some(COLMAP_INPUT_VERSION),
        ..Default::default()
    });
    state.reshoot = Some(ReshootState {
        recipe: Some(recipe),
        source_snapshot_complete: true,
        supplemental_frames_complete: true,
        source_image_count: 2,
        reshoot_frame_count: 1,
        has_alpha,
        mask_count: u64::from(has_alpha),
        colmap_input_version: Some(COLMAP_INPUT_VERSION),
        ..Default::default()
    });
    (root, state)
}

#[tokio::test]
async fn saved_repeated_reshoot_snapshot_accepts_all_batches_and_mixed_dimensions() {
    let (root, state) = repeated_snapshot_fixture(false);
    let manager = ProjectManager::with_root(root.path().into());
    let state_path = root.path().join("state.json");
    manager.write_state(&state_path, &state).await.unwrap();
    let saved = manager.read_state(&state_path).await.unwrap();
    assert_eq!(
        validate_current_colmap_input_snapshot(root.path(), &saved)
            .await
            .unwrap(),
        PreparedImageSequence {
            image_count: 3,
            mask_count: 0,
            has_alpha: false,
        }
    );
}

#[tokio::test]
async fn repeated_reshoot_snapshot_rejects_incomplete_flags_versions_and_counts() {
    let (root, state) = repeated_snapshot_fixture(false);
    for mutation in [
        "source_incomplete",
        "new_incomplete",
        "missing_reshoot_version",
        "old_reshoot_version",
        "old_frame_version",
        "source_count",
        "new_count",
        "total_count",
    ] {
        let mut changed = state.clone();
        match mutation {
            "source_incomplete" => {
                changed.reshoot.as_mut().unwrap().source_snapshot_complete = false
            }
            "new_incomplete" => {
                changed
                    .reshoot
                    .as_mut()
                    .unwrap()
                    .supplemental_frames_complete = false
            }
            "missing_reshoot_version" => {
                changed.reshoot.as_mut().unwrap().colmap_input_version = None
            }
            "old_reshoot_version" => {
                changed.reshoot.as_mut().unwrap().colmap_input_version = Some(1)
            }
            "old_frame_version" => changed.frames.as_mut().unwrap().colmap_input_version = Some(1),
            "source_count" => changed.reshoot.as_mut().unwrap().source_image_count += 1,
            "new_count" => changed.reshoot.as_mut().unwrap().reshoot_frame_count += 1,
            "total_count" => changed.frames.as_mut().unwrap().extracted_frames = Some(4),
            _ => unreachable!(),
        }
        assert!(
            validate_current_colmap_input_snapshot(root.path(), &changed)
                .await
                .is_err(),
            "accepted inconsistent checkpoint: {mutation}",
        );
    }
}

#[tokio::test]
async fn repeated_reshoot_snapshot_rejects_duplicate_omitted_and_unlisted_batches() {
    for mutation in ["duplicate", "omit_history", "extra", "missing_history"] {
        let (root, mut state) = repeated_snapshot_fixture(false);
        let recipe = state.reshoot.as_mut().unwrap().recipe.as_mut().unwrap();
        match mutation {
            "duplicate" => recipe.new_names[0] = recipe.source_names[1].clone(),
            "omit_history" => recipe.source_names[1] = "reshoot_other_000001.png".into(),
            "extra" => {
                for role in ["frames", "colmap_frames"] {
                    let directory = root.path().join("work").join(role);
                    fs::copy(
                        directory.join(&recipe.source_names[0]),
                        directory.join("unlisted.png"),
                    )
                    .unwrap();
                }
            }
            "missing_history" => {
                for role in ["frames", "colmap_frames"] {
                    fs::remove_file(
                        root.path()
                            .join("work")
                            .join(role)
                            .join(&recipe.source_names[1]),
                    )
                    .unwrap();
                }
            }
            _ => unreachable!(),
        }
        assert!(
            validate_current_colmap_input_snapshot(root.path(), &state)
                .await
                .is_err(),
            "accepted an incorrect frozen image set: {mutation}",
        );
    }
}

#[tokio::test]
async fn repeated_reshoot_snapshot_requires_alpha_masks_for_historical_batches() {
    let (root, state) = repeated_snapshot_fixture(true);
    assert_eq!(
        validate_current_colmap_input_snapshot(root.path(), &state)
            .await
            .unwrap()
            .mask_count,
        3,
    );
    fs::remove_file(
        root.path()
            .join("work/masks/reshoot_previous_batch_000001.png.png"),
    )
    .unwrap();
    let error = validate_current_colmap_input_snapshot(root.path(), &state)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("COLMAP Mask"));
}

#[tokio::test]
#[ignore = "Set OOOSPLAT_REPEATED_RESHOOT_PROJECT to a registered completed Reshoot V2 project; source is read-only"]
async fn actual_completed_reshoot_v2_can_be_used_as_a_reshoot_source() {
    let started = Instant::now();
    let root = PathBuf::from(
        std::env::var_os("OOOSPLAT_REPEATED_RESHOOT_PROJECT")
            .expect("OOOSPLAT_REPEATED_RESHOOT_PROJECT is required"),
    )
    .canonicalize()
    .unwrap();
    let metadata: ProjectMetadata =
        serde_json::from_slice(&fs::read(root.join("project.json")).unwrap()).unwrap();
    let state: PipelineStateFile =
        serde_json::from_slice(&fs::read(root.join("state.json")).unwrap()).unwrap();
    assert_eq!(metadata.status, ProjectStatus::Completed);
    assert!(metadata.imported.is_none());
    assert!(
        state.inputs.is_none(),
        "This fixture exercises the saved V2 path"
    );
    let reshoot = state
        .reshoot
        .as_ref()
        .expect("Reshoot V2 state is required");
    let recipe = reshoot
        .recipe
        .as_ref()
        .expect("Saved V2 recipe is required");
    assert_eq!(recipe.version, 2);
    let expected = state.frames.as_ref().unwrap().extracted_frames.unwrap();
    assert_eq!(
        expected,
        reshoot.source_image_count + reshoot.reshoot_frame_count
    );
    assert_eq!(
        recipe.source_names.len() as u64 + recipe.new_names.len() as u64,
        expected,
    );

    let frame_metadata = image_file_metadata(&root);
    let frames = root.join("work/frames");
    let database = root.join("work/colmap/database.db");
    let reconstruction = reusable_reconstruction(&root, &frames, metadata.output.as_ref())
        .await
        .unwrap();
    let final_ply = catalog::project_model_path(&root, &metadata).unwrap();
    let mut protected_files = vec![
        root.join("project.json"),
        root.join("state.json"),
        final_ply,
        database.clone(),
    ];
    protected_files.extend(reusable_model_files(&reconstruction.model).unwrap());
    let protected_hashes = critical_hashes(&root, &protected_files);

    // Exercise the continuation compatibility normalizer against the actual
    // historical layout, but mutate only an in-memory clone. In particular,
    // the old producer did not write a separate base-database.db.
    let mut normalized = state.clone();
    let saved_recipe = serde_json::to_value(&reshoot.recipe).unwrap();
    let saved_training = serde_json::to_value(&state.brush_training).unwrap();
    let paths = ProjectPaths::existing(metadata.id, root.clone());
    crate::pipeline::artifacts::normalize_reshoot_v2(&paths, &mut normalized).unwrap();
    let resumed = normalized.reshoot.as_ref().unwrap();
    assert!(resumed.source_snapshot_complete);
    assert!(resumed.supplemental_frames_complete);
    assert!(resumed.supplemental_features_complete);
    assert!(resumed.incremental_matching_complete);
    assert!(resumed.incremental_reconstruction_complete);
    assert_eq!(serde_json::to_value(&resumed.recipe).unwrap(), saved_recipe);
    assert_eq!(
        serde_json::to_value(&normalized.brush_training).unwrap(),
        saved_training
    );

    let snapshot = validate_current_colmap_input_snapshot(&root, &state)
        .await
        .unwrap();
    assert_eq!(snapshot.image_count, expected);
    let resolved =
        resolve_reshoot_colmap_input(&root, &metadata, &state, &reconstruction.model, &database)
            .await
            .unwrap();
    assert_eq!(resolved.mode, ReshootColmapInputMode::CurrentV2);
    assert_eq!(resolved.colmap_images, root.join("work/colmap_frames"));

    // Use the existing catalog without registering or rewriting anything.
    let (registered_root, _) = catalog::load_registered_project(metadata.id).await.unwrap();
    assert_eq!(registered_root.canonicalize().unwrap(), root);
    let assets = reshoot_runner::source_assets(metadata.id).await.unwrap();
    assert_eq!(assets.root.canonicalize().unwrap(), root);
    assert_eq!(assets.names.len() as u64, expected);
    assert_eq!(assets.colmap_input_mode, ReshootColmapInputMode::CurrentV2);
    let info = PipelineRunner::new(
        EnginePaths::from_root(root.join("validation-engines-must-not-exist")),
        |_| {},
    )
    .inspect_reshoot_source_v2(metadata.id)
    .await
    .unwrap();
    assert!(info.eligible);
    assert_eq!(info.source_image_count, expected);
    assert!(!info.source_identity.is_empty());

    assert_eq!(critical_hashes(&root, &protected_files), protected_hashes);
    assert_eq!(image_file_metadata(&root), frame_metadata);
    eprintln!(
        "Repeated-reshoot source accepted: {expected} images; critical SHA-256 and all frame/mask names, sizes and mtimes unchanged; no engine/task created; elapsed {:?}",
        started.elapsed(),
    );
}
