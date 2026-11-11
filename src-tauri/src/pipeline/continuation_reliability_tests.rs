//! Engine-free continuation/publication regression tests. Every asset belongs
//! to a tempfile; no registered project, real media or hardware is accessed.
use super::*;
use crate::pipeline::{
    artifacts::record_saved_legacy_candidate,
    inputs::{GenerationInputs, InputCheckpoint, SourceCheckpoint},
    publication::{self, PublicationIdentity, PublicationPhase, PublicationTransaction},
    reshoot_connection,
};
use crate::reconstruction::direct_input::{Model, ModelImage, Observation, Point};
use std::io::Write;
use tokio_util::sync::CancellationToken;

const OLD: &str = "frame_0000000000.png";
const NEW: &str = "reshoot_current_0000000000.png";

#[tokio::test]
async fn reshoot_metadata_without_any_saved_source_groups_cannot_bypass_publication_guard() {
    let mut fixture = fixture(false);
    fixture.metadata.reshoot = Some(
        serde_json::from_value(serde_json::json!({
            "sourceProjectId": uuid::Uuid::new_v4(),
            "sourceProjectPath": fixture._temporary.path().join("missing-original"),
            "sourceFinalPly": fixture._temporary.path().join("missing-original/final.ply"),
            "reshootSourcePath": fixture._temporary.path().join("missing-input")
        }))
        .unwrap(),
    );
    let output = output(&fixture);
    let original = std::fs::read(&fixture.candidate).unwrap();
    let error = fixture
        .runner
        .publish_work_order(
            &fixture.manager,
            &fixture.paths,
            &mut fixture.metadata,
            &mut fixture.state,
            &fixture.candidate,
            output,
            "missing-reshoot-state".into(),
            None,
        )
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains(legacy_work_order::UNRESTORABLE_RECIPE));
    assert_eq!(std::fs::read(&fixture.candidate).unwrap(), original);
    assert!(!fixture.paths.project.join("final.ply").exists());
    assert_no_engines(&fixture);
}

struct Fixture {
    _temporary: tempfile::TempDir,
    manager: ProjectManager,
    paths: ProjectPaths,
    runner: PipelineRunner,
    metadata: ProjectMetadata,
    state: PipelineStateFile,
    candidate: PathBuf,
}

fn fixture(reshoot: bool) -> Fixture {
    let temporary = tempfile::tempdir().unwrap();
    let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), temporary.path().join("project"));
    for directory in [
        &paths.frames,
        &paths.colmap_frames,
        &paths.colmap,
        &paths.brush,
        &paths.logs,
    ] {
        std::fs::create_dir_all(directory).unwrap();
    }
    let candidate = paths.brush.join("final.ply.tmp");
    let mut file = std::fs::File::create(&candidate).unwrap();
    file.write_all(b"ply\nformat binary_little_endian 1.0\nelement vertex 1\n")
        .unwrap();
    for name in [
        "x", "y", "z", "f_dc_0", "f_dc_1", "f_dc_2", "opacity", "scale_0", "scale_1", "scale_2",
        "rot_0", "rot_1", "rot_2", "rot_3",
    ] {
        writeln!(file, "property float {name}").unwrap();
    }
    writeln!(file, "end_header").unwrap();
    for value in [
        0_f32, 0., 1., 0.5, 0.5, 0.5, 0., -2., -2., -2., 1., 0., 0., 0.,
    ] {
        file.write_all(&value.to_le_bytes()).unwrap();
    }
    drop(file);
    let mut state = PipelineStateFile::created(Quality::Fast);
    state.stage = PipelineStage::Failed;
    state.frames = Some(FrameState {
        extracted_frames: Some(2),
        ..Default::default()
    });
    if reshoot {
        state.reshoot = Some(ReshootState {
            recipe: Some(serde_json::from_value(serde_json::json!({
                "version": 2,
                "options": {"condition": "geometry", "allowFallback": false,
                    "sourceIdentity": "saved-source", "inputIdentity": "saved-input", "projection": "perspective"},
                "initial": {"reconstruction": "full", "training": "cold"},
                "actual": {"reconstruction": "full", "training": "cold"},
                "batchId": "current", "sourceNames": [OLD], "newNames": [NEW],
                "reconstructionFallbacks": 1, "trainingFallbacks": 1
            })).unwrap()),
            ..Default::default()
        });
    }
    let metadata = ProjectMetadata {
        id: paths.id,
        name: "continuation regression".into(),
        app_id: crate::project::PROJECT_APP_ID.into(),
        project_path: paths.project.clone(),
        source_path: temporary.path().join("unavailable-original"),
        quality: Quality::Fast,
        started_at: Some(Utc::now() - chrono::Duration::days(2)),
        duration_ms: Some(5_000),
        status: ProjectStatus::Failed,
        ..Default::default()
    };
    let manager = ProjectManager::for_diagnostics(temporary.path().to_path_buf());
    let runner = PipelineRunner::new(
        EnginePaths::from_root(temporary.path().join("missing-engines")),
        |_| {},
    );
    Fixture {
        _temporary: temporary,
        manager,
        paths,
        runner,
        metadata,
        state,
        candidate,
    }
}

fn output(fixture: &Fixture) -> ProjectOutput {
    let info = inspect_gaussian_ply(&fixture.candidate).unwrap();
    ProjectOutput {
        final_ply: fixture.paths.project.join("final.ply"),
        file_size: info.file_size,
        splat_count: info.splat_count,
        input_images: 2,
        registered_images: 2,
        registered_ratio: 1.,
        points_3d: 1,
    }
}

fn model(root: &Path, names: &[&str]) {
    Model {
        cameras: vec![ColmapCamera {
            id: 1,
            model_id: 1,
            model: "PINHOLE".into(),
            width: 4,
            height: 4,
            params: vec![2., 2., 2., 2.],
        }],
        images: names
            .iter()
            .enumerate()
            .map(|(index, name)| ModelImage {
                pose: crate::video::panorama::PosedImage {
                    id: index as u32 + 1,
                    camera_id: 1,
                    name: (*name).into(),
                    qvec: [1., 0., 0., 0.],
                    tvec: [0.; 3],
                },
                observations: vec![Observation {
                    x: 2.,
                    y: 2.,
                    point: Some(100),
                }],
            })
            .collect(),
        points: vec![Point {
            id: 100,
            xyz: [0., 0., 5.],
            rgb: [128; 3],
            error: 0.,
            track: (0..names.len())
                .map(|index| (index as u32 + 1, 0))
                .collect(),
        }],
    }
    .write(root)
    .unwrap();
}

fn candidate_hash(fixture: &Fixture) -> String {
    crate::video::panorama::fingerprint(&fixture.candidate).unwrap()
}

fn recipe_bytes(state: &PipelineStateFile) -> Vec<u8> {
    serde_json::to_vec(&(
        &state.brush_training,
        state
            .reshoot
            .as_ref()
            .and_then(|reshoot| reshoot.recipe.as_ref()),
        &state.generation_policy,
    ))
    .unwrap()
}

fn assert_no_engines(fixture: &Fixture) {
    for entry in std::fs::read_dir(&fixture.paths.logs).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        assert!(
            !["brush", "colmap", "ffmpeg", "ffprobe"]
                .iter()
                .any(|engine| name.starts_with(engine)),
            "unexpected engine log: {name}"
        );
    }
}

async fn prepared_transaction(fixture: &Fixture) -> PublicationTransaction {
    let transaction = PublicationTransaction {
        version: publication::PUBLICATION_VERSION,
        transaction_id: uuid::Uuid::new_v4(),
        identity: PublicationIdentity {
            attempt_id: "saved-training-attempt".into(),
            ..Default::default()
        },
        phase: PublicationPhase::Prepared,
        candidate_relative_path: fixture
            .candidate
            .strip_prefix(&fixture.paths.project)
            .unwrap()
            .to_path_buf(),
        destination_relative_path: "final.ply".into(),
        candidate_sha256: candidate_hash(fixture),
        output_sha256: None,
        output: output(fixture),
        started_at: Utc::now() - chrono::Duration::days(2),
        committed_at: None,
        project_views_reconciled_at: None,
        reconciled_at: None,
    };
    crate::project::manager::atomic_write_json(
        &fixture.paths.work.join(publication::TRANSACTION_FILE_NAME),
        &transaction,
    )
    .await
    .unwrap();
    transaction
}

#[tokio::test]
async fn legacy_candidate_continuation_does_not_count_two_days_of_stopped_time() {
    let mut fixture = fixture(false);
    fixture.state.brush_complete = true;
    fixture.state.brush_training.legacy_dataset_identity =
        Some("saved-content-bound-dataset".into());
    fixture.state.brush_training.resolved = Some(crate::presets::resolve_brush_training_preset(
        Quality::Fast,
        false,
        None,
        4,
        1,
    ));
    fixture.state.validated_output = Some(output(&fixture));
    record_saved_legacy_candidate(
        &fixture.paths,
        &mut fixture.state,
        &fixture.candidate,
        None,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let recipe = recipe_bytes(&fixture.state);
    let candidate = std::fs::read(&fixture.candidate).unwrap();
    let result = fixture
        .runner
        .execute_work_order(
            fixture.manager.clone(),
            fixture.paths.clone(),
            &mut fixture.metadata,
            fixture.state.clone(),
            None,
        )
        .await
        .unwrap();
    assert!(
        result.duration_ms >= 5_000 && result.duration_ms < 65_000,
        "stopped time leaked into {} ms",
        result.duration_ms
    );
    let saved = fixture
        .manager
        .read_state(&fixture.paths.state)
        .await
        .unwrap();
    assert_eq!(recipe_bytes(&saved), recipe);
    assert_eq!(std::fs::read(&fixture.candidate).unwrap(), candidate);
    assert_eq!(std::fs::read(&result.final_ply).unwrap(), candidate);
    assert_no_engines(&fixture);
}

#[tokio::test]
async fn committed_reshoot_reconciles_without_proof_model_or_changed_time() {
    let mut fixture = fixture(true);
    let publication = publication::publish_candidate(
        &fixture.paths.project,
        &fixture.paths.work.join(publication::TRANSACTION_FILE_NAME),
        &fixture.candidate,
        &fixture.paths.project.join("final.ply"),
        output(&fixture),
        PublicationIdentity {
            attempt_id: "already-committed".into(),
            ..Default::default()
        },
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let committed_at = publication.transaction.committed_at.unwrap();
    let before = std::fs::read(fixture.paths.project.join("final.ply")).unwrap();
    std::fs::remove_file(&fixture.candidate).unwrap();
    let recipe = recipe_bytes(&fixture.state);
    for _ in 0..2 {
        let result = fixture
            .runner
            .execute_work_order(
                fixture.manager.clone(),
                fixture.paths.clone(),
                &mut fixture.metadata,
                fixture.state.clone(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(result.duration_ms, 5_000);
        assert_eq!(result.completed_at, committed_at);
        assert_eq!(fixture.metadata.completed_at, Some(committed_at));
        assert_eq!(
            recipe_bytes(
                &fixture
                    .manager
                    .read_state(&fixture.paths.state)
                    .await
                    .unwrap()
            ),
            recipe
        );
        assert_eq!(std::fs::read(&result.final_ply).unwrap(), before);
    }
    assert_no_engines(&fixture);
}

#[tokio::test]
async fn prepared_reshoot_without_either_payload_can_rebuild_without_connection_proof() {
    let mut fixture = fixture(true);
    let transaction = prepared_transaction(&fixture).await;
    std::fs::remove_file(&fixture.candidate).unwrap();
    fixture.state.brush_complete = true;
    fixture.state.validated_output = Some(transaction.output);
    let recipe = recipe_bytes(&fixture.state);
    assert!(fixture
        .runner
        .reconcile_publish_transaction(
            &fixture.manager,
            &fixture.paths,
            &mut fixture.metadata,
            &mut fixture.state
        )
        .await
        .unwrap()
        .is_none());
    assert!(!fixture.state.brush_complete);
    assert!(fixture.state.validated_output.is_none());
    assert_eq!(recipe_bytes(&fixture.state), recipe);
    assert!(!fixture.paths.project.join("final.ply").exists());
    assert!(!fixture
        .paths
        .work
        .join(publication::TRANSACTION_FILE_NAME)
        .exists());
    assert!(fixture
        .paths
        .work
        .join(format!(
            "publish-transaction.rebuild-required-{}.json",
            transaction.transaction_id
        ))
        .is_file());
    assert_no_engines(&fixture);
}

#[tokio::test]
async fn prepared_reshoot_with_payload_but_without_owned_model_stays_unpublished() {
    let mut fixture = fixture(true);
    prepared_transaction(&fixture).await;
    let before = std::fs::read(&fixture.candidate).unwrap();
    let recipe = recipe_bytes(&fixture.state);
    assert!(fixture
        .runner
        .reconcile_publish_transaction(
            &fixture.manager,
            &fixture.paths,
            &mut fixture.metadata,
            &mut fixture.state
        )
        .await
        .is_err());
    assert!(!fixture.paths.project.join("final.ply").exists());
    assert_eq!(std::fs::read(&fixture.candidate).unwrap(), before);
    assert_eq!(recipe_bytes(&fixture.state), recipe);
    assert_no_engines(&fixture);
}

#[tokio::test]
async fn full_reshoot_candidate_requires_old_and_new_views_without_spending_more_retries() {
    let mut fixture = fixture(true);
    let selected = fixture.paths.colmap.join("sparse-full");
    model(&selected, &[NEW]);
    let recipe = recipe_bytes(&fixture.state);
    let hash = candidate_hash(&fixture);
    let error = fixture
        .runner
        .ensure_reshoot_candidate_connection(&fixture.paths, &mut fixture.state, hash.clone())
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains(reshoot_connection::DISCONNECTED_MESSAGE));
    assert_eq!(recipe_bytes(&fixture.state), recipe);
    assert!(!fixture.paths.project.join("final.ply").exists());
    model(&selected, &[OLD, NEW]);
    fixture
        .runner
        .ensure_reshoot_candidate_connection(&fixture.paths, &mut fixture.state, hash.clone())
        .await
        .unwrap();
    assert!(reshoot_connection::candidate_is_proven(&fixture.state, &hash).unwrap());
    assert_eq!(recipe_bytes(&fixture.state), recipe);
    assert_no_engines(&fixture);
}

fn unified_groups(state: &mut PipelineStateFile) {
    let mut inputs = InputCheckpoint::new(GenerationInputs::Materials { items: vec![] });
    inputs.prepared = true;
    for (id, name) in [("__source", OLD), ("current", NEW)] {
        inputs.sources.push(SourceCheckpoint {
            id: id.into(),
            identity: format!("saved-{id}"),
            camera_ids: vec![1],
            camera_assignment: None,
            projection: crate::video::panorama::VideoProjection::Perspective,
            names: vec![name.into()],
            originals: vec![],
            video: None,
            image_sequence: None,
            dimensions: vec![[4, 4]],
            working_dimensions: Some([4, 4]),
            frames: None,
            panorama_records: vec![],
            adopted: 0,
            bridge_frames: vec![],
        });
    }
    state.inputs = Some(inputs);
}

#[tokio::test]
async fn matching_gps_candidate_uses_its_actual_model_not_visual_authority() {
    for gps_connected in [false, true] {
        let mut fixture = fixture(true);
        unified_groups(&mut fixture.state);
        let visual = fixture.paths.colmap.join("authoritative");
        let gps = fixture.paths.colmap.join("gps-v1/model");
        model(&visual, if gps_connected { &[NEW] } else { &[OLD, NEW] });
        model(&gps, if gps_connected { &[OLD, NEW] } else { &[NEW] });
        fixture.state.gps = Some(crate::reconstruction::gps::Checkpoint {
            status: "matching".into(),
            output_identity: Some(gps_runner::model_identity(&gps).unwrap()),
            ..crate::reconstruction::gps::Checkpoint::pending()
        });
        let hash = candidate_hash(&fixture);
        let result = fixture
            .runner
            .ensure_reshoot_candidate_connection(&fixture.paths, &mut fixture.state, hash.clone())
            .await;
        if gps_connected {
            result.unwrap();
            assert!(reshoot_connection::candidate_is_proven(&fixture.state, &hash).unwrap());
        } else {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains(reshoot_connection::DISCONNECTED_MESSAGE));
        }
        assert!(!fixture.paths.project.join("final.ply").exists());
        assert_no_engines(&fixture);
    }
}

#[tokio::test]
async fn bound_candidate_proof_survives_model_cleanup_but_not_candidate_changes() {
    let mut fixture = fixture(true);
    let selected = fixture.paths.colmap.join("sparse-full");
    model(&selected, &[OLD, NEW]);
    let hash = candidate_hash(&fixture);
    fixture
        .runner
        .ensure_reshoot_candidate_connection(&fixture.paths, &mut fixture.state, hash.clone())
        .await
        .unwrap();
    fixture.state = serde_json::from_slice(&serde_json::to_vec(&fixture.state).unwrap()).unwrap();
    std::fs::remove_dir_all(selected).unwrap();
    fixture
        .runner
        .ensure_reshoot_candidate_connection(&fixture.paths, &mut fixture.state, hash)
        .await
        .unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&fixture.candidate)
        .unwrap()
        .write_all(b"changed")
        .unwrap();
    let changed = candidate_hash(&fixture);
    assert!(fixture
        .runner
        .ensure_reshoot_candidate_connection(&fixture.paths, &mut fixture.state, changed)
        .await
        .is_err());
    assert!(!fixture.paths.project.join("final.ply").exists());
    assert_no_engines(&fixture);
}

#[tokio::test]
async fn disconnected_full_reshoot_repeats_shared_training_guard_without_preparing_or_rebuilding() {
    let mut fixture = fixture(true);
    unified_groups(&mut fixture.state);
    fixture.state.generation_policy = Some(Default::default());
    fixture.state.features_complete = true;
    fixture.state.matching_complete = true;
    fixture.state.reconstruction_complete = true;
    let recipe = fixture
        .state
        .reshoot
        .as_ref()
        .unwrap()
        .recipe
        .as_ref()
        .unwrap();
    fixture.state.inputs.as_mut().unwrap().execution_plan =
        Some(crate::pipeline::execution_plan::ExecutionPlan::reshoot(
            recipe,
            crate::pipeline::execution_plan::SnapshotModel {
                relative_path: "unused-prevalidated-source".into(),
                sha256: "saved-source-model".into(),
                coordinate_frame_id: "saved-coordinate-frame".into(),
                has_erp: false,
                fixed_images: vec![],
                fixed_cameras: vec![],
            },
        ));
    let owned_model = fixture.paths.colmap.join("authoritative");
    model(&owned_model, &[NEW]);
    let report = ReconstructionReport {
        input_images: 2,
        registered_images: 1,
        registered_ratio: 0.5,
        points_3d: 1,
        quality: ReconstructionQuality::Warning,
    };
    let acceleration = crate::engines::ColmapAccelerationStatus {
        backend: crate::engines::ColmapBackend::Cpu,
        detection_state: crate::engines::GpuDetectionState::Unavailable,
        reason_code: crate::engines::AccelerationReasonCode::NoNvidiaGpu,
        reason: "CPU-only engine-free fixture".into(),
        device: None,
        requirements: crate::engines::AccelerationRequirements {
            minimum_driver_version: "0".into(),
            minimum_compute_capability: "0".into(),
        },
        detected_nvidia_device_count: 0,
    };
    fixture
        .manager
        .write_state(&fixture.paths.state, &fixture.state)
        .await
        .unwrap();
    let saved_recipe = recipe_bytes(&fixture.state);
    let saved_state = std::fs::read(&fixture.paths.state).unwrap();
    let model_files = reusable_model_files(&owned_model).unwrap();
    let model_bytes = model_files
        .iter()
        .map(|path| std::fs::read(path).unwrap())
        .collect::<Vec<_>>();

    // A compatibility adapter may supply an already validated reconstruction
    // directly. Exercise that real common training tail, not a mocked guard.
    // No snapshot/database is needed before the PrepareTraining success check.
    for _ in 0..2 {
        let restored = fixture
            .manager
            .read_state(&fixture.paths.state)
            .await
            .unwrap();
        let error = fixture
            .runner
            .run_inputs_after_reconstruction(
                &fixture.manager,
                &fixture.paths,
                &mut fixture.metadata,
                restored,
                &acceleration,
                owned_model.clone(),
                report.clone(),
                None,
            )
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains(reshoot_connection::DISCONNECTED_MESSAGE));
        let saved = fixture
            .manager
            .read_state(&fixture.paths.state)
            .await
            .unwrap();
        assert_eq!(recipe_bytes(&saved), saved_recipe);
        assert!(
            saved.features_complete && saved.matching_complete && saved.reconstruction_complete
        );
        assert_eq!(std::fs::read(&fixture.paths.state).unwrap(), saved_state);
        assert!(!fixture.paths.brush.join("dataset").exists());
        assert!(!fixture.paths.colmap.join("database.db").exists());
        assert!(!fixture.paths.work.join("reshoot-snapshot-v3").exists());
        assert!(!fixture.paths.project.join("final.ply").exists());
        for (path, expected) in model_files.iter().zip(&model_bytes) {
            assert_eq!(std::fs::read(path).unwrap(), *expected);
        }
        assert_no_engines(&fixture);
    }
}
