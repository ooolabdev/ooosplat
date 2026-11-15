use super::*;
use crate::{
    pipeline::reshoot::{ReshootRecipe, TrainingMode},
    presets::Quality,
    project::metadata::ReshootState,
    reconstruction::{
        gps::{Checkpoint, Similarity},
        splat_transform::{export_transformed_ply_to, validate_training_ply},
    },
};
use std::fs;

fn project() -> (tempfile::TempDir, ProjectPaths, PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let paths = ProjectPaths::existing(
        uuid::Uuid::new_v4(),
        temporary.path().join("derived-project"),
    );
    let dataset = paths.brush.join("dataset");
    fs::create_dir_all(dataset.join("sparse/0")).unwrap();
    fs::create_dir(dataset.join("images")).unwrap();
    // Initialization preparation must not touch ordinary dataset assets.
    fs::write(
        dataset.join("sparse/0/points3D.bin"),
        b"retained sparse points",
    )
    .unwrap();
    fs::write(dataset.join("images/frame.png"), b"retained image bytes").unwrap();
    (temporary, paths, dataset)
}

fn gaussian(path: &Path, x: f32) -> GaussianIdentity {
    let properties = [
        "x", "y", "z", "f_dc_0", "f_dc_1", "f_dc_2", "opacity", "scale_0", "scale_1", "scale_2",
        "rot_0", "rot_1", "rot_2", "rot_3",
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
    validated_identity(path)
}

fn validated_identity(path: &Path) -> GaussianIdentity {
    let (info, sh, sha256) = validate_training_ply(path, &CancellationToken::new()).unwrap();
    identity(&info, sh, sha256)
}

fn saved_state(source: &GaussianIdentity) -> PipelineStateFile {
    let mut state = PipelineStateFile::created(Quality::Fast);
    state.brush_training.dataset_identity = Some("validated-training-dataset".into());
    let recipe: ReshootRecipe = serde_json::from_value(serde_json::json!({
        "version": 2,
        "options": {
            "condition": "good",
            "allowFallback": true,
            "sourceIdentity": "immutable-source-identity",
            "inputIdentity": "new-input-identity",
            "projection": "perspective"
        },
        "initial": { "reconstruction": "incremental", "training": "warm" },
        "actual": { "reconstruction": "incremental", "training": "warm" },
        "batchId": "saved-batch",
        "initSha256": source.sha256,
        "initGeoreference": "verified-source-coordinate-frame",
        "sourceNames": ["old.png"],
        "newNames": ["new.png"]
    }))
    .unwrap();
    state.reshoot = Some(ReshootState {
        recipe: Some(recipe),
        source_snapshot_complete: true,
        ..Default::default()
    });
    state
}

fn gps_transform(state: &mut PipelineStateFile) -> Similarity {
    let transform = Similarity {
        scale: 2.,
        translation: [4., 5., 6.],
        ..Similarity::enu_to_ply()
    };
    state.gps = Some(Checkpoint {
        status: "complete".into(),
        transform: Some(transform),
        reference_valid: true,
        ..Checkpoint::pending()
    });
    transform
}

fn cold_after_fallback(state: &mut PipelineStateFile) {
    let recipe = state.reshoot.as_mut().unwrap().recipe.as_mut().unwrap();
    recipe.actual.training = TrainingMode::Cold;
    recipe.training_fallbacks = 1;
}

fn archive_files(paths: &ProjectPaths) -> Vec<PathBuf> {
    let directory = paths.work.join("initialization-archive");
    if !directory.exists() {
        return vec![];
    }
    let mut files = fs::read_dir(directory)
        .unwrap()
        .flat_map(|entry| {
            fs::read_dir(entry.unwrap().path())
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    files.sort();
    files
}

#[tokio::test]
async fn transformed_warm_install_survives_state_round_trip_and_cold_archives_by_receipt() {
    let (_temporary, paths, dataset) = project();
    let source_path = paths.work.join("init-source.ply");
    let source = gaussian(&source_path, 1.);
    let source_bytes = fs::read(&source_path).unwrap();
    let mut state = saved_state(&source);
    let transform = gps_transform(&mut state);
    let staged = staging_path(&paths).unwrap();
    let staged_directory = staged.path().parent().unwrap().to_owned();
    export_transformed_ply_to(
        &source_path,
        staged.path(),
        transform.engine_transform(),
        Default::default(),
        |_, _| {},
        || Ok(()),
    )
    .unwrap();
    let transformed = validated_identity(staged.path());
    assert_ne!(transformed.sha256, source.sha256);
    assert_eq!(transformed.splat_count, source.splat_count);
    let cancel = CancellationToken::new();
    let warm = install_staged(
        &paths,
        &state,
        &dataset,
        staged.path(),
        transformed.clone(),
        "verified-source-coordinate-frame".into(),
        &cancel,
    )
    .await
    .unwrap();
    drop(staged);
    assert!(!staged_directory.exists());
    verify_start(&paths, &dataset, &warm, &cancel)
        .await
        .unwrap();
    let receipt: Receipt =
        serde_json::from_slice(&fs::read(dataset.join(RECEIPT)).unwrap()).unwrap();
    assert_eq!(receipt.binding, Binding::from_state(&state).unwrap());
    assert!(receipt.owned.contains(&transformed));

    // The saved plan changes, but the same prepared dataset and coordinate
    // identity must remain stable across an application restart.
    cold_after_fallback(&mut state);
    state.brush_training.oom_retry_started = true;
    state.brush_training.oom_retry_used = true;
    crate::project::manager::atomic_write_json(&paths.state, &state)
        .await
        .unwrap();
    let restored: PipelineStateFile =
        serde_json::from_slice(&fs::read(&paths.state).unwrap()).unwrap();
    let saved_before = serde_json::to_value(&restored).unwrap();
    assert!(restored
        .brush_training
        .legacy_initialization_sha256
        .is_none());
    let cold = InitializationContract::SparsePoints;
    prepare_saved(&paths, &restored, &dataset, &cold, &cancel)
        .await
        .unwrap();
    verify_start(&paths, &dataset, &cold, &cancel)
        .await
        .unwrap();
    assert!(!dataset.join("init.ply").exists());
    let archived = archive_files(&paths);
    assert_eq!(archived.len(), 1);
    assert!(!archived[0].starts_with(&dataset));
    assert_eq!(validated_identity(&archived[0]), transformed);
    // Repeated continuation does not re-grant fallback or archive twice.
    prepare_saved(&paths, &restored, &dataset, &cold, &cancel)
        .await
        .unwrap();
    assert_eq!(archive_files(&paths), archived);
    assert_eq!(serde_json::to_value(&restored).unwrap(), saved_before);
    assert_eq!(fs::read(&source_path).unwrap(), source_bytes);
    assert_eq!(
        fs::read(dataset.join("sparse/0/points3D.bin")).unwrap(),
        b"retained sparse points"
    );
    assert_eq!(
        fs::read(dataset.join("images/frame.png")).unwrap(),
        b"retained image bytes"
    );
}

#[tokio::test]
async fn resumed_warm_keeps_identical_initializer_without_accumulating_archives() {
    let (_temporary, paths, dataset) = project();
    let source = gaussian(&paths.work.join("init-source.ply"), 1.);
    let state = saved_state(&source);
    let cancel = CancellationToken::new();
    for _ in 0..2 {
        let staged = staging_path(&paths).unwrap();
        let directory = staged.path().parent().unwrap().to_owned();
        let staged_identity = gaussian(staged.path(), 1.);
        let contract = install_staged(
            &paths,
            &state,
            &dataset,
            staged.path(),
            staged_identity,
            "verified-source-coordinate-frame".into(),
            &cancel,
        )
        .await
        .unwrap();
        drop(staged);
        assert!(!directory.exists());
        verify_start(&paths, &dataset, &contract, &cancel)
            .await
            .unwrap();
        assert_eq!(validated_identity(&dataset.join("init.ply")), source);
        assert!(archive_files(&paths).is_empty());
    }
}

#[tokio::test]
async fn unknown_initializer_blocks_cold_and_staged_warm_without_changing_original_files() {
    let (_temporary, paths, dataset) = project();
    let source = gaussian(&paths.work.join("init-source.ply"), 1.);
    let mut state = saved_state(&source);
    cold_after_fallback(&mut state);
    let unknown_path = dataset.join("init.ply");
    gaussian(&unknown_path, 99.);
    let unknown_bytes = fs::read(&unknown_path).unwrap();
    let state_before = serde_json::to_value(&state).unwrap();
    let cancel = CancellationToken::new();
    assert!(prepare_saved(
        &paths,
        &state,
        &dataset,
        &InitializationContract::SparsePoints,
        &cancel,
    )
    .await
    .is_err());
    assert!(!dataset.join(RECEIPT).exists());
    let staged = staging_path(&paths).unwrap();
    let staged_directory = staged.path().parent().unwrap().to_owned();
    let staged_identity = gaussian(staged.path(), 1.);
    assert!(install_staged(
        &paths,
        &state,
        &dataset,
        staged.path(),
        staged_identity,
        "verified-source-coordinate-frame".into(),
        &cancel,
    )
    .await
    .is_err());
    assert_eq!(fs::read(&unknown_path).unwrap(), unknown_bytes);
    assert!(archive_files(&paths).is_empty());
    assert_eq!(serde_json::to_value(&state).unwrap(), state_before);
    drop(staged);
    assert!(!staged_directory.exists());
}

#[tokio::test]
async fn corrupted_or_rebound_receipt_cannot_authorize_cold_archival() {
    for case in [
        "json",
        "version",
        "source",
        "coordinates",
        "dataset",
        "transform",
    ] {
        let (_temporary, paths, dataset) = project();
        let source = gaussian(&paths.work.join("init-source.ply"), 1.);
        let mut state = saved_state(&source);
        gps_transform(&mut state);
        let init = dataset.join("init.ply");
        let transformed = gaussian(&init, 2.);
        persist_receipt(&paths, &state, &dataset, vec![transformed])
            .await
            .unwrap();
        let original_bytes = fs::read(&init).unwrap();
        let path = dataset.join(RECEIPT);
        if case == "json" {
            fs::write(&path, b"{broken").unwrap();
        } else {
            let mut receipt: Receipt = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            match case {
                "version" => receipt.version += 1,
                "source" => receipt.binding.source_sha256 = Some("a".repeat(64)),
                "coordinates" => receipt.binding.coordinate_frame_id = Some("another-frame".into()),
                "dataset" => receipt.binding.dataset_identity = Some("another-dataset".into()),
                "transform" => receipt.binding.transform = None,
                _ => unreachable!(),
            }
            fs::write(&path, serde_json::to_vec(&receipt).unwrap()).unwrap();
        }
        let receipt_before = fs::read(&path).unwrap();
        cold_after_fallback(&mut state);
        assert!(
            prepare_saved(
                &paths,
                &state,
                &dataset,
                &InitializationContract::SparsePoints,
                &CancellationToken::new(),
            )
            .await
            .is_err(),
            "case {case}"
        );
        assert_eq!(fs::read(&init).unwrap(), original_bytes, "case {case}");
        assert_eq!(fs::read(path).unwrap(), receipt_before, "case {case}");
        assert!(archive_files(&paths).is_empty(), "case {case}");
    }
}

#[test]
fn staged_guard_removes_only_its_owned_file_and_empty_directory() {
    let (_temporary, paths, _dataset) = project();
    for populated in [false, true] {
        let staged = staging_path(&paths).unwrap();
        let path = staged.path().to_owned();
        let directory = path.parent().unwrap().to_owned();
        if populated {
            gaussian(&path, 1.);
        }
        drop(staged);
        assert!(!path.exists());
        assert!(!directory.exists());
    }
    let staged = staging_path(&paths).unwrap();
    let directory = staged.path().parent().unwrap().to_owned();
    let unrelated = directory.join("unrelated-evidence.log");
    fs::write(&unrelated, b"do not delete").unwrap();
    drop(staged);
    assert_eq!(fs::read(unrelated).unwrap(), b"do not delete");
    assert!(directory.is_dir());
}

#[tokio::test]
async fn legacy_cold_without_receipt_uses_saved_hash_evidence_without_resetting_recipe_or_retries()
{
    for use_legacy_identity in [false, true] {
        let (_temporary, paths, dataset) = project();
        let source = gaussian(&paths.work.join("init-source.ply"), 1.);
        let mut state = saved_state(&source);
        cold_after_fallback(&mut state);
        state.brush_training.oom_retry_started = true;
        state.brush_training.oom_retry_used = true;
        state.brush_training.oom_retry_source_count = Some(1);
        state.brush_training.legacy_dataset_identity = state.brush_training.dataset_identity.take();
        let init = dataset.join("init.ply");
        let expected = gaussian(&init, if use_legacy_identity { 2. } else { 1. });
        if use_legacy_identity {
            // No receipt was historically recorded for this transformed copy.
            // A previously validated legacy content identity is sufficient.
            gps_transform(&mut state);
            state.brush_training.legacy_initialization_sha256 = Some(expected.sha256.clone());
        }
        let alias = dataset.join(format!("init.warm-{}.ply", uuid::Uuid::new_v4()));
        fs::copy(&init, &alias).unwrap();
        assert!(!dataset.join(RECEIPT).exists());
        let saved = serde_json::to_value(&state).unwrap();
        let cancel = CancellationToken::new();
        prepare_saved(
            &paths,
            &state,
            &dataset,
            &InitializationContract::SparsePoints,
            &cancel,
        )
        .await
        .unwrap();
        assert!(!init.exists());
        assert!(!alias.exists());
        let archive = archive_files(&paths);
        assert_eq!(archive.len(), 2);
        for file in archive {
            assert_eq!(validated_identity(&file), expected);
        }
        verify_start(
            &paths,
            &dataset,
            &InitializationContract::SparsePoints,
            &cancel,
        )
        .await
        .unwrap();
        assert_eq!(serde_json::to_value(&state).unwrap(), saved);
    }
}

#[tokio::test]
async fn transformed_cold_copy_without_receipt_or_saved_identity_is_not_guessed_owned() {
    let (_temporary, paths, dataset) = project();
    let source = gaussian(&paths.work.join("init-source.ply"), 1.);
    let mut state = saved_state(&source);
    gps_transform(&mut state);
    cold_after_fallback(&mut state);
    let init = dataset.join("init.ply");
    gaussian(&init, 2.);
    let original = fs::read(&init).unwrap();
    assert!(prepare_saved(
        &paths,
        &state,
        &dataset,
        &InitializationContract::SparsePoints,
        &CancellationToken::new(),
    )
    .await
    .is_err());
    assert_eq!(fs::read(init).unwrap(), original);
    assert!(!dataset.join(RECEIPT).exists());
    assert!(archive_files(&paths).is_empty());
}

#[tokio::test]
async fn startup_revalidation_rejects_changes_after_preparation_and_cold_visibility_regressions() {
    let (_temporary, paths, dataset) = project();
    let source = gaussian(&paths.work.join("init-source.ply"), 1.);
    let state = saved_state(&source);
    let staged = staging_path(&paths).unwrap();
    let identity = gaussian(staged.path(), 1.);
    let cancel = CancellationToken::new();
    let contract = install_staged(
        &paths,
        &state,
        &dataset,
        staged.path(),
        identity,
        "verified-source-coordinate-frame".into(),
        &cancel,
    )
    .await
    .unwrap();
    verify_start(&paths, &dataset, &contract, &cancel)
        .await
        .unwrap();
    gaussian(&dataset.join("init.ply"), 3.);
    assert!(verify_start(&paths, &dataset, &contract, &cancel)
        .await
        .is_err());
    assert!(verify_start(
        &paths,
        &dataset,
        &InitializationContract::SparsePoints,
        &cancel
    )
    .await
    .is_err());
}

#[tokio::test]
async fn cancelled_preparation_keeps_initializer_and_preserved_allowances() {
    let (_temporary, paths, dataset) = project();
    let source = gaussian(&dataset.join("init.ply"), 1.);
    let mut state = saved_state(&source);
    cold_after_fallback(&mut state);
    let saved = serde_json::to_value(&state).unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let result = prepare_saved(
        &paths,
        &state,
        &dataset,
        &InitializationContract::SparsePoints,
        &cancel,
    )
    .await;
    assert!(matches!(result, Err(SplatError::Cancelled)));
    assert_eq!(validated_identity(&dataset.join("init.ply")), source);
    assert!(archive_files(&paths).is_empty());
    assert_eq!(serde_json::to_value(&state).unwrap(), saved);
}
