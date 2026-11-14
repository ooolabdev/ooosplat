use super::*;

#[tokio::test]
async fn new_colmap_tasks_are_rejected_without_registering_an_execution() {
    let root = tempfile::tempdir().unwrap();
    let model = root.path().join("model");
    let images = root.path().join("images");
    std::fs::create_dir(&model).unwrap();
    std::fs::create_dir(&images).unwrap();
    let service = TaskService::at(root.path().join("registry"));
    let error = service.create(None, model.clone(), Quality::Balanced,
        root.path().join("projects"), true, "gui", None,
        Some(serde_json::json!({"inputs":{"mode":"colmap","input":{"model":model,"images":images}}})))
        .await.unwrap_err();
    assert_eq!(error.code, "UNSUPPORTED_INPUT");
    assert_eq!(
        error.reason,
        Some(crate::error::InputAdmissionReason::ColmapDisabled)
    );
    assert!(service.all().await.unwrap().is_empty());
    assert!(!root.path().join("projects").exists());
}

#[tokio::test]
async fn colmap_import_disable_does_not_block_saved_preproject_continuation_or_idempotency() {
    let (root, service, task) = fixture().await;
    let model = root.path().join("saved-model");
    let images = root.path().join("saved-images");
    std::fs::create_dir(&model).unwrap();
    std::fs::create_dir(&images).unwrap();
    std::fs::write(model.join("cameras.txt"), "1 PINHOLE 4 3 4 4 2 1.5\n").unwrap();
    std::fs::write(
        model.join("images.txt"),
        "1 1 0 0 0 0 0 0 1 1.png\n1 1 1\n2 1 0 0 0 1 0 0 1 2.png\n1 1 1\n",
    )
    .unwrap();
    std::fs::write(model.join("points3D.txt"), "1 0 0 3 12 34 56 0 1 0 2 0\n").unwrap();
    for name in ["1.png", "2.png"] {
        image::RgbImage::new(4, 3).save(images.join(name)).unwrap();
    }
    let run = notification_run(&service, task.task_id);
    let parameters = {
        let mut records = service.records();
        let saved = records.tasks.get_mut(&task.task_id).unwrap();
        saved.status = TaskStatus::Failed;
        saved.input_path = std::fs::canonicalize(&model).unwrap();
        saved.input_type = crate::project::ProjectInputType::Images;
        saved.request_parameters.as_mut().unwrap()["inputs"] = serde_json::json!({
            "mode":"colmap","input":{"model":std::fs::canonicalize(&model).unwrap(),
                "images":std::fs::canonicalize(&images).unwrap()}});
        saved.request_parameters.clone()
    };
    // An existing identity is not a request to import a new reconstruction.
    let existing = service
        .create(
            Some(task.task_id),
            model.clone(),
            Quality::Balanced,
            root.path().join("projects"),
            true,
            "gui",
            None,
            parameters,
        )
        .await
        .unwrap();
    assert_eq!(existing.task_id, task.task_id);
    *service.executor.lock().unwrap() = Some(Arc::new(|execution, _| {
        Box::pin(async move {
            assert_eq!(
                execution.request_parameters.as_ref().unwrap()["inputs"]["mode"],
                "colmap"
            );
            assert!(execution.project_id.is_none());
            Err(crate::error::SplatError::Cancelled)
        })
    }));
    service
        .continue_identified(task.task_id, run, default_engine_paths(None), None)
        .await
        .unwrap();
    let stopped = service.wait(task.task_id).await.unwrap();
    assert_eq!(stopped.status, TaskStatus::Cancelled);
    assert_eq!(stopped.runs[1].kind, "continue");
}

#[tokio::test]
async fn ba_scope_is_frozen_before_project_creation_and_old_reservations_keep_their_identity() {
    use sha2::{Digest, Sha256};
    for version in [None, Some(crate::pipeline::ba_policy::VERSION)] {
        let (root, service, task) = fixture().await;
        assert_eq!(
            task.ba_policy_version,
            Some(crate::pipeline::ba_policy::VERSION)
        );
        {
            let mut records = service.records();
            let saved = records.tasks.get_mut(&task.task_id).unwrap();
            saved.ba_policy_version = version;
            // Isolate the pre-camera-policy reservation bytes in this BA
            // compatibility test. Camera policy has its own regression below.
            saved.camera_grouping_policy_version = None;
            sync_saved_camera_grouping_policy(&mut saved.request_parameters, None).unwrap();
            saved.ordinary_image_preparation_version = None;
            sync_saved_ordinary_image_preparation(&mut saved.request_parameters, None).unwrap();
        }
        *service.executor.lock().unwrap() = Some(Arc::new(move |execution, _| {
            Box::pin(async move {
                assert_eq!(execution.ba_policy_version, version);
                let intent = execution.creation_intent.as_ref().unwrap();
                assert_eq!(intent.ba_policy_version, version);
                if version.is_none() {
                    let old_bytes = serde_json::to_vec(&serde_json::json!({
                    "input":execution.input_path,"request":execution.request_parameters,
                    "quality":execution.quality,"configuration":execution.configuration,
                    "source":execution.source_project_id,"root":execution.projects_root,
                    "planner":execution.planner_enabled,"brushExecutionOptions":execution.brush_execution_options,
                })).unwrap();
                    assert_eq!(
                        intent.request_identity,
                        format!("{:x}", Sha256::digest(old_bytes))
                    );
                }
                Err(crate::error::SplatError::Process(
                    "creation scope test failure".into(),
                ))
            })
        }));
        service
            .start(task.task_id, default_engine_paths(None), None)
            .await
            .unwrap();
        let stopped = service.wait(task.task_id).await.unwrap();
        service.persist().await.unwrap();
        let restored = TaskService::at(root.path().join("registry"));
        let original_intent = stopped.creation_intent.clone();
        let expected = original_intent.clone();
        *restored.executor.lock().unwrap() = Some(Arc::new(move |execution, _| {
            let expected = expected.clone();
            Box::pin(async move {
                assert_eq!(execution.ba_policy_version, version);
                assert_eq!(execution.creation_intent, expected);
                Err(crate::error::SplatError::Cancelled)
            })
        }));
        restored
            .continue_identified(
                task.task_id,
                stopped.run_id.unwrap(),
                default_engine_paths(None),
                None,
            )
            .await
            .unwrap();
        let stopped = restored.wait(task.task_id).await.unwrap();
        assert_eq!(stopped.ba_policy_version, version);
        assert_eq!(stopped.creation_intent, original_intent);
    }
}

#[tokio::test]
async fn camera_grouping_policy_is_internal_and_idempotency_preserves_old_task_policy() {
    let (root, service, task) = fixture().await;
    assert_eq!(task.camera_grouping_policy_version, Some(1));
    assert_eq!(task.ordinary_image_preparation_version, Some(1));
    assert_eq!(
        task.request_parameters.as_ref().unwrap()["ordinaryImagePreparationVersion"],
        1
    );
    assert_eq!(
        task.request_parameters.as_ref().unwrap()["cameraGroupingPolicyVersion"],
        1
    );
    let mut request = task.request_parameters.clone().unwrap();
    request["cameraGroupingPolicyVersion"] = Value::from(999);
    request["ordinaryImagePreparationVersion"] = Value::from(999);
    let same = service
        .create(
            None,
            task.input_path.clone(),
            task.quality,
            task.projects_root.clone(),
            true,
            "gui",
            task.client_request_id.clone(),
            Some(request.clone()),
        )
        .await
        .unwrap();
    assert_eq!(same.task_id, task.task_id);
    assert_eq!(same.camera_grouping_policy_version, Some(1));
    assert_eq!(same.ordinary_image_preparation_version, Some(1));

    let new = service
        .create(
            None,
            task.input_path.clone(),
            task.quality,
            task.projects_root.clone(),
            true,
            "gui",
            Some("new-camera-request".into()),
            Some(request.clone()),
        )
        .await
        .unwrap();
    assert_eq!(new.camera_grouping_policy_version, Some(1));
    assert_eq!(new.ordinary_image_preparation_version, Some(1));
    assert_eq!(
        new.request_parameters.as_ref().unwrap()["ordinaryImagePreparationVersion"],
        1
    );
    assert_eq!(
        new.request_parameters.as_ref().unwrap()["cameraGroupingPolicyVersion"],
        1
    );

    let legacy = {
        let mut records = service.records();
        let saved = records.tasks.get_mut(&task.task_id).unwrap();
        saved.camera_grouping_policy_version = None;
        sync_saved_camera_grouping_policy(&mut saved.request_parameters, None).unwrap();
        saved.ordinary_image_preparation_version = None;
        sync_saved_ordinary_image_preparation(&mut saved.request_parameters, None).unwrap();
        saved.clone()
    };
    let bytes = serde_json::to_value(&legacy).unwrap();
    assert!(bytes.get("camera_grouping_policy_version").is_none());
    assert!(bytes.get("ordinary_image_preparation_version").is_none());
    let decoded: TaskRecord = serde_json::from_value(bytes).unwrap();
    assert_eq!(decoded.camera_grouping_policy_version, None);
    assert_eq!(decoded.ordinary_image_preparation_version, None);
    let same = service
        .create(
            None,
            task.input_path.clone(),
            task.quality,
            task.projects_root.clone(),
            true,
            "gui",
            task.client_request_id.clone(),
            Some(request),
        )
        .await
        .unwrap();
    assert_eq!(same.task_id, legacy.task_id);
    assert_eq!(same.camera_grouping_policy_version, None);
    assert_eq!(same.ordinary_image_preparation_version, None);
    service.persist().await.unwrap();
    let restored = TaskService::at(root.path().join("registry"))
        .get(task.task_id)
        .await
        .unwrap();
    assert_eq!(restored.camera_grouping_policy_version, None);
    assert_eq!(restored.ordinary_image_preparation_version, None);
    assert!(restored
        .request_parameters
        .as_ref()
        .unwrap()
        .get("ordinaryImagePreparationVersion")
        .is_none());
    assert!(restored
        .request_parameters
        .unwrap()
        .get("cameraGroupingPolicyVersion")
        .is_none());
}

#[tokio::test]
async fn new_camera_policy_rejects_mixed_image_dimensions_before_registering_a_task() {
    use crate::pipeline::inputs::{GenerationInputs, InputSource, MaterialInput};
    for explicit_inputs in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let images = root.path().join("images");
        std::fs::create_dir(&images).unwrap();
        image::RgbImage::new(4, 3)
            .save(images.join("a.png"))
            .unwrap();
        image::RgbImage::new(8, 3)
            .save(images.join("b.png"))
            .unwrap();
        let parameters = explicit_inputs.then(|| serde_json::json!({
            "inputs": GenerationInputs::Materials { items: vec![MaterialInput {
                id: "photos".into(), source: InputSource::ImageFolder { path: images.clone() },
                identity: None, projection: crate::video::panorama::VideoProjection::Perspective,
                image_matching_strategy: Default::default(), camera_relation: None,
                confirmed_small_images: true, confirmed_image_order: true, frame_removal_fps: None,
            }] }
        }));
        let service = TaskService::at(root.path().join("registry"));
        let error = service
            .create(
                None,
                images,
                Quality::Fast,
                root.path().join("projects"),
                true,
                "gui",
                None,
                parameters,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "TASK_ERROR");
        assert!(error.message.contains("b.png"));
        assert!(error.message.contains("8×3"));
        assert!(service.all().await.unwrap().is_empty());
        assert!(!root.path().join("projects").exists());
    }
}

#[tokio::test]
async fn camera_grouping_policy_survives_interrupted_creation_without_upgrading_legacy_intents() {
    use sha2::{Digest, Sha256};
    for version in [
        None,
        Some(crate::pipeline::inputs::CAMERA_GROUPING_POLICY_VERSION),
    ] {
        let (root, service, task) = fixture().await;
        {
            let mut records = service.records();
            let saved = records.tasks.get_mut(&task.task_id).unwrap();
            saved.camera_grouping_policy_version = version;
            sync_saved_camera_grouping_policy(&mut saved.request_parameters, version).unwrap();
            saved.ordinary_image_preparation_version = version;
            sync_saved_ordinary_image_preparation(&mut saved.request_parameters, version).unwrap();
        }
        *service.executor.lock().unwrap() = Some(Arc::new(move |execution, _| {
            Box::pin(async move {
                assert_eq!(execution.camera_grouping_policy_version, version);
                assert_eq!(execution.ordinary_image_preparation_version, version);
                let intent = execution.creation_intent.as_ref().unwrap();
                assert_eq!(intent.camera_grouping_policy_version, version);
                assert_eq!(intent.ordinary_image_preparation_version, version);
                let mut expected = serde_json::json!({
                    "input":execution.input_path,"request":execution.request_parameters,
                    "quality":execution.quality,"configuration":execution.configuration,
                    "source":execution.source_project_id,"root":execution.projects_root,
                    "planner":execution.planner_enabled,"brushExecutionOptions":execution.brush_execution_options,
                });
                if let Some(ba) = execution.ba_policy_version {
                    expected["baPolicyVersion"] = Value::from(ba);
                }
                if let Some(camera) = version {
                    expected["cameraGroupingPolicyVersion"] = Value::from(camera);
                    expected["ordinaryImagePreparationVersion"] = Value::from(camera);
                }
                assert_eq!(
                    intent.request_identity,
                    format!(
                        "{:x}",
                        Sha256::digest(serde_json::to_vec(&expected).unwrap())
                    )
                );
                let old_intent: crate::project::creation::ProjectCreationIntent =
                    serde_json::from_value(serde_json::to_value(intent).unwrap()).unwrap();
                assert_eq!(old_intent.camera_grouping_policy_version, version);
                assert_eq!(old_intent.ordinary_image_preparation_version, version);
                Err(crate::error::SplatError::Process(
                    "creation interrupted".into(),
                ))
            })
        }));
        service
            .start(task.task_id, default_engine_paths(None), None)
            .await
            .unwrap();
        let stopped = service.wait(task.task_id).await.unwrap();
        let intent = stopped.creation_intent.clone();
        service.persist().await.unwrap();
        let restored = TaskService::at(root.path().join("registry"));
        let expected = intent.clone();
        *restored.executor.lock().unwrap() = Some(Arc::new(move |execution, _| {
            let expected = expected.clone();
            Box::pin(async move {
                assert_eq!(execution.camera_grouping_policy_version, version);
                assert_eq!(execution.creation_intent, expected);
                assert_eq!(execution.ordinary_image_preparation_version, version);
                Err(crate::error::SplatError::Cancelled)
            })
        }));
        restored
            .continue_identified(
                task.task_id,
                stopped.run_id.unwrap(),
                default_engine_paths(None),
                None,
            )
            .await
            .unwrap();
        let stopped = restored.wait(task.task_id).await.unwrap();
        assert_eq!(stopped.camera_grouping_policy_version, version);
        assert_eq!(stopped.creation_intent, intent);
        assert_eq!(stopped.ordinary_image_preparation_version, version);
    }
}

#[tokio::test]
async fn continuation_uses_authoritative_camera_policy_without_changing_saved_features_or_recipe() {
    for version in [
        None,
        Some(crate::pipeline::inputs::CAMERA_GROUPING_POLICY_VERSION),
    ] {
        // The two internal policies are independent. In particular, tasks
        // created after camera sharing but before this pixel fix keep None.
        let preparation_version = if version.is_some() { None } else { Some(1) };
        let (root, service, task) = fixture().await;
        let previous_run = notification_run(&service, task.task_id);
        let project = root.path().join("saved-camera-work-order");
        std::fs::create_dir(&project).unwrap();
        let mut state = crate::project::PipelineStateFile::created(task.quality);
        state.camera_grouping_policy_version = version;
        state.ordinary_image_preparation_version = preparation_version;
        state.generation_policy = task.generation_policy;
        state.configuration = Some(task.configuration.clone());
        state.features_complete = true;
        state.matching_complete = true;
        state.brush_training.oom_retry_used = true;
        atomic_write_json(&project.join("state.json"), &state)
            .await
            .unwrap();
        let bytes = std::fs::read(project.join("state.json")).unwrap();
        {
            let mut records = service.records();
            let saved = records.tasks.get_mut(&task.task_id).unwrap();
            saved.project_id = Some(Uuid::new_v4());
            saved.project_path = Some(project.clone());
            saved.status = TaskStatus::Failed;
            saved.camera_grouping_policy_version = if version.is_none() { Some(1) } else { None };
            saved.ordinary_image_preparation_version = version;
        }
        persist_bound_project_metadata(&service, task.task_id).await;
        *service.executor.lock().unwrap() = Some(Arc::new(move |execution, _| {
            Box::pin(async move {
                assert_eq!(execution.camera_grouping_policy_version, version);
                assert_eq!(
                    execution.ordinary_image_preparation_version,
                    preparation_version
                );
                assert_eq!(
                    execution
                        .request_parameters
                        .as_ref()
                        .and_then(|p| p.get("ordinaryImagePreparationVersion"))
                        .and_then(Value::as_u64),
                    preparation_version.map(u64::from)
                );
                assert_eq!(
                    execution
                        .request_parameters
                        .as_ref()
                        .and_then(|p| p.get("cameraGroupingPolicyVersion"))
                        .and_then(Value::as_u64),
                    version.map(u64::from)
                );
                Err(crate::error::SplatError::Cancelled)
            })
        }));
        service
            .continue_identified(task.task_id, previous_run, default_engine_paths(None), None)
            .await
            .unwrap();
        let stopped = service.wait(task.task_id).await.unwrap();
        assert_eq!(stopped.camera_grouping_policy_version, version);
        assert_eq!(stopped.runs[1].kind, "continue");
        assert_eq!(
            stopped.ordinary_image_preparation_version,
            preparation_version
        );
        assert_eq!(std::fs::read(project.join("state.json")).unwrap(), bytes);
    }
}

#[tokio::test]
async fn creation_identity_is_durable_before_execution_and_reused_after_restart() {
    let (root, service, task) = fixture().await;
    *service.executor.lock().unwrap() = Some(Arc::new(|execution, _| {
        Box::pin(async move {
            assert!(execution.creation_intent.is_some());
            Err(crate::error::SplatError::Process("creation fault".into()))
        })
    }));
    service
        .start(task.task_id, default_engine_paths(None), None)
        .await
        .unwrap();
    let failed = service.wait(task.task_id).await.unwrap();
    let intent = failed.creation_intent.clone().unwrap();
    assert_eq!(intent.task_id, task.task_id);
    assert!(failed.project_id.is_none());
    let restored = TaskService::at(root.path().join("registry"));
    let expected = intent.clone();
    *restored.executor.lock().unwrap() = Some(Arc::new(move |execution, _| {
        let expected = expected.clone();
        Box::pin(async move {
            assert_eq!(execution.creation_intent, Some(expected));
            Err(crate::error::SplatError::Cancelled)
        })
    }));
    restored
        .continue_identified(
            task.task_id,
            failed.run_id.unwrap(),
            default_engine_paths(None),
            None,
        )
        .await
        .unwrap();
    let final_task = restored.wait(task.task_id).await.unwrap();
    assert_eq!(final_task.creation_intent, Some(intent));
    assert_eq!(final_task.status, TaskStatus::Cancelled);
}

#[test]
fn local_gps_summary_survives_bounded_task_view_without_coordinates() {
    let summary = serde_json::json!({"detectedSourceIds":["images-2"],"status":"fallback"});
    let snapshot = actual_snapshot(serde_json::json!({"inputs":{"gpsAssistance":summary}}));
    assert_eq!(snapshot.pointer("/inputs/gpsAssistance").unwrap(), &summary);
    let decoded: crate::pipeline::inputs::GpsAssistance =
        serde_json::from_value(summary.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), summary);
}

#[tokio::test]
async fn ready_project_binding_is_persisted_and_stale_runs_cannot_replace_it() {
    let (root, service, task) = fixture().await;
    let signal = Arc::new(tokio::sync::Notify::new());
    let stop = signal.clone();
    *service.executor.lock().unwrap() = Some(Arc::new(move |_, _| {
        let stop = stop.clone();
        Box::pin(async move {
            stop.notified().await;
            Err(crate::error::SplatError::Cancelled)
        })
    }));
    let started = service
        .start(task.task_id, default_engine_paths(None), None)
        .await
        .unwrap();
    let intent = service
        .get(task.task_id)
        .await
        .unwrap()
        .creation_intent
        .unwrap();
    let metadata = crate::project::ProjectMetadata {
        id: intent.project_id,
        workspace_task_id: Some(task.task_id),
        project_path: intent.project_path.clone(),
        ..Default::default()
    };
    service
        .bind_created_project(task.task_id, started.run_id.unwrap(), metadata.clone())
        .await
        .unwrap();
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.path().join("registry/tasks.json")).unwrap())
            .unwrap();
    assert_eq!(
        saved["tasks"][task.task_id.to_string()]["project_id"],
        intent.project_id.to_string()
    );
    let before = std::fs::read(root.path().join("registry/tasks.json")).unwrap();
    assert_eq!(
        service
            .bind_created_project(task.task_id, Uuid::new_v4(), metadata)
            .await
            .unwrap_err()
            .code,
        "STALE_RUN_ID"
    );
    assert_eq!(
        std::fs::read(root.path().join("registry/tasks.json")).unwrap(),
        before
    );
    signal.notify_one();
    service.wait(task.task_id).await.unwrap();
}

#[test]
fn status_summary_does_not_expose_internal_recovery_or_focal_audit_records() {
    let value = actual_snapshot(serde_json::json!({
        "inputs": {
            "recoverySources": [{"originals":["private/source.mov"],"panoramaRecords":[{"pts":42}]}],
            "temporaryPngFocalRecords": [{"checkpoint":"internal"}],
            "temporaryPngFocalPendingRecords": [{"checkpoint":"pending"}],
            "sources": [{"id":"video","originals":["private/source.mov"],"bridgeFrames":[]}]
        }
    }));
    for field in [
        "recoverySources",
        "temporaryPngFocalRecords",
        "temporaryPngFocalPendingRecords",
    ] {
        assert!(value["inputs"].get(field).is_none());
    }
    assert!(value["inputs"]["sources"][0].get("originals").is_none());
}

#[test]
fn generation_entry_future_stays_heap_boxed() {
    let root = tempfile::tempdir().unwrap();
    let service = TaskService::at(root.path().join("registry"));
    let engines = default_engine_paths(None);
    let start = service.start_inner(Uuid::nil(), engines.clone(), None, false, None);
    assert!(std::mem::size_of_val(&start) < 64 * 1024);
    let runner = PipelineRunner::new(engines, |_| {});
    let input = root.path().join("input.mp4");
    let projects = root.path().join("projects");
    let generate = runner.generate(&input, Quality::Fast, &projects);
    assert!(
        std::mem::size_of_val(&generate) < 1024,
        "the public generation entry must not embed the full pipeline future on a Tokio worker stack"
    );
}

#[tokio::test]
async fn cli_and_desktop_registries_share_one_os_execution_lock() {
    let (root, first, task) = fixture().await;
    let lock = root.path().join("shared-generation.lock");
    let first = first.with_run_lock(lock.clone());
    let second = TaskService::at(root.path().join("cli-records")).with_run_lock(lock);
    let next = second
        .create(
            None,
            root.path().join("input.mp4"),
            Quality::Fast,
            root.path().join("projects"),
            true,
            "cli",
            None,
            None,
        )
        .await
        .unwrap();
    let signal = Arc::new(tokio::sync::Notify::new());
    let captured = signal.clone();
    *first.executor.lock().unwrap() = Some(Arc::new(move |_, _| {
        let signal = captured.clone();
        Box::pin(async move {
            signal.notified().await;
            Err(crate::error::SplatError::Cancelled)
        })
    }));
    *second.executor.lock().unwrap() = Some(Arc::new(|_, _| {
        Box::pin(async { Err(crate::error::SplatError::Cancelled) })
    }));
    let receipt = first
        .start(task.task_id, default_engine_paths(None), None)
        .await
        .unwrap();
    assert!(!first.execution_settled(receipt.run_id));
    assert_eq!(
        second
            .start(next.task_id, default_engine_paths(None), None)
            .await
            .unwrap_err()
            .code,
        "TASK_BUSY"
    );
    assert_eq!(
        second.get(next.task_id).await.unwrap().status,
        TaskStatus::Created
    );
    signal.notify_one();
    first.wait(task.task_id).await.unwrap();
    second
        .start(next.task_id, default_engine_paths(None), None)
        .await
        .unwrap();
    second.wait(next.task_id).await.unwrap();
    assert!(first.execution_settled(receipt.run_id));
}

#[tokio::test]
async fn continue_requires_matching_stopped_run_and_readable_state_not_completed_work() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("clip.mp4");
    std::fs::write(&input, b"fixture").unwrap();
    let service = TaskService::at(root.path().join("registry"));
    let task = service
        .create(
            None,
            input,
            Quality::High,
            root.path().join("projects"),
            true,
            "mcp",
            None,
            None,
        )
        .await
        .unwrap();
    let run = Uuid::new_v4();
    assert_eq!(
        service
            .resume_identified(task.task_id, run, default_engine_paths(None), None)
            .await
            .unwrap_err()
            .code,
        "STALE_RUN_ID"
    );
    {
        let mut records = service.records();
        let t = records.tasks.get_mut(&task.task_id).unwrap();
        t.run_id = Some(run);
        t.status = TaskStatus::Created;
    }
    assert_eq!(
        service
            .resume_identified(task.task_id, run, default_engine_paths(None), None)
            .await
            .unwrap_err()
            .code,
        "TASK_NOT_RESUMABLE"
    );
    {
        service
            .records()
            .tasks
            .get_mut(&task.task_id)
            .unwrap()
            .status = TaskStatus::Cancelled;
    }
    let project = root.path().join("checkpoint");
    std::fs::create_dir(&project).unwrap();
    {
        let mut records = service.records();
        let t = records.tasks.get_mut(&task.task_id).unwrap();
        t.project_path = Some(project.clone());
        t.project_id = Some(Uuid::new_v4());
    }
    persist_bound_project_metadata(&service, task.task_id).await;
    assert_eq!(
        service
            .resume_identified(task.task_id, run, default_engine_paths(None), None)
            .await
            .unwrap_err()
            .code,
        "IO_ERROR"
    );
    std::fs::write(project.join("state.json"), b"not valid json").unwrap();
    assert_eq!(
        service
            .continue_identified(task.task_id, run, default_engine_paths(None), None)
            .await
            .unwrap_err()
            .code,
        "INVALID_DATA"
    );
    let state = crate::project::PipelineStateFile::created(Quality::High);
    atomic_write_json(&project.join("state.json"), &state)
        .await
        .unwrap();
    // No completed stage or working images: the public executor decides to
    // prepare them, rather than rejecting a valid saved work order up front.
    *service.executor.lock().unwrap() = Some(Arc::new(|_, _| {
        Box::pin(async { Err(crate::error::SplatError::Cancelled) })
    }));
    let resumed = service
        .resume_identified(task.task_id, run, default_engine_paths(None), None)
        .await
        .unwrap();
    assert!(resumed.accepted);
    assert_ne!(resumed.run_id, Some(run));
    let stopped = service.wait(task.task_id).await.unwrap();
    assert_eq!(stopped.runs.last().unwrap().kind, "continue");
    assert_eq!(
        stopped.runs.last().unwrap().continued_from_status,
        Some(TaskStatus::Cancelled)
    );
    assert_eq!(
        service
            .resume_identified(task.task_id, run, default_engine_paths(None), None)
            .await
            .unwrap_err()
            .code,
        "STALE_RUN_ID"
    );
}

#[tokio::test]
async fn cancelled_reshoot_can_resume_from_verified_snapshot_before_work_frames_exist() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("clip.mp4");
    std::fs::write(&input, b"fixture").unwrap();
    let service = TaskService::at(root.path().join("registry"));
    let task = service
        .create(
            None,
            input,
            Quality::High,
            root.path().join("projects"),
            true,
            "mcp",
            None,
            None,
        )
        .await
        .unwrap();
    let source_project = Uuid::new_v4();
    let project = root.path().join("snapshot-checkpoint");
    let source = root.path().join("snapshot-source");
    let model = source.join("sparse");
    std::fs::create_dir_all(&model).unwrap();
    std::fs::write(
        model.join("cameras.txt"),
        "1 PINHOLE 1000 500 1000 1000 500 250\n",
    )
    .unwrap();
    std::fs::write(model.join("images.txt"), "1 1 0 0 0 0 0 0 1 frame.jpg\n\n").unwrap();
    std::fs::write(model.join("points3D.txt"), "").unwrap();
    let image = source.join("frame.jpg");
    std::fs::write(&image, b"immutable image fixture").unwrap();
    let snapshot_root = project.join("work/reshoot-snapshot-v3");
    let snapshot = crate::pipeline::reshoot_snapshot::ReshootSnapshotBuilder::new(&snapshot_root)
        .source_project_id(source_project)
        .model(&model)
        .image(&image, "frame.jpg")
        .build(&tokio_util::sync::CancellationToken::new())
        .await
        .unwrap();
    let mut state = crate::project::PipelineStateFile::created(Quality::High);
    state.inputs = Some(crate::pipeline::inputs::InputCheckpoint::new(
        crate::pipeline::inputs::GenerationInputs::Materials { items: vec![] },
    ));
    state.reshoot = Some(crate::project::ReshootState {
        snapshot_v3_identity: Some(snapshot.content_identity),
        snapshot_v3_ready: true,
        ..Default::default()
    });
    atomic_write_json(&project.join("state.json"), &state)
        .await
        .unwrap();
    let run = Uuid::new_v4();
    {
        let mut records = service.records();
        let saved = records.tasks.get_mut(&task.task_id).unwrap();
        saved.project_id = Some(Uuid::new_v4());
        saved.project_path = Some(project);
        saved.source_project_id = Some(source_project);
        saved.run_id = Some(run);
        saved.status = TaskStatus::Cancelled;
    }
    persist_bound_project_metadata(&service, task.task_id).await;
    *service.executor.lock().unwrap() = Some(Arc::new(|_, _| {
        Box::pin(async { Err(crate::error::SplatError::Cancelled) })
    }));

    let receipt = service
        .resume_identified(task.task_id, run, default_engine_paths(None), None)
        .await
        .unwrap();
    assert!(receipt.accepted);
    service.wait(task.task_id).await.unwrap();
}

#[tokio::test]
async fn gui_project_retry_requires_the_current_failed_run() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("clip.mp4");
    std::fs::write(&input, b"fixture").unwrap();
    let service = TaskService::at(root.path().join("registry"));
    let task = service
        .create(
            None,
            input,
            Quality::High,
            root.path().join("projects"),
            true,
            "gui",
            None,
            None,
        )
        .await
        .unwrap();
    let project = Uuid::new_v4();
    let project_root = root.path().join("project");
    std::fs::create_dir_all(&project_root).unwrap();
    atomic_write_json(
        &project_root.join("state.json"),
        &crate::project::PipelineStateFile::created(Quality::High),
    )
    .await
    .unwrap();
    let failed_run = Uuid::new_v4();
    {
        let mut records = service.records();
        let saved = records.tasks.get_mut(&task.task_id).unwrap();
        saved.project_id = Some(project);
        saved.project_path = Some(project_root);
        saved.run_id = Some(failed_run);
        saved.status = TaskStatus::Failed;
    }
    persist_bound_project_metadata(&service, task.task_id).await;
    assert_eq!(
        service
            .resume_project_identified(project, Uuid::new_v4(), default_engine_paths(None), None,)
            .await
            .unwrap_err()
            .code,
        "STALE_RUN_ID"
    );
    *service.executor.lock().unwrap() = Some(Arc::new(|_, _| {
        Box::pin(async { Err(crate::error::SplatError::Cancelled) })
    }));
    let retried = service
        .resume_project_identified(project, failed_run, default_engine_paths(None), None)
        .await
        .unwrap();
    assert!(retried.accepted);
    assert_ne!(retried.run_id, Some(failed_run));
    service.wait(task.task_id).await.unwrap();
}

#[test]
fn generation_defaults_are_not_retry_consent_and_invalid_ratios_are_rejected() {
    let defaults = generation_parameters(None).unwrap();
    assert_eq!(defaults["generationPolicy"]["autoRetryEnabled"], true);
    assert_eq!(
        defaults["generationPolicy"]["version"],
        crate::pipeline::generation_policy::VERSION
    );
    let migrated = generation_parameters(Some(serde_json::json!({"generationPolicy":{"version":1,"autoRetryEnabled":false,"frameSamplingRatio":0.5}}))).unwrap();
    assert_eq!(
        migrated["generationPolicy"]["version"],
        crate::pipeline::generation_policy::VERSION
    );
    assert_eq!(migrated["generationPolicy"]["autoRetryEnabled"], false);
    let off=generation_parameters(Some(serde_json::json!({"generationPolicy":{"autoRetryEnabled":false,"frameSamplingRatio":0.333},"reshootOptions":{"allowFallback":true}}))).unwrap();
    assert_eq!(off["generationPolicy"]["frameSamplingRatio"], 0.333);
    assert_eq!(off["reshootOptions"]["allowFallback"], false);
    // Explicit old API refusal is not silently widened by a new default.
    let legacy = generation_parameters(Some(
        serde_json::json!({"reshootOptions":{"allowFallback":false}}),
    ))
    .unwrap();
    assert_eq!(legacy["generationPolicy"]["autoRetryEnabled"], false);
    for ratio in [0., -1., 1.1] {
        assert!(generation_parameters(Some(
            serde_json::json!({"generationPolicy":{"frameSamplingRatio":ratio}})
        ))
        .is_err());
    }
    assert!(generation_parameters(Some(serde_json::json!("invalid"))).is_err());
}

#[tokio::test]
async fn manual_retry_before_project_creation_starts_a_new_run_without_changing_consent() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("input.mp4");
    std::fs::write(&input, b"fixture").unwrap();
    let service = TaskService::at(root.path().join("registry"));
    let task=service.create(None,input,Quality::Fast,root.path().join("projects"),false,"gui",None,Some(serde_json::json!({"generationPolicy":{"autoRetryEnabled":false,"frameSamplingRatio":0.5}}))).await.unwrap();
    let preserved_policy = crate::pipeline::generation_policy::GenerationPolicy {
        version: 5,
        auto_retry_enabled: false,
        frame_sampling_ratio: Some(0.5),
        frame_sampling_fps: None,
    };
    {
        let mut registry = service.records();
        let saved = registry.tasks.get_mut(&task.task_id).unwrap();
        saved.generation_policy = Some(preserved_policy);
        saved.request_parameters.as_mut().unwrap()["generationPolicy"] =
            serde_json::to_value(preserved_policy).unwrap();
        saved.configuration.shared.minimum_selected_frames = 987;
    }
    *service.executor.lock().unwrap() = Some(Arc::new(|task, _| {
        Box::pin(async move {
            assert!(task.planner_enabled);
            let policy = task.generation_policy.unwrap();
            assert_eq!(policy.version, 5);
            assert!(!policy.auto_retry_enabled);
            assert_eq!(task.configuration.shared.minimum_selected_frames, 987);
            Err(crate::error::SplatError::Process(
                "fixture fails before project creation".into(),
            ))
        })
    }));
    let first = service
        .start(task.task_id, default_engine_paths(None), None)
        .await
        .unwrap();
    service.wait(task.task_id).await.unwrap();
    let second = service
        .retry_failed(
            task.task_id,
            first.run_id.unwrap(),
            default_engine_paths(None),
            None,
        )
        .await
        .unwrap();
    assert!(second.accepted);
    assert_ne!(first.run_id, second.run_id);
    let finished = service.wait(task.task_id).await.unwrap();
    assert_eq!(finished.runs.len(), 2);
    assert_eq!(finished.runs[1].kind, "continue");
    assert_eq!(
        finished.runs[1].continued_from_status,
        Some(TaskStatus::Failed)
    );
    assert_eq!(
        finished.generation_policy.unwrap().frame_sampling_ratio,
        Some(0.5)
    );
    assert_eq!(finished.generation_policy.unwrap().version, 5);
    assert_eq!(finished.configuration.shared.minimum_selected_frames, 987);
    assert!(!finished.automatically_retried);
    assert_eq!(
        service
            .retry_failed(
                task.task_id,
                first.run_id.unwrap(),
                default_engine_paths(None),
                None
            )
            .await
            .unwrap_err()
            .code,
        "STALE_RUN_ID"
    );
}

#[tokio::test]
async fn failed_retry_uses_the_saved_v5_checkpoint_policy_and_configuration() {
    let (root, service, task) = fixture().await;
    let project_id = Uuid::new_v4();
    let project = root.path().join("saved-project");
    std::fs::create_dir_all(&project).unwrap();
    let policy = crate::pipeline::generation_policy::GenerationPolicy {
        version: 5,
        auto_retry_enabled: false,
        frame_sampling_ratio: Some(0.4),
        frame_sampling_fps: None,
    };
    let video = crate::video::VideoInfo {
        duration: 20.0,
        fps: 30.0,
        total_frames: 600,
        width: 1920,
        height: 1080,
        codec: "h264".into(),
        rotation: 0,
        pixel_format: "yuv420p".into(),
        has_alpha: false,
    };
    let mut configuration = (*crate::presets::pipeline_optimization_config()).clone();
    configuration.shared.minimum_selected_frames = 432;
    let mut state = crate::project::PipelineStateFile::created(Quality::Balanced);
    state.generation_policy = Some(policy);
    state.configuration = Some(configuration.clone());
    state.planner_enabled = true;
    state.video = Some(video.clone());
    state.frames = Some(crate::project::FrameState::from(
        &policy.frame_plan(&video, Quality::Balanced),
    ));
    std::fs::write(
        project.join("state.json"),
        serde_json::to_vec_pretty(&state).unwrap(),
    )
    .unwrap();
    let failed_run = Uuid::new_v4();
    {
        let mut registry = service.records();
        let saved = registry.tasks.get_mut(&task.task_id).unwrap();
        saved.project_id = Some(project_id);
        saved.project_path = Some(project);
        saved.status = TaskStatus::Failed;
        saved.run_id = Some(failed_run);
        saved.generation_policy = Some(Default::default());
        saved.request_parameters.as_mut().unwrap()["generationPolicy"] =
            serde_json::to_value(crate::pipeline::generation_policy::GenerationPolicy::default())
                .unwrap();
    }
    persist_bound_project_metadata(&service, task.task_id).await;
    *service.executor.lock().unwrap() = Some(Arc::new(move |task, _| {
        Box::pin(async move {
            let actual = task.generation_policy.unwrap();
            assert_eq!(actual, policy);
            assert_eq!(task.configuration.shared.minimum_selected_frames, 432);
            assert_eq!(
                task.request_parameters.unwrap()["generationPolicy"]["version"],
                5
            );
            Err(crate::error::SplatError::Process("fixture".into()))
        })
    }));
    service
        .retry_failed(task.task_id, failed_run, default_engine_paths(None), None)
        .await
        .unwrap();
    let finished = service.wait(task.task_id).await.unwrap();
    assert_eq!(finished.generation_policy, Some(policy));
    assert_eq!(finished.configuration.shared.minimum_selected_frames, 432);
}

#[tokio::test]
async fn all_stop_reasons_continue_with_saved_actual_recipe_and_spent_allowances() {
    for status in [
        TaskStatus::Failed,
        TaskStatus::Cancelled,
        TaskStatus::Interrupted,
    ] {
        let (root, service, task) = fixture().await;
        let previous_run = notification_run(&service, task.task_id);
        let project = root.path().join("saved-work-order");
        std::fs::create_dir_all(&project).unwrap();
        let policy = crate::pipeline::generation_policy::GenerationPolicy {
            version: 5,
            auto_retry_enabled: true,
            frame_sampling_ratio: Some(0.4),
            frame_sampling_fps: None,
        };
        let initial = crate::presets::resolve_brush_training_preset(
            Quality::High,
            true,
            Some(8192),
            1920,
            1000,
        );
        let mut actual = initial;
        actual.preset.max_splats = Some(800_000);
        let mut state = crate::project::PipelineStateFile::created(Quality::High);
        state.generation_policy = Some(policy);
        state.configuration = Some(task.configuration.clone());
        state
            .configuration
            .as_mut()
            .unwrap()
            .shared
            .minimum_selected_frames = 432;
        state.planner_enabled = true;
        state.brush_training.initial_resolved = Some(initial);
        state.brush_training.resolved = Some(actual);
        state.brush_training.oom_retry_used = true;
        state.brush_training.oom_retry_started = true;
        state.brush_training.oom_retry_source_count = Some(1_000_000);
        atomic_write_json(&project.join("state.json"), &state)
            .await
            .unwrap();
        let before = std::fs::read(project.join("state.json")).unwrap();
        {
            let mut records = service.records();
            let saved = records.tasks.get_mut(&task.task_id).unwrap();
            saved.status = status;
            saved.project_id = Some(Uuid::new_v4());
            saved.project_path = Some(project.clone());
            // A stale task-level recipe must not override the saved work order.
            saved.generation_policy = Some(Default::default());
            saved.error = Some(TaskError {
                code: "pipeline_failed".into(),
                message: "previous stop evidence".into(),
                failed_stage: Some(PipelineStage::TrainingSplats),
                engine: None,
                exit_code: None,
                classification: None,
                classification_is_heuristic: false,
                classification_version: None,
                termination: None,
                failure_id: None,
                log_sources: vec![],
            });
        }
        persist_bound_project_metadata(&service, task.task_id).await;
        let observed_path = project.join("state.json");
        let expected_bytes = before.clone();
        *service.executor.lock().unwrap() = Some(Arc::new(move |task, _| {
            let path = observed_path.clone();
            let bytes = expected_bytes.clone();
            Box::pin(async move {
                assert_eq!(task.generation_policy, Some(policy));
                assert_eq!(task.configuration.shared.minimum_selected_frames, 432);
                assert!(task.automatically_retried);
                let checkpoint: crate::project::metadata::BrushTrainingCheckpoint =
                    serde_json::from_value(
                        task.actual_configuration.as_ref().unwrap()["brush_training"].clone(),
                    )
                    .unwrap();
                assert_eq!(checkpoint.initial_resolved, Some(initial));
                assert_eq!(checkpoint.resolved, Some(actual));
                assert!(checkpoint.oom_retry_used && checkpoint.oom_retry_started);
                assert_eq!(checkpoint.oom_retry_source_count, Some(1_000_000));
                // Admission never mutates a saved work order or clears spent OOM state.
                assert_eq!(std::fs::read(path).unwrap(), bytes);
                Err(crate::error::SplatError::Cancelled)
            })
        }));
        let receipt = match status {
            TaskStatus::Failed => {
                service
                    .resume_identified(task.task_id, previous_run, default_engine_paths(None), None)
                    .await
            }
            TaskStatus::Cancelled => {
                service
                    .retry_failed(task.task_id, previous_run, default_engine_paths(None), None)
                    .await
            }
            _ => {
                service
                    .continue_identified(
                        task.task_id,
                        previous_run,
                        default_engine_paths(None),
                        None,
                    )
                    .await
            }
        }
        .unwrap();
        assert!(receipt.accepted);
        let finished = service.wait(task.task_id).await.unwrap();
        assert_eq!(finished.status, TaskStatus::Cancelled);
        assert!(finished.automatically_retried);
        assert_eq!(
            finished.runs[0].error.as_ref().unwrap().message,
            "previous stop evidence"
        );
        assert_eq!(finished.runs[1].kind, "continue");
        assert_eq!(finished.runs[1].continued_from_status, Some(status));
        assert_eq!(std::fs::read(project.join("state.json")).unwrap(), before);
        let restored = TaskService::at(root.path().join("registry"))
            .get(task.task_id)
            .await
            .unwrap();
        assert!(restored.automatically_retried);
        assert_eq!(restored.runs[1].continued_from_status, Some(status));
    }
}

#[tokio::test]
async fn stopped_tasks_without_a_project_continue_the_original_request_even_without_work() {
    for status in [
        TaskStatus::Failed,
        TaskStatus::Cancelled,
        TaskStatus::Interrupted,
    ] {
        let (_root, service, task) = fixture().await;
        let previous_run = notification_run(&service, task.task_id);
        let policy = crate::pipeline::generation_policy::GenerationPolicy {
            version: 5,
            auto_retry_enabled: false,
            frame_sampling_ratio: Some(0.37),
            frame_sampling_fps: None,
        };
        {
            let mut records = service.records();
            let saved = records.tasks.get_mut(&task.task_id).unwrap();
            saved.status = status;
            saved.generation_policy = Some(policy);
            saved.request_parameters.as_mut().unwrap()["generationPolicy"] =
                serde_json::to_value(policy).unwrap();
            saved.configuration.shared.minimum_selected_frames = 687;
        }
        *service.executor.lock().unwrap() = Some(Arc::new(move |task, _| {
            Box::pin(async move {
                assert_eq!(task.generation_policy, Some(policy));
                assert_eq!(task.configuration.shared.minimum_selected_frames, 687);
                assert!(task.project_id.is_none());
                Err(crate::error::SplatError::Cancelled)
            })
        }));
        service
            .continue_identified(task.task_id, previous_run, default_engine_paths(None), None)
            .await
            .unwrap();
        let finished = service.wait(task.task_id).await.unwrap();
        assert_eq!(finished.status, TaskStatus::Cancelled);
        assert_eq!(finished.runs[1].kind, "continue");
        assert_eq!(finished.runs[1].continued_from_status, Some(status));
        assert_eq!(finished.generation_policy, Some(policy));
        assert!(!finished.automatically_retried);
    }
}

#[tokio::test]
async fn continuation_does_not_restart_created_active_or_completed_tasks() {
    let (_root, service, task) = fixture().await;
    let run = Uuid::new_v4();
    for status in [
        TaskStatus::Created,
        TaskStatus::Starting,
        TaskStatus::Running,
        TaskStatus::Cancelling,
        TaskStatus::Completed,
    ] {
        {
            let mut records = service.records();
            let saved = records.tasks.get_mut(&task.task_id).unwrap();
            saved.run_id = Some(run);
            saved.status = status;
        }
        let error = service
            .continue_identified(task.task_id, run, default_engine_paths(None), None)
            .await
            .unwrap_err();
        assert_eq!(error.code, "TASK_NOT_RESUMABLE");
        let unchanged = service.get(task.task_id).await.unwrap();
        assert_eq!(unchanged.status, status);
        assert_eq!(unchanged.run_id, Some(run));
        assert!(unchanged.runs.is_empty());
    }
}
use crate::pipeline::runner::default_engine_paths;

type RecordedUpdates = Arc<SyncMutex<Vec<TaskUpdate>>>;

#[test]
fn legacy_task_errors_are_not_reclassified_or_rewritten() {
    let legacy = serde_json::json!({
        "code": "pipeline_failed", "message": "Brush 显存不足：wgpu error: Out of Memory\nAn IO error happened",
        "failed_stage": "trainingSplats", "engine": "brush", "exit_code": 101,
        "classification": "brush_dataset", "classification_is_heuristic": true,
        "failure_id": null, "log_sources": ["brush"]
    });
    let error: TaskError = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(error.classification_version, None);
    assert_eq!(error.termination, None);
    assert_eq!(serde_json::to_value(error).unwrap(), legacy);
}

#[tokio::test]
async fn current_failure_status_and_classification_survive_task_restart() {
    let (root, service, task) = fixture().await;
    *service.executor.lock().unwrap() = Some(Arc::new(move |_, _| {
        Box::pin(async {
            let output = crate::process::ProcessOutput {
                success: false,
                exit_code: Some(-1073740940),
                termination: crate::process::ProcessTermination {
                    exit_code: Some(-1073740940),
                    windows_status: Some("0xC0000374".into()),
                    ..Default::default()
                },
                stdout: "435 (connected 435, loaded 435)\nRegistering image #235".into(),
                stderr: "Could not register\nDiscarding reconstruction because it is too small"
                    .into(),
            };
            Err(crate::error::SplatError::EngineProcess(Box::new(
                crate::failure::EngineProcessFailure::from_output(
                    crate::pipeline::PipelineEngine::Colmap,
                    "mapper",
                    &output,
                ),
            )))
        })
    }));
    service
        .start_inner(task.task_id, default_engine_paths(None), None, false, None)
        .await
        .unwrap();
    let finished = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        service.wait(task.task_id),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(finished.status, TaskStatus::Failed);
    let error = finished.error.unwrap();
    assert_eq!(error.classification.as_deref(), Some("colmap_crash"));
    assert_eq!(error.classification_version, Some(1));
    assert!(!error.classification_is_heuristic);
    assert_eq!(error.exit_code, Some(-1073740940));
    assert_eq!(
        error
            .termination
            .as_ref()
            .unwrap()
            .windows_status
            .as_deref(),
        Some("0xC0000374")
    );
    let reloaded = TaskService::at(root.path().join("registry"))
        .get(task.task_id)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(reloaded.error.unwrap()).unwrap(),
        serde_json::to_value(error).unwrap()
    );
}

fn record_notifications(service: &TaskService) -> RecordedUpdates {
    let updates: RecordedUpdates = Default::default();
    let observed = updates.clone();
    *service.emitter.lock().unwrap() = Some(Arc::new(move |update| {
        observed.lock().unwrap().push(update);
    }));
    updates
}

fn notification_run(service: &TaskService, id: Uuid) -> Uuid {
    let run = Uuid::new_v4();
    let mut registry = service.records();
    let task = registry.tasks.get_mut(&id).unwrap();
    task.status = TaskStatus::Running;
    task.stage = Some(PipelineStage::Reconstructing);
    task.run_id = Some(run);
    task.sequence = 0;
    task.runs.push(TaskRun {
        run_id: run,
        kind: "generation".into(),
        continued_from_status: None,
        started_at: Utc::now(),
        ended_at: None,
        logs: Default::default(),
        last_engine: None,
        last_exit_code: None,
        error: None,
        brush_executions: Vec::new(),
        current_brush_execution: None,
    });
    run
}

fn notification_event(sequence: u64, kind: crate::pipeline::EventKind) -> PipelineEvent {
    let mut event = PipelineEvent::mapped(
        PipelineStage::Reconstructing,
        0.5,
        format!("line {sequence}"),
    );
    event.kind = kind;
    event.sequence = sequence;
    event.elapsed_ms = sequence;
    event
}

#[tokio::test]
async fn brush_executions_survive_runtime_clear_and_are_isolated_per_attempt_and_run() {
    use crate::pipeline::{
        event::PipelineOperation, runtime::RuntimeTracker, EventKind, PipelineEngine,
    };
    let (_root, service, task) = fixture().await;
    let run = notification_run(&service, task.task_id);
    let initial =
        crate::presets::resolve_brush_training_preset(Quality::High, true, Some(8192), 3840, 1000);
    let mut checkpoint = crate::project::PipelineStateFile::created(Quality::High);
    checkpoint.brush_training.initial_resolved = Some(initial);
    checkpoint.brush_training.resolved = Some(initial);
    service.on_runner_update(
        task.task_id,
        run,
        RunnerUpdate::Configuration(checkpoint_snapshot(&checkpoint)),
    );
    let mut event = PipelineEvent::mapped(PipelineStage::TrainingSplats, 0.0, "start");
    event.sequence = 1;
    event.engine = Some(PipelineEngine::Brush);
    event.operation = Some(PipelineOperation {
        id: 1,
        attempt: 1,
        source_id: None,
    });
    service.on_event(task.task_id, run, event.clone());
    let mut runtime = RuntimeTracker::new();
    runtime.snapshot.process_id = 100;
    event.sequence = 2;
    event.kind = EventKind::Runtime;
    event.runtime = Some(runtime.snapshot.clone());
    service.on_event(task.task_id, run, event.clone());
    runtime.line("GPU initialization completed: backend=Vulkan device=NVIDIA GPU type=DiscreteGpu driver=NVIDIA driver_info=616.92", std::time::Instant::now(), true);
    event.sequence = 3;
    event.runtime = Some(runtime.snapshot.clone());
    service.on_event(task.task_id, run, event.clone());
    service.on_runner_update(
        task.task_id,
        run,
        RunnerUpdate::ProcessExited {
            engine: PipelineEngine::Brush,
            exit_code: Some(101),
        },
    );
    service.on_runner_update(task.task_id, run, RunnerUpdate::BrushPreparation);
    let first = service.get(task.task_id).await.unwrap();
    assert_eq!(first.runs[0].current_brush_execution, None);
    assert_eq!(
        first.runs[0].brush_executions[0]
            .device
            .as_ref()
            .unwrap()
            .name,
        "NVIDIA GPU"
    );
    assert_eq!(first.runs[0].brush_executions[0].exit_code, Some(101));
    let mut retry = initial;
    retry.preset.max_splats = Some(800_000);
    checkpoint.brush_training.resolved = Some(retry);
    checkpoint.brush_training.oom_retry_used = true;
    checkpoint.brush_training.oom_retry_started = true;
    service.on_runner_update(
        task.task_id,
        run,
        RunnerUpdate::Configuration(checkpoint_snapshot(&checkpoint)),
    );
    assert!(
        !service
            .get(task.task_id)
            .await
            .unwrap()
            .automatically_retried
    );
    service.on_runner_update(task.task_id, run, RunnerUpdate::AutomaticRetry);
    assert!(
        service
            .get(task.task_id)
            .await
            .unwrap()
            .automatically_retried
    );
    event.sequence = 4;
    event.kind = EventKind::Stage;
    event.operation = Some(PipelineOperation {
        id: 2,
        attempt: 2,
        source_id: None,
    });
    event.runtime = None;
    service.on_event(task.task_id, run, event.clone());
    let mut second = RuntimeTracker::new();
    second.snapshot.process_id = 101;
    event.sequence = 5;
    event.kind = EventKind::Runtime;
    event.runtime = Some(second.snapshot);
    service.on_event(task.task_id, run, event.clone());
    // A late first-attempt sample and a foreign run must not rewrite either record.
    event.sequence = 6;
    event.operation = Some(PipelineOperation {
        id: 1,
        attempt: 1,
        source_id: None,
    });
    event.runtime = Some(runtime.snapshot);
    service.on_event(task.task_id, run, event.clone());
    service.on_event(task.task_id, Uuid::new_v4(), event);
    let mut exporting = PipelineEvent::mapped(PipelineStage::Exporting, 0.0, "publish");
    exporting.sequence = 7;
    exporting.operation = Some(PipelineOperation {
        id: 3,
        attempt: 0,
        source_id: None,
    });
    service.on_event(task.task_id, run, exporting);
    let saved = service.get(task.task_id).await.unwrap();
    assert!(saved.runtime.is_none());
    assert_eq!(saved.runs[0].brush_executions.len(), 2);
    assert!(saved.runs[0].brush_executions[1].device.is_none());
    assert_eq!(saved.runs[0].current_brush_execution, Some(1));
    assert_eq!(
        saved.runs[0].brush_executions[0].configuration,
        Some(initial)
    );
    assert_eq!(saved.runs[0].brush_executions[1].configuration, Some(retry));
    service.persist().await.unwrap();
    let restored = TaskService::at(service.root.clone())
        .get(task.task_id)
        .await
        .unwrap();
    assert_eq!(restored.runs[0].brush_executions.len(), 2);
    let mut old = serde_json::to_value(&saved.runs[0]).unwrap();
    old.as_object_mut().unwrap().remove("brush_executions");
    old.as_object_mut()
        .unwrap()
        .remove("current_brush_execution");
    let old: TaskRun = serde_json::from_value(old).unwrap();
    assert!(old.brush_executions.is_empty());

    // Cancellation has no ProcessExited callback, but must still freeze this attempt.
    let runner = Arc::new(PipelineRunner::new(default_engine_paths(None), |_| {}));
    service
        .finish(
            task.task_id,
            run,
            runner,
            Err(crate::error::SplatError::Cancelled),
        )
        .await;
    let cancelled = service.get(task.task_id).await.unwrap();
    assert_eq!(cancelled.status, TaskStatus::Cancelled);
    assert!(cancelled.runs[0]
        .brush_executions
        .iter()
        .all(|v| v.ended_at.is_some()));
    assert_eq!(cancelled.runs[0].brush_executions[0].exit_code, Some(101));
    assert_eq!(cancelled.runs[0].brush_executions[1].exit_code, None);
    assert!(cancelled.error.unwrap().failure_id.is_none());
}

#[tokio::test]
async fn panorama_substage_change_clears_face_counts_runtime_and_eta() {
    let (_root, service, task) = fixture().await;
    let run = notification_run(&service, task.task_id);
    let mut face = PipelineEvent::mapped(PipelineStage::TrainingSplats, 0.95, "faces");
    face.sequence = 1;
    face.substage = Some("panoramaTrainingPreparation".into());
    face.current = Some(228);
    face.total = Some(228);
    face.unit = Some("panoramaFaces".into());
    service.on_event(task.task_id, run, face);
    let prepared = service.get(task.task_id).await.unwrap();
    assert_eq!(prepared.progress, Some(95.0));
    assert_eq!(prepared.current, Some(228));
    let mut validation = PipelineEvent::mapped(PipelineStage::TrainingSplats, 0.95, "validate");
    validation.sequence = 2;
    validation.substage = Some("panoramaTrainingValidation".into());
    validation.indeterminate = true;
    service.on_event(task.task_id, run, validation);
    let validating = service.get(task.task_id).await.unwrap();
    assert!(validating.current.is_none() && validating.progress.is_none());
    let mut train = PipelineEvent::mapped(PipelineStage::TrainingSplats, 0.0, "training");
    train.sequence = 3;
    train.substage = Some("training".into());
    train.current = Some(0);
    train.total = Some(8000);
    train.unit = Some("iterations".into());
    service.on_event(task.task_id, run, train);
    let training = service.get(task.task_id).await.unwrap();
    assert_eq!(training.total, Some(8000));
    assert_eq!(training.current, Some(0));
    assert!(training.runtime.is_none() && training.eta_seconds.is_none());
}

#[tokio::test]
async fn feature_group_start_keeps_credible_percent_but_never_reuses_previous_process_resources() {
    use crate::pipeline::{event::PipelineOperation, EventKind};
    let (_root, service, task) = fixture().await;
    let run = notification_run(&service, task.task_id);
    let mut start = PipelineEvent::mapped(PipelineStage::ExtractingFeatures, 0.5, "features");
    start.sequence = 1;
    start.operation = Some(PipelineOperation {
        id: 1,
        attempt: 1,
        source_id: Some("photos".into()),
    });
    start.current = Some(2);
    start.total = Some(4);
    start.unit = Some("images".into());
    service.on_event(task.task_id, run, start.clone());
    let initial_overall = service.get(task.task_id).await.unwrap().estimated_progress;
    let snapshot: crate::pipeline::runtime::RuntimeSnapshot =
        serde_json::from_value(serde_json::json!({
            "processId": 42, "phase": "running", "updatedAt": Utc::now(), "lastOutputAgeMs": 0,
            "training": null, "device": null, "backend": null, "config": {}, "resources": null
        }))
        .unwrap();
    for (offset, kind) in [
        EventKind::Runtime,
        EventKind::Log,
        EventKind::Heartbeat,
        EventKind::Progress,
    ]
    .into_iter()
    .enumerate()
    {
        let mut empty = start.clone();
        empty.sequence = offset as u64 + 2;
        empty.kind = kind;
        empty.current = None;
        empty.total = None;
        empty.unit = None;
        empty.stage_progress = None;
        empty.indeterminate = true;
        empty.progress = 99.9; // stale mapped values are not measurements
        empty.runtime = (kind == EventKind::Runtime).then(|| snapshot.clone());
        service.on_event(task.task_id, run, empty);
        let observed = service.get(task.task_id).await.unwrap();
        assert_eq!(observed.progress, Some(50.0));
        assert_eq!((observed.current, observed.total), (Some(2), Some(4)));
        assert_eq!(observed.unit.as_deref(), Some("images"));
        assert_eq!(observed.estimated_progress, initial_overall);
        assert_eq!(observed.runtime.unwrap().process_id, 42);
    }
    let mut foreign_source = start.clone();
    foreign_source.sequence = 6;
    foreign_source.kind = EventKind::Progress;
    foreign_source.operation.as_mut().unwrap().source_id = Some("other-source".into());
    foreign_source.stage_progress = Some(99.0);
    service.on_event(task.task_id, run, foreign_source);
    let observed = service.get(task.task_id).await.unwrap();
    assert_eq!(observed.progress, Some(50.0));
    assert_eq!(observed.sequence, 5);
    let mut next = start.clone();
    next.sequence = 6;
    next.operation.as_mut().unwrap().id = 2;
    service.on_event(task.task_id, run, next.clone());
    let observed = service.get(task.task_id).await.unwrap();
    assert_eq!(observed.progress, Some(50.0));
    assert_eq!((observed.current, observed.total), (Some(2), Some(4)));
    assert!(observed.runtime.is_none()); // new PID has not supplied any sample
    let mut stale = start;
    stale.sequence = 7;
    stale.kind = EventKind::Runtime;
    stale.runtime = Some(snapshot);
    service.on_event(task.task_id, run, stale);
    assert!(service.get(task.task_id).await.unwrap().runtime.is_none());

    next.sequence = 8;
    next.operation.as_mut().unwrap().id = 3;
    next.operation.as_mut().unwrap().attempt = 2;
    next.current = None;
    next.total = None;
    next.unit = None;
    next.stage_progress = None;
    next.indeterminate = true;
    service.on_event(task.task_id, run, next);
    let retry = service.get(task.task_id).await.unwrap();
    assert!(retry.progress.is_none() && retry.current.is_none() && retry.total.is_none());
    assert_eq!(retry.operation.unwrap().attempt, 2);
}

#[tokio::test]
async fn empty_mapper_activity_keeps_its_estimate_but_changed_counter_units_do_not() {
    use crate::pipeline::{event::PipelineOperation, EventKind};
    let (_root, service, task) = fixture().await;
    let run = notification_run(&service, task.task_id);
    let mut registration = notification_event(1, EventKind::Stage);
    registration.operation = Some(PipelineOperation {
        id: 1,
        attempt: 1,
        source_id: None,
    });
    registration.current = Some(40);
    registration.total = Some(200);
    registration.unit = Some("registeredImages".into());
    registration.stage_progress = Some(18.0);
    registration.progress = 49.6;
    service.on_event(task.task_id, run, registration.clone());
    let mut activity = registration.clone();
    activity.sequence = 2;
    activity.kind = EventKind::Progress;
    activity.message = "Global bundle adjustment".into();
    activity.stage_progress = None;
    activity.indeterminate = true;
    activity.progress = 99.9;
    service.on_event(task.task_id, run, activity.clone());
    let adjusting = service.get(task.task_id).await.unwrap();
    assert_eq!(adjusting.progress, Some(18.0));
    assert_eq!(adjusting.estimated_progress, Some(49.6));
    assert_eq!((adjusting.current, adjusting.total), (Some(40), Some(200)));
    activity.sequence = 3;
    activity.unit = Some("unrelatedLocalBatch".into());
    activity.current = Some(1);
    activity.total = Some(3);
    service.on_event(task.task_id, run, activity);
    let local = service.get(task.task_id).await.unwrap();
    assert!(local.progress.is_none());
    assert_eq!((local.current, local.total), (Some(1), Some(3)));
    assert_eq!(local.estimated_progress, Some(49.6));
    registration.sequence = 4;
    registration.operation.as_mut().unwrap().id = 2;
    registration.substage = Some("differentPhase".into());
    registration.stage_progress = None;
    registration.current = None;
    registration.total = None;
    registration.unit = None;
    registration.indeterminate = true;
    registration.progress = 45.0;
    service.on_event(task.task_id, run, registration);
    let next = service.get(task.task_id).await.unwrap();
    assert!(next.progress.is_none() && next.current.is_none() && next.total.is_none());
    assert_eq!(next.estimated_progress, Some(49.6));
}

#[tokio::test]
async fn panorama_monitor_survives_non_lifecycle_events_and_rejects_old_operations() {
    use crate::pipeline::{event::PipelineOperation, EventKind};
    let (_root, service, task) = fixture().await;
    let run = notification_run(&service, task.task_id);
    let mut stage = notification_event(1, EventKind::Stage);
    stage.substage = Some("panoramaReconstruction".into());
    stage.operation = Some(PipelineOperation {
        id: 1,
        attempt: 1,
        source_id: None,
    });
    service.on_event(task.task_id, run, stage.clone());
    let snapshot: crate::pipeline::runtime::RuntimeSnapshot =
        serde_json::from_value(serde_json::json!({
            "processId": 42, "phase": "running", "updatedAt": Utc::now(), "lastOutputAgeMs": 0,
            "training": null, "device": null, "backend": null, "config": {}, "resources": null
        }))
        .unwrap();
    for sequence in 2..12 {
        let mut event = stage.clone();
        event.sequence = sequence;
        event.kind = if sequence % 2 == 0 {
            EventKind::Runtime
        } else {
            EventKind::Log
        };
        event.substage = None; // Legacy producer omitted this; it is not a lifecycle change.
        if event.kind == EventKind::Runtime {
            event.runtime = Some(snapshot.clone());
        }
        service.on_event(task.task_id, run, event);
        let current = service.get(task.task_id).await.unwrap();
        assert_eq!(current.substage.as_deref(), Some("panoramaReconstruction"));
        assert_eq!(current.runtime.unwrap().process_id, 42);
    }
    let mut retry = stage.clone();
    retry.sequence = 12;
    retry.operation = Some(PipelineOperation {
        id: 2,
        attempt: 2,
        source_id: None,
    });
    retry.stage_progress = None;
    retry.indeterminate = true;
    service.on_event(task.task_id, run, retry.clone());
    let mut stale = stage;
    stale.sequence = 13;
    stale.kind = EventKind::Runtime;
    stale.runtime = Some(snapshot);
    service.on_event(task.task_id, run, stale);
    let current = service.get(task.task_id).await.unwrap();
    assert!(current.runtime.is_none());
    assert_eq!(current.operation.unwrap().attempt, 2);
    let mut local_count = retry;
    local_count.sequence = 14;
    local_count.kind = EventKind::Progress;
    local_count.current = Some(80);
    local_count.total = Some(100);
    service.on_event(task.task_id, run, local_count);
    assert!(service.get(task.task_id).await.unwrap().progress.is_none());
}

#[tokio::test]
async fn notification_bursts_are_bounded_and_do_not_clone_full_records() {
    use notifications::{MAX_PENDING_LOGS, MAX_PENDING_LOG_BYTES};
    for lines in [1000, 5000] {
        let (root, service, task) = fixture().await;
        let updates = record_notifications(&service);
        let run = notification_run(&service, task.task_id);
        // Represent stdout persistence independently of the lossy UI transport.
        let log = root.path().join("colmap.log");
        let mut output = std::fs::File::create(&log).unwrap();
        use std::io::Write;
        for sequence in 1..=lines {
            let event = notification_event(sequence, crate::pipeline::EventKind::Log);
            writeln!(output, "{}", event.message).unwrap();
            service.on_event(task.task_id, run, event);
        }
        assert!(updates.lock().unwrap().is_empty());
        {
            let pending = service.notifications.lock().unwrap();
            let pending = &pending.tasks[&task.task_id];
            assert!(pending.log_count() <= MAX_PENDING_LOGS);
            assert!(pending.log_bytes <= MAX_PENDING_LOG_BYTES);
        }
        service.flush_notifications();
        let messages = updates.lock().unwrap();
        assert_eq!(messages.len(), 1);
        let update = &messages[0];
        assert_eq!(update.task.sequence, lines);
        assert_eq!(update.events.last().unwrap().sequence, lines);
        assert!(update.events.len() <= MAX_PENDING_LOGS);
        assert_eq!(
            update.dropped_event_count,
            lines - update.events.len() as u64
        );
        assert!(update
            .events
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence));
        let wire = serde_json::to_value(update).unwrap();
        for field in [
            "configuration",
            "actual_configuration",
            "request_parameters",
            "client_request_id",
            "elapsed_offset_ms",
        ] {
            assert!(
                wire["task"].get(field).is_none(),
                "unexpected heavyweight {field}"
            );
        }
        assert_eq!(wire["task"]["recent_events"], serde_json::json!([]));
        assert_eq!(wire["task"]["runs"].as_array().unwrap().len(), 1);
        assert!(wire["task"]["runs"][0].get("logs").is_none());
        assert_eq!(
            std::fs::read_to_string(log).unwrap().lines().count() as u64,
            lines
        );
        // The full query/persistence shape remains untouched.
        assert!(
            serde_json::to_value(&service.records().tasks[&task.task_id])
                .unwrap()
                .get("configuration")
                .is_some()
        );
    }
}

#[tokio::test]
async fn real_process_logs_remain_complete_when_ui_notifications_overflow() {
    use crate::process::{ProcessManager, ProcessSpec, ProcessUpdate};
    use std::sync::atomic::{AtomicU64, Ordering};

    for lines in [1000u64, 5000] {
        let (root, service, task) = fixture().await;
        let updates = record_notifications(&service);
        let run = notification_run(&service, task.task_id);
        let observed_service = service.clone();
        let task_id = task.task_id;
        let sequence = Arc::new(AtomicU64::new(0));
        let observed_sequence = sequence.clone();
        let observer = Arc::new(move |update| {
            if let ProcessUpdate::Line { line, .. } = update {
                let number = observed_sequence.fetch_add(1, Ordering::Relaxed) + 1;
                let mut event = notification_event(number, crate::pipeline::EventKind::Log);
                event.message = line;
                observed_service.on_event(task_id, run, event);
            }
        });
        #[cfg(windows)]
        let (executable, args) = (
            PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"))
                .join("System32/WindowsPowerShell/v1.0/powershell.exe"),
            // ProcessManager starts this fixed-output test process hidden.
            vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                format!("1..{lines} | ForEach-Object {{ Write-Output ('COLMAP-STRESS ' + $_) }}")
                    .into(),
            ],
        );
        #[cfg(unix)]
        let (executable, args) = (
            PathBuf::from("/bin/sh"),
            vec![
                "-c".into(),
                format!("i=1; while [ \"$i\" -le {lines} ]; do printf 'COLMAP-STRESS %s\\n' \"$i\"; i=$((i + 1)); done").into(),
            ],
        );
        let path = root.path().join("logs/colmap.log");
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            ProcessManager::new().run(ProcessSpec {
                executable,
                args,
                working_directory: None,
                log_path: Some(path.clone()),
                observer: Some(observer),
            }),
        )
        .await
        .expect("the synthetic output process must finish")
        .unwrap();
        assert!(output.success, "synthetic process failed: {output:?}");
        assert_eq!(sequence.load(Ordering::Relaxed), lines);
        assert_eq!(output.stdout.lines().count() as u64, lines);
        let persisted = std::fs::read_to_string(path).unwrap();
        let stdout_lines: Vec<_> = persisted
            .lines()
            .filter(|line| line.starts_with("COLMAP-STRESS "))
            .collect();
        assert_eq!(stdout_lines.len() as u64, lines);
        assert_eq!(stdout_lines.first().unwrap(), &"COLMAP-STRESS 1");
        assert_eq!(
            *stdout_lines.last().unwrap(),
            format!("COLMAP-STRESS {lines}")
        );
        assert!(updates.lock().unwrap().is_empty());
        service.flush_notifications();
        let messages = updates.lock().unwrap();
        assert_eq!(messages.len(), 1);
        assert!(messages[0].events.len() <= notifications::MAX_PENDING_LOGS);
        assert_eq!(
            messages[0].events.last().unwrap().message,
            format!("COLMAP-STRESS {lines}")
        );
        assert_eq!(
            messages[0].dropped_event_count,
            lines - messages[0].events.len() as u64
        );
    }
}

#[tokio::test]
async fn notification_byte_limit_counts_escaped_utf8_and_keeps_recent_logs() {
    let (_root, service, task) = fixture().await;
    let updates = record_notifications(&service);
    let run = notification_run(&service, task.task_id);
    for sequence in 1..=300 {
        let mut event = notification_event(sequence, crate::pipeline::EventKind::Log);
        event.message = "中文\\\"\n".repeat(500);
        service.on_event(task.task_id, run, event);
    }
    {
        let pending = service.notifications.lock().unwrap();
        let pending = &pending.tasks[&task.task_id];
        assert!(pending.log_bytes <= notifications::MAX_PENDING_LOG_BYTES);
        assert!(pending.log_count() < 200);
    }
    service.flush_notifications();
    let messages = updates.lock().unwrap();
    let update = &messages[0];
    assert_eq!(update.events.last().unwrap().sequence, 300);
    assert!(update.dropped_event_count > 0);
    assert!(
        update
            .events
            .iter()
            .map(|e| serde_json::to_vec(e).unwrap().len())
            .sum::<usize>()
            <= notifications::MAX_PENDING_LOG_BYTES
    );
}

#[tokio::test]
async fn progress_and_runtime_are_coalesced_and_phase_changes_flush_logs_immediately() {
    let (_root, service, task) = fixture().await;
    let updates = record_notifications(&service);
    let run = notification_run(&service, task.task_id);
    for sequence in 1..=1000 {
        service.on_event(
            task.task_id,
            run,
            notification_event(sequence * 2 - 1, crate::pipeline::EventKind::Progress),
        );
        service.on_event(
            task.task_id,
            run,
            notification_event(sequence * 2, crate::pipeline::EventKind::Runtime),
        );
    }
    service.flush_notifications();
    {
        let messages = updates.lock().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].events.len(), 2);
        assert_eq!(messages[0].events[0].sequence, 1999);
        assert_eq!(messages[0].events[1].sequence, 2000);
    }
    service.on_event(
        task.task_id,
        run,
        notification_event(2001, crate::pipeline::EventKind::Log),
    );
    let mut phase = PipelineEvent::mapped(PipelineStage::TrainingSplats, 0.0, "training starts");
    phase.sequence = 2002;
    service.on_event(task.task_id, run, phase);
    let messages = updates.lock().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].task.stage, Some(PipelineStage::TrainingSplats));
    assert_eq!(
        messages[1]
            .events
            .iter()
            .map(|e| e.sequence)
            .collect::<Vec<_>>(),
        vec![2001, 2002]
    );
}

#[tokio::test]
async fn notification_pump_skips_lifecycle_gate_and_preserves_queued_events() {
    let (_root, service, task) = fixture().await;
    let updates = record_notifications(&service);
    let run = notification_run(&service, task.task_id);
    let gate = service.gate.lock().await;
    let mut phase = PipelineEvent::mapped(PipelineStage::TrainingSplats, 0.0, "phase");
    phase.sequence = 1;
    service.on_event(task.task_id, run, phase);
    service.flush_notifications();
    assert!(updates.lock().unwrap().is_empty());
    assert!(service
        .notifications
        .lock()
        .unwrap()
        .tasks
        .contains_key(&task.task_id));
    drop(gate);
    service.flush_notifications();
    assert_eq!(updates.lock().unwrap().len(), 1);
    assert!(service.notifications.lock().unwrap().tasks.is_empty());
}

#[tokio::test]
async fn terminal_notification_flushes_last_logs_and_cannot_emit_old_running_state() {
    for terminal in [
        TaskStatus::Completed,
        TaskStatus::Failed,
        TaskStatus::Cancelled,
    ] {
        let (root, service, task) = fixture().await;
        let updates = record_notifications(&service);
        let run = notification_run(&service, task.task_id);
        service.on_event(
            task.task_id,
            run,
            notification_event(1, crate::pipeline::EventKind::Log),
        );
        let gate = service.gate.lock().await;
        {
            let mut registry = service.records();
            let task = registry.tasks.get_mut(&task.task_id).unwrap();
            task.status = terminal;
            task.revision += 1;
        }
        service.flush_notifications();
        assert!(updates.lock().unwrap().is_empty());
        service.persist().await.unwrap();
        service.notify(task.task_id);
        drop(gate);
        service.flush_notifications();
        service.on_event(
            task.task_id,
            run,
            notification_event(2, crate::pipeline::EventKind::Log),
        );
        let messages = updates.lock().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].task.status, terminal);
        assert_eq!(messages[0].events.len(), 1);
        let disk: Registry = serde_json::from_slice(
            &std::fs::read(root.path().join("registry/tasks.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(disk.tasks[&task.task_id].status, terminal);
        assert!(service.notifications.lock().unwrap().tasks.is_empty());
    }
}

#[tokio::test]
async fn a_new_run_clears_pending_old_logs_and_only_sends_its_current_kind() {
    let (_root, service, task) = fixture().await;
    let updates = record_notifications(&service);
    let old = notification_run(&service, task.task_id);
    service.on_event(
        task.task_id,
        old,
        notification_event(1, crate::pipeline::EventKind::Log),
    );
    let current = notification_run(&service, task.task_id);
    service
        .records()
        .tasks
        .get_mut(&task.task_id)
        .unwrap()
        .runs
        .last_mut()
        .unwrap()
        .kind = "resume".into();
    service.notify(task.task_id);
    service.on_event(
        task.task_id,
        old,
        notification_event(2, crate::pipeline::EventKind::Log),
    );
    service.on_event(
        task.task_id,
        current,
        notification_event(1, crate::pipeline::EventKind::Log),
    );
    service.flush_notifications();
    let messages = updates.lock().unwrap();
    assert_eq!(messages.len(), 2);
    assert!(messages[0].events.is_empty());
    assert_eq!(messages[0].task.runs[0].kind, "resume");
    assert_eq!(messages[0].task.runs.len(), 1);
    assert_eq!(messages[1].events.len(), 1);
    assert_eq!(messages[1].events[0].run_id, Some(current));
}

#[tokio::test]
async fn delayed_positive_sequence_cannot_regress_stage_runtime_or_progress() {
    let (_root, service, task) = fixture().await;
    let updates = record_notifications(&service);
    let run = notification_run(&service, task.task_id);
    let mut newer = notification_event(10, crate::pipeline::EventKind::Stage);
    newer.stage = PipelineStage::TrainingSplats;
    newer.current = Some(10);
    newer.total = Some(100);
    newer.stage_progress = Some(10.0);
    service.on_event(task.task_id, run, newer);
    let before = service.get(task.task_id).await.unwrap();
    for sequence in [9, 10] {
        service.on_event(
            task.task_id,
            run,
            notification_event(sequence, crate::pipeline::EventKind::Runtime),
        );
    }
    let after = service.get(task.task_id).await.unwrap();
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.stage, Some(PipelineStage::TrainingSplats));
    assert_eq!(after.sequence, 10);
    assert_eq!(after.progress, Some(10.0));
    service.flush_notifications();
    assert_eq!(updates.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn concurrent_flushes_and_callbacks_cannot_announce_running_after_terminal() {
    let (_root, service, task) = fixture().await;
    let updates = record_notifications(&service);
    let run = notification_run(&service, task.task_id);
    let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let background_service = service.clone();
    let background_stop = stopped.clone();
    let flusher = std::thread::spawn(move || {
        while !background_stop.load(std::sync::atomic::Ordering::Acquire) {
            background_service.flush_notifications();
            std::thread::yield_now();
        }
    });
    for sequence in 1..=1000 {
        service.on_event(
            task.task_id,
            run,
            notification_event(sequence, crate::pipeline::EventKind::Log),
        );
    }
    let gate = service.gate.lock().await;
    {
        let mut records = service.records();
        let task = records.tasks.get_mut(&task.task_id).unwrap();
        task.status = TaskStatus::Failed;
        task.revision += 1;
    }
    service.persist().await.unwrap();
    service.notify(task.task_id);
    drop(gate);
    for sequence in 1001..=2000 {
        service.on_event(
            task.task_id,
            run,
            notification_event(sequence, crate::pipeline::EventKind::Log),
        );
    }
    stopped.store(true, std::sync::atomic::Ordering::Release);
    flusher.join().unwrap();
    service.flush_notifications();
    let messages = updates.lock().unwrap();
    assert_eq!(messages.last().unwrap().task.status, TaskStatus::Failed);
    assert_eq!(
        messages
            .iter()
            .filter(|m| m.task.status == TaskStatus::Failed)
            .count(),
        1
    );
    assert!(messages
        .windows(2)
        .all(|pair| pair[0].task.revision < pair[1].task.revision));
    assert!(service.notifications.lock().unwrap().tasks.is_empty());
}

#[test]
fn notification_pump_starts_on_tauri_runtime_and_sends_once_per_interval() {
    assert!(tokio::runtime::Handle::try_current().is_err());
    let (_root, service, task) = tauri::async_runtime::block_on(fixture());
    let updates = record_notifications(&service);
    let run = notification_run(&service, task.task_id);
    let pump = service.spawn_notification_pump();
    tauri::async_runtime::block_on(async {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(850);
        let mut sequence = 0;
        while tokio::time::Instant::now() < deadline {
            for _ in 0..100 {
                sequence += 1;
                service.on_event(
                    task.task_id,
                    run,
                    notification_event(sequence, crate::pipeline::EventKind::Log),
                );
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    });
    pump.abort();
    let _ = tauri::async_runtime::block_on(pump);
    let messages = updates.lock().unwrap();
    assert!(!messages.is_empty());
    assert!(
        messages.len() <= 4,
        "ordinary output must not produce a per-line burst"
    );
    assert!(messages
        .iter()
        .all(|batch| batch.events.len() <= notifications::MAX_PENDING_LOGS));
}

#[test]
fn checkpoint_writer_starts_and_persists_from_a_plain_setup_thread() {
    assert!(tokio::runtime::Handle::try_current().is_err());
    let (root, service, task) = tauri::async_runtime::block_on(fixture());
    {
        let mut registry = service.records();
        let record = registry.tasks.get_mut(&task.task_id).unwrap();
        record.status = TaskStatus::Running;
        record.revision += 1;
    }
    // This call reproduced the startup panic before using Tauri's runtime.
    let writer = service.spawn_checkpoint_writer();
    let persisted = tauri::async_runtime::block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let bytes = tokio::fs::read(root.path().join("registry/tasks.json"))
                    .await
                    .unwrap();
                let registry: Registry = serde_json::from_slice(&bytes).unwrap();
                if registry.tasks[&task.task_id].revision > task.revision {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
    });
    writer.abort();
    let _ = tauri::async_runtime::block_on(writer);
    persisted.expect("the startup writer must run and persist on Tauri's runtime");
    assert!(tokio::runtime::Handle::try_current().is_err());
}

async fn fixture() -> (tempfile::TempDir, TaskService, TaskRecord) {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("input.mp4");
    std::fs::write(&input, b"test input").unwrap();
    let service = TaskService::at(root.path().join("registry"));
    let task = service
        .create(
            None,
            input,
            Quality::Balanced,
            root.path().join("projects"),
            false,
            "gui",
            Some("request-1".into()),
            Some(serde_json::json!({"quality":"balanced"})),
        )
        .await
        .unwrap();
    (root, service, task)
}

async fn persist_bound_project_metadata(service: &TaskService, id: Uuid) {
    let task = service.records().tasks[&id].clone();
    let root = task.project_path.as_ref().unwrap();
    let metadata = crate::project::ProjectMetadata {
        id: task.project_id.unwrap(),
        app_id: crate::project::PROJECT_APP_ID.into(),
        workspace_task_id: Some(id),
        project_path: root.clone(),
        source_path: task.input_path,
        quality: task.quality,
        status: task.status.project_status(),
        created_at: Utc::now(),
        ..Default::default()
    };
    atomic_write_json(&root.join("project.json"), &metadata)
        .await
        .unwrap();
}

fn assert_preparation_diagnostic(
    service: &TaskService,
    error: &ServiceError,
    operation: &str,
    role: &str,
) -> crate::diagnostics::DiagnosticReport {
    let details = error.details.as_ref().expect("safe preparation envelope");
    assert_eq!(details["preparation"]["operation"], operation);
    assert_eq!(details["preparation"]["pathRole"], role);
    let pipeline = &details["pipelineError"];
    assert_eq!(pipeline["engine"], "system");
    assert!(pipeline.get("failedStage").is_none());
    assert!(pipeline.get("projectPath").is_none());
    assert!(pipeline.get("logsDirectory").is_none());
    assert!(error.message.contains(operation));
    let failure = Uuid::parse_str(pipeline["failureId"].as_str().unwrap()).unwrap();
    let report = service
        .diagnostics
        .captured_report_for_test(failure)
        .unwrap();
    assert!(report.stage.is_none());
    assert_eq!(report.engine, Some(crate::pipeline::PipelineEngine::System));
    assert!(report.log_tail.contains(operation));
    report
}

#[tokio::test]
async fn continuation_preparation_read_errors_capture_diagnostics_without_changing_runs() {
    for mutation in [
        "state_missing",
        "state_directory",
        "state_json",
        "project_missing",
        "project_json",
        "state_missing_log_blocked",
    ] {
        let (root, service, task) = fixture().await;
        let project = root.path().join("private-checkpoint");
        std::fs::create_dir_all(&project).unwrap();
        let run = notification_run(&service, task.task_id);
        {
            let mut records = service.records();
            let saved = records.tasks.get_mut(&task.task_id).unwrap();
            saved.status = TaskStatus::Failed;
            saved.project_id = Some(Uuid::new_v4());
            saved.project_path = Some(project.clone());
        }
        persist_bound_project_metadata(&service, task.task_id).await;
        atomic_write_json(
            &project.join("state.json"),
            &crate::project::PipelineStateFile::created(Quality::High),
        )
        .await
        .unwrap();
        match mutation {
            "state_missing" => std::fs::remove_file(project.join("state.json")).unwrap(),
            "state_directory" => {
                std::fs::remove_file(project.join("state.json")).unwrap();
                std::fs::create_dir(project.join("state.json")).unwrap();
            }
            "state_json" => std::fs::write(project.join("state.json"), b"{").unwrap(),
            "project_missing" => std::fs::remove_file(project.join("project.json")).unwrap(),
            "project_json" => std::fs::write(project.join("project.json"), b"{").unwrap(),
            "state_missing_log_blocked" => {
                std::fs::remove_file(project.join("state.json")).unwrap();
                std::fs::write(project.join("logs"), b"not a directory").unwrap();
            }
            _ => unreachable!(),
        }
        service.persist().await.unwrap();
        let before = serde_json::to_value(service.get(task.task_id).await.unwrap()).unwrap();
        *service.executor.lock().unwrap() = Some(Arc::new(|_, _| {
            Box::pin(async { panic!("Preparation failure must not start an execution") })
        }));
        let events = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = events.clone();
        *service.emitter.lock().unwrap() = Some(Arc::new(move |_| {
            observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }));
        let error = service
            .continue_identified(task.task_id, run, default_engine_paths(None), None)
            .await
            .unwrap_err();
        assert_eq!(error.task_id, Some(task.task_id));
        assert_eq!(error.run_id, Some(run));
        let project_read = mutation.starts_with("project");
        let report = assert_preparation_diagnostic(
            &service,
            &error,
            if project_read {
                "read_retry_project"
            } else {
                "read_retry_state"
            },
            if project_read {
                "project_metadata"
            } else {
                "project_state"
            },
        );
        if mutation.ends_with("json") {
            assert_eq!(error.code, "INVALID_DATA");
            assert!(error.details.as_ref().unwrap()["preparation"]["errorKind"].is_null());
            assert!(error.details.as_ref().unwrap()["preparation"]["osError"].is_null());
        } else {
            assert_eq!(error.code, "IO_ERROR");
            assert!(error.details.as_ref().unwrap()["preparation"]["osError"].is_number());
        }
        assert!(!serde_json::to_string(&error)
            .unwrap()
            .contains(&project.to_string_lossy().to_string()));
        assert!(!report
            .detail
            .contains(&project.to_string_lossy().to_string()));
        assert_eq!(
            serde_json::to_value(service.get(task.task_id).await.unwrap()).unwrap(),
            before
        );
        assert_eq!(events.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn catalog_only_continuation_reports_the_requested_project_read_instead_of_not_found() {
    for mutation in [
        "index_missing",
        "missing_id",
        "index_directory",
        "index_json",
        "project_missing",
        "project_json",
        "state_missing",
        "state_json",
    ] {
        let root = tempfile::tempdir().unwrap();
        let index = root.path().join("project-index.json");
        let project_root = root.path().join("private-catalog-project");
        std::fs::create_dir_all(&project_root).unwrap();
        let project = Uuid::new_v4();
        let task = Uuid::new_v4();
        let run = Uuid::new_v4();
        let mut service = TaskService::at(root.path().join("registry"));
        service.continuation_index_path = Some(index.clone());
        atomic_write_json(
            &index,
            &serde_json::json!({"schemaVersion":1,
            "projects":[{"id":project,"path":project_root}]}),
        )
        .await
        .unwrap();
        let metadata = crate::project::ProjectMetadata {
            id: project,
            app_id: crate::project::PROJECT_APP_ID.into(),
            workspace_task_id: Some(task),
            project_path: project_root.clone(),
            status: ProjectStatus::Failed,
            ..Default::default()
        };
        atomic_write_json(&project_root.join("project.json"), &metadata)
            .await
            .unwrap();
        atomic_write_json(
            &project_root.join("state.json"),
            &crate::project::PipelineStateFile::created(Quality::Balanced),
        )
        .await
        .unwrap();
        match mutation {
            "index_missing" => std::fs::remove_file(&index).unwrap(),
            "missing_id" => atomic_write_json(
                &index,
                &serde_json::json!({"schemaVersion":1,"projects":[]}),
            )
            .await
            .unwrap(),
            "index_directory" => {
                std::fs::remove_file(&index).unwrap();
                std::fs::create_dir(&index).unwrap();
            }
            "index_json" => std::fs::write(&index, b"{").unwrap(),
            "project_missing" => std::fs::remove_file(project_root.join("project.json")).unwrap(),
            "project_json" => std::fs::write(project_root.join("project.json"), b"{").unwrap(),
            "state_missing" => std::fs::remove_file(project_root.join("state.json")).unwrap(),
            "state_json" => std::fs::write(project_root.join("state.json"), b"{").unwrap(),
            _ => unreachable!(),
        }
        assert!(service.all().await.unwrap().is_empty());
        let error = service
            .resume_project_identified(project, run, default_engine_paths(None), None)
            .await
            .unwrap_err();
        if matches!(mutation, "index_missing" | "missing_id") {
            assert_eq!(error.code, "TASK_NOT_FOUND");
            assert!(error.details.is_none());
        } else {
            assert_eq!(error.run_id, Some(run));
            let role = if mutation.starts_with("index") {
                "project_index"
            } else if mutation.starts_with("project") {
                "project_metadata"
            } else {
                "project_state"
            };
            assert_preparation_diagnostic(
                &service,
                &error,
                if role == "project_state" {
                    "read_retry_state"
                } else {
                    "read_retry_project"
                },
                role,
            );
            assert_eq!(
                error.code,
                if mutation.ends_with("json") {
                    "INVALID_DATA"
                } else {
                    "IO_ERROR"
                }
            );
        }
        assert!(service.all().await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn preparation_service_errors_redact_paths_after_preserving_the_primary_io_classification() {
    let root = tempfile::tempdir().unwrap();
    let service = TaskService::at(root.path().join("registry"));
    let private = root.path().join("secret-image.jpg");
    let context = PreparationContext::new(Some(Uuid::new_v4()), Some(root.path().into()));
    let error = context
        .step(
            RetryPreparationOperation::ReadRetryState,
            "project_state",
            None,
            async {
                Err::<(), _>(crate::error::SplatError::Io(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("Cannot read {}", private.display()),
                )))
            },
        )
        .await
        .unwrap_err();
    let error = service
        .preparation_service_error(
            error,
            &context.snapshot(),
            None,
            None,
            std::slice::from_ref(&private),
        )
        .await;
    assert_eq!(error.code, "IO_ERROR");
    assert_eq!(
        error.details.as_ref().unwrap()["preparation"]["errorKind"],
        "PermissionDenied"
    );
    assert_preparation_diagnostic(&service, &error, "read_retry_state", "project_state");
    let serialized = serde_json::to_string(&error).unwrap();
    assert!(!serialized.contains("secret-image"));
    assert!(!serialized.contains(&root.path().to_string_lossy().to_string()));
}

#[tokio::test]
async fn a_busy_task_prevents_preparation_reads_and_diagnostics_for_another_task() {
    let (root, service, task) = fixture().await;
    let stopped_run = notification_run(&service, task.task_id);
    let project = root.path().join("unreadable-stopped-project");
    std::fs::create_dir(&project).unwrap();
    {
        let mut records = service.records();
        let stopped = records.tasks.get_mut(&task.task_id).unwrap();
        stopped.status = TaskStatus::Failed;
        stopped.project_id = Some(Uuid::new_v4());
        stopped.project_path = Some(project.clone());
    }
    let input = root.path().join("another.mp4");
    std::fs::write(&input, b"fixture").unwrap();
    let active = service
        .create(
            None,
            input,
            Quality::Fast,
            root.path().join("projects"),
            true,
            "gui",
            None,
            None,
        )
        .await
        .unwrap();
    let active_run = notification_run(&service, active.task_id);
    service.persist().await.unwrap();
    let before = serde_json::to_value(service.all().await.unwrap()).unwrap();
    let error = service
        .continue_identified(task.task_id, stopped_run, default_engine_paths(None), None)
        .await
        .unwrap_err();
    assert_eq!(error.code, "TASK_BUSY");
    assert_eq!(error.task_id, Some(active.task_id));
    assert_eq!(error.run_id, Some(active_run));
    assert!(error.details.is_none());
    assert!(!project.join("logs").exists());
    assert_eq!(
        serde_json::to_value(service.all().await.unwrap()).unwrap(),
        before
    );
}

#[tokio::test]
async fn a_busy_task_prevents_catalog_only_continuation_from_reading_a_broken_index() {
    for identified in [false, true] {
        let (root, mut service, active) = fixture().await;
        let active_run = notification_run(&service, active.task_id);
        let index = root.path().join("unreadable-project-index.json");
        std::fs::write(&index, b"{").unwrap();
        service.continuation_index_path = Some(index.clone());
        service.persist().await.unwrap();
        let before = serde_json::to_value(service.all().await.unwrap()).unwrap();
        let project = Uuid::new_v4();
        let result = if identified {
            service
                .resume_project_identified(
                    project,
                    Uuid::new_v4(),
                    default_engine_paths(None),
                    None,
                )
                .await
        } else {
            service
                .resume(project, default_engine_paths(None), None)
                .await
        };
        let error = result.unwrap_err();
        assert_eq!(error.code, "TASK_BUSY");
        assert_eq!(error.task_id, Some(active.task_id));
        assert_eq!(error.run_id, Some(active_run));
        assert!(error.details.is_none());
        assert_eq!(std::fs::read(&index).unwrap(), b"{");
        assert_eq!(
            serde_json::to_value(service.all().await.unwrap()).unwrap(),
            before
        );
    }
}

#[tokio::test]
async fn preparation_redaction_preserves_fixed_tags_when_private_basenames_collide() {
    for name in ["state", "project", "brush"] {
        let root = tempfile::tempdir().unwrap();
        let private_root = root.path().join(name);
        std::fs::create_dir(&private_root).unwrap();
        let service = TaskService::at(root.path().join("registry"));
        let context = PreparationContext::new(Some(Uuid::new_v4()), Some(private_root.clone()));
        let error = context
            .step(
                RetryPreparationOperation::ReadRetryState,
                "project_state",
                Some("brush"),
                async {
                    Err::<(), _>(
                        crate::error::SplatError::Io(std::io::Error::from_raw_os_error(5))
                            .with_io_diagnostic(format!(
                                "io_substep=replace_state; target={}",
                                private_root.display()
                            )),
                    )
                },
            )
            .await
            .unwrap_err();
        let error = service
            .preparation_service_error(
                error,
                &context.snapshot(),
                None,
                None,
                std::slice::from_ref(&private_root),
            )
            .await;
        assert_eq!(error.code, "IO_ERROR");
        assert!(error
            .message
            .contains("retry_preparation_operation=read_retry_state"));
        assert!(error.message.contains("path_role=project_state"));
        assert!(error.message.contains("cache=brush"));
        assert!(error.message.contains("io_substep=replace_state"));
        assert!(error.message.contains("os_error=5"));
        assert!(!error
            .message
            .contains(&private_root.to_string_lossy().to_string()));
        assert_preparation_diagnostic(&service, &error, "read_retry_state", "project_state");
    }
}

#[tokio::test]
async fn image_matching_request_survives_restart_and_cannot_change_on_retry() {
    use crate::project::ImageMatchingStrategy;
    let (root, service, _) = fixture().await;
    let images = root.path().join("photos");
    std::fs::create_dir(&images).unwrap();
    for name in ["a.png", "b.png"] {
        image::RgbImage::new(4, 3).save(images.join(name)).unwrap();
    }
    for strategy in [
        ImageMatchingStrategy::Sequential,
        ImageMatchingStrategy::Exhaustive,
    ] {
        let parameters = serde_json::json!({"imageMatchingStrategy": strategy});
        let task = service
            .create(
                None,
                images.clone(),
                Quality::Balanced,
                root.path().join("projects"),
                true,
                "gui",
                None,
                Some(parameters.clone()),
            )
            .await
            .unwrap();
        let reopened = TaskService::at(root.path().join("registry"));
        let restored = reopened.get(task.task_id).await.unwrap();
        let mut expected = Some(generation_parameters(Some(parameters)).unwrap());
        sync_saved_camera_grouping_policy(
            &mut expected,
            Some(crate::pipeline::inputs::CAMERA_GROUPING_POLICY_VERSION),
        )
        .unwrap();
        sync_saved_ordinary_image_preparation(
            &mut expected,
            Some(crate::pipeline::inputs::ORDINARY_IMAGE_PREPARATION_VERSION),
        )
        .unwrap();
        assert_eq!(restored.request_parameters, expected);
        assert_eq!(
            restored.input_type,
            crate::project::ProjectInputType::Images
        );
        // The original request wins if transport retries reuse the same workspace ID.
        let retried = reopened
            .create(
                Some(task.task_id),
                images.clone(),
                Quality::High,
                root.path().join("projects"),
                false,
                "gui",
                None,
                Some(serde_json::json!({"imageMatchingStrategy":"other"})),
            )
            .await
            .unwrap();
        assert_eq!(retried.request_parameters, restored.request_parameters);
    }
}

#[tokio::test]
async fn changed_video_identity_is_rejected_before_starting_a_run() {
    let (root, service, _) = fixture().await;
    let input = root.path().join("changed.mp4");
    std::fs::write(&input, b"original").unwrap();
    let identity = crate::video::panorama::source_identity(&input).unwrap();
    let task = service
        .create(
            None,
            input.clone(),
            Quality::Fast,
            root.path().join("projects"),
            false,
            "gui",
            None,
            Some(serde_json::json!({"videoSourceIdentity": identity})),
        )
        .await
        .unwrap();
    std::fs::write(&input, b"changed input size").unwrap();
    let error = service
        .start_inner(task.task_id, default_engine_paths(None), None, false, None)
        .await
        .unwrap_err();
    assert_eq!(error.code, "INPUT_CHANGED");
    let unchanged = service.get(task.task_id).await.unwrap();
    assert_eq!(unchanged.status, TaskStatus::Created);
    assert!(unchanged.run_id.is_none() && unchanged.project_id.is_none());
    assert!(!unchanged.status.active());
}
#[tokio::test]
async fn concurrent_idempotent_creation_and_conflicting_parameters() {
    let (root, service, task) = fixture().await;
    let (a, b) = tokio::join!(
        service.create(
            None,
            root.path().join("input.mp4"),
            Quality::Balanced,
            root.path().join("other"),
            true,
            "mcp",
            Some("request-1".into()),
            Some(serde_json::json!({"quality":"balanced"}))
        ),
        service.create(
            None,
            root.path().join("input.mp4"),
            Quality::Balanced,
            root.path().join("projects"),
            false,
            "gui",
            Some("request-1".into()),
            Some(serde_json::json!({"quality":"balanced"}))
        )
    );
    assert_eq!(a.unwrap().task_id, task.task_id);
    assert_eq!(b.unwrap().task_id, task.task_id);
    assert_eq!(service.all().await.unwrap().len(), 1);
    let error = service
        .create(
            None,
            root.path().join("missing.mp4"),
            Quality::High,
            root.path().into(),
            true,
            "mcp",
            Some("request-1".into()),
            Some(serde_json::json!({"quality":"high"})),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "IDEMPOTENCY_CONFLICT");
}
#[tokio::test]
async fn gui_mcp_admission_duplicate_start_and_disconnect_are_independent() {
    let (root, service, first) = fixture().await;
    let signal = Arc::new(tokio::sync::Notify::new());
    let worker_signal = signal.clone();
    *service.executor.lock().unwrap() = Some(Arc::new(move |_, _| {
        let signal = worker_signal.clone();
        Box::pin(async move {
            signal.notified().await;
            Err(crate::error::SplatError::Process("mock failure".into()))
        })
    }));
    let second = service
        .create(
            None,
            root.path().join("input.mp4"),
            Quality::Fast,
            root.path().join("projects"),
            false,
            "mcp",
            None,
            None,
        )
        .await
        .unwrap();
    let receipt = service
        .start(first.task_id, default_engine_paths(None), None)
        .await
        .unwrap();
    assert!(receipt.accepted);
    assert!(
        !service
            .start(first.task_id, default_engine_paths(None), None)
            .await
            .unwrap()
            .accepted
    );
    let busy = service
        .start(second.task_id, default_engine_paths(None), None)
        .await
        .unwrap_err();
    assert_eq!(busy.code, "TASK_BUSY");
    assert_eq!(busy.task_id, Some(first.task_id));
    assert_eq!(
        service.get(second.task_id).await.unwrap().status,
        TaskStatus::Created
    );
    // Dropping the request/receipt leaves the execution registered and alive.
    let _receipt = receipt;
    assert!(service.get(first.task_id).await.unwrap().status.active());
    signal.notify_one();
    let terminal = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        service.wait(first.task_id),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(terminal.status, TaskStatus::Failed);
    assert!(service.active.lock().await.is_none());
    assert!(
        !service
            .start(first.task_id, default_engine_paths(None), None)
            .await
            .unwrap()
            .accepted
    );
    assert_eq!(service.get(first.task_id).await.unwrap().runs.len(), 1);
    *service.executor.lock().unwrap() = None;
}
#[tokio::test]
async fn early_failure_remains_queryable_and_releases_the_lock() {
    let (_root, service, task) = fixture().await;
    let receipt = service
        .start(
            task.task_id,
            default_engine_paths(Some(PathBuf::from("missing-engines"))),
            None,
        )
        .await
        .unwrap();
    assert!(receipt.accepted);
    let terminal = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        service.wait(task.task_id),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(terminal.status, TaskStatus::Failed);
    assert!(terminal.project_id.is_none());
    assert!(terminal.error.is_some());
    assert!(service.active.lock().await.is_none());
}
#[tokio::test]
async fn cancellation_rejects_stale_execution_then_confirms_terminal_state() {
    let (_root, service, task) = fixture().await;
    let updates = record_notifications(&service);
    let observe = service.clone();
    *service.executor.lock().unwrap() = Some(Arc::new(move |task, _| {
        let service = observe.clone();
        Box::pin(async move {
            loop {
                if service.get(task.task_id).await.unwrap().status == TaskStatus::Cancelling {
                    return Err(crate::error::SplatError::Cancelled);
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
    }));
    let receipt = service
        .start(task.task_id, default_engine_paths(None), None)
        .await
        .unwrap();
    let run = receipt.run_id.unwrap();
    service.on_event(
        task.task_id,
        run,
        notification_event(1, crate::pipeline::EventKind::Progress),
    );
    service.on_event(
        task.task_id,
        run,
        notification_event(2, crate::pipeline::EventKind::Log),
    );
    assert_eq!(
        service
            .cancel(task.task_id, Uuid::new_v4())
            .await
            .unwrap_err()
            .code,
        "STALE_RUN_ID"
    );
    assert_eq!(
        service.cancel(task.task_id, run).await.unwrap().status,
        TaskStatus::Cancelling
    );
    let terminal = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        service.wait(task.task_id),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(terminal.status, TaskStatus::Cancelled);
    assert_eq!(
        service.cancel(task.task_id, run).await.unwrap().status,
        TaskStatus::Cancelled
    );
    assert!(service.active.lock().await.is_none());
    let messages = updates.lock().unwrap();
    let cancelling = messages
        .iter()
        .find(|message| message.task.status == TaskStatus::Cancelling)
        .unwrap();
    assert_eq!(cancelling.events.last().unwrap().sequence, 2);
    assert_eq!(messages.last().unwrap().task.status, TaskStatus::Cancelled);
    assert!(service.notifications.lock().unwrap().tasks.is_empty());
    *service.executor.lock().unwrap() = None;
}
#[tokio::test]
async fn panic_and_restart_do_not_leave_phantom_running_tasks() {
    let (root, service, task) = fixture().await;
    *service.executor.lock().unwrap() = Some(Arc::new(|_, _| {
        Box::pin(async { panic!("mock runner panic") })
    }));
    service
        .start(task.task_id, default_engine_paths(None), None)
        .await
        .unwrap();
    let terminal = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        service.wait(task.task_id),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(terminal.status, TaskStatus::Failed);
    assert!(service.active.lock().await.is_none());
    {
        let _guard = service.gate.lock().await;
        service
            .records()
            .tasks
            .get_mut(&task.task_id)
            .unwrap()
            .status = TaskStatus::Running;
        service.persist().await.unwrap();
    }
    let restarted = TaskService::at(root.path().join("registry"));
    assert_eq!(
        restarted.get(task.task_id).await.unwrap().status,
        TaskStatus::Interrupted
    );
    assert!(restarted.active.lock().await.is_none());
}
#[tokio::test]
async fn task_configuration_is_frozen_and_old_run_events_are_rejected() {
    let (_root, service, task) = fixture().await;
    let mut config = task.configuration.clone();
    config.shared.minimum_selected_frames = 987;
    crate::presets::with_pipeline_config(config, async {
        assert_eq!(
            crate::presets::pipeline_optimization_config()
                .shared
                .minimum_selected_frames,
            987
        );
        let inherited = crate::presets::spawn_pipeline_blocking(|| {
            crate::presets::pipeline_optimization_config()
                .shared
                .minimum_selected_frames
        })
        .await
        .unwrap();
        assert_eq!(inherited, 987);
    })
    .await;
    assert_eq!(
        service
            .get(task.task_id)
            .await
            .unwrap()
            .configuration
            .shared
            .minimum_selected_frames,
        task.configuration.shared.minimum_selected_frames
    );
    let revision = task.revision;
    service.on_event(
        task.task_id,
        Uuid::new_v4(),
        PipelineEvent::mapped(PipelineStage::TrainingSplats, 0.9, "late"),
    );
    assert_eq!(service.get(task.task_id).await.unwrap().revision, revision);
}
#[tokio::test]
async fn logs_are_bounded_incremental_and_detect_rewrite_and_wrong_execution() {
    let (root, _, mut task) = fixture().await;
    let logroot = root.path().join("logs");
    std::fs::create_dir(&logroot).unwrap();
    let path = logroot.join("brush.log");
    std::fs::write(
        &path,
        (0..10000).map(|i| format!("行 {i}\n")).collect::<String>(),
    )
    .unwrap();
    let run = Uuid::new_v4();
    task.status = TaskStatus::Running;
    task.run_id = Some(run);
    task.runs.push(TaskRun {
        run_id: run,
        kind: "new".into(),
        continued_from_status: None,
        started_at: Utc::now(),
        ended_at: None,
        logs: BTreeMap::from([("brush".into(), logs::register(&logroot, &path, 0).unwrap())]),
        last_engine: None,
        last_exit_code: None,
        error: None,
        brush_executions: Vec::new(),
        current_brush_execution: None,
    });
    let request = logs::LogRequest {
        task_id: task.task_id,
        max_bytes: Some(1024),
        ..Default::default()
    };
    let first = logs::read(&task, &request).unwrap();
    assert!(first.entries[0].text.len() <= 1024);
    assert!(first.truncated);
    assert!(!first.entries[0].text.contains('�'));
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all("追加行\n半行".as_bytes()).unwrap();
    let next = logs::read(
        &task,
        &logs::LogRequest {
            cursor: first.next_cursor.clone(),
            ..request.clone()
        },
    )
    .unwrap();
    assert_eq!(next.entries[0].text, "追加行\n");
    file.write_all("补齐\n".as_bytes()).unwrap();
    let third = logs::read(
        &task,
        &logs::LogRequest {
            cursor: next.next_cursor.clone(),
            ..request.clone()
        },
    )
    .unwrap();
    assert_eq!(third.entries[0].text, "半行补齐\n");
    std::fs::write(&path, "重写日志\n").unwrap();
    let reset = logs::read(
        &task,
        &logs::LogRequest {
            cursor: third.next_cursor,
            ..request.clone()
        },
    )
    .unwrap();
    assert!(reset.cursor_reset);
    let other = Uuid::new_v4();
    task.run_id = Some(other);
    let mut other_run = task.runs[0].clone();
    other_run.run_id = other;
    task.runs.push(other_run);
    assert_eq!(
        logs::read(
            &task,
            &logs::LogRequest {
                cursor: first.next_cursor,
                ..request
            }
        )
        .unwrap_err()
        .code,
        "CURSOR_MISMATCH"
    );
}
#[tokio::test]
async fn cursors_do_not_leak_redacted_text_and_reject_cross_task_access() {
    let (root, _, mut task) = fixture().await;
    let logroot = root.path().join("logs");
    std::fs::create_dir(&logroot).unwrap();
    let path = logroot.join("brush.log");
    std::fs::write(&path, "Authorization: Bearer private-token\n").unwrap();
    let run = Uuid::new_v4();
    task.status = TaskStatus::Running;
    task.run_id = Some(run);
    task.runs.push(TaskRun {
        run_id: run,
        kind: "new".into(),
        continued_from_status: None,
        started_at: Utc::now(),
        ended_at: Some(Utc::now()),
        logs: BTreeMap::from([("brush".into(), logs::register(&logroot, &path, 0).unwrap())]),
        last_engine: None,
        last_exit_code: None,
        error: None,
        brush_executions: Vec::new(),
        current_brush_execution: None,
    });
    let request = logs::LogRequest {
        task_id: task.task_id,
        ..Default::default()
    };
    let page = logs::read(&task, &request).unwrap();
    assert!(!page.entries[0].text.contains("private-token"));
    let cursor = logs::decode(page.next_cursor.as_ref().unwrap()).unwrap();
    assert!(cursor.files.values().all(|p| p.anchor.len() == 64));
    task.task_id = Uuid::new_v4();
    assert_eq!(
        logs::read(
            &task,
            &logs::LogRequest {
                task_id: task.task_id,
                cursor: page.next_cursor,
                ..Default::default()
            }
        )
        .unwrap_err()
        .code,
        "CURSOR_MISMATCH"
    );
    assert_eq!(
        logs::read(
            &task,
            &logs::LogRequest {
                task_id: task.task_id,
                sources: Some(vec!["../secrets".into()]),
                ..Default::default()
            }
        )
        .unwrap_err()
        .code,
        "INVALID_LOG_SOURCE"
    );
}
#[test]
fn unauthorized_paths_and_log_symlinks_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let private = outside.path().join("private.log");
    std::fs::write(&private, "private").unwrap();
    assert_eq!(
        authorize_input(&private, &[root.path().into()])
            .unwrap_err()
            .code,
        "PATH_NOT_AUTHORIZED"
    );
    #[cfg(windows)]
    let link = std::os::windows::fs::symlink_file(&private, root.path().join("brush.log"));
    #[cfg(unix)]
    let link = std::os::unix::fs::symlink(&private, root.path().join("brush.log"));
    assert!(
        link.is_ok(),
        "This verification requires permission to create its temporary symlink fixture"
    );
    if link.is_ok() {
        assert_eq!(
            logs::register(root.path(), &root.path().join("brush.log"), 0)
                .unwrap_err()
                .code,
            "LOG_ACCESS_DENIED"
        );
    }
}

#[tokio::test]
async fn successful_execution_is_persisted_and_terminal_events_cannot_reactivate_it() {
    let (root, service, task) = fixture().await;
    let path = root.path().join("synthetic-project");
    let project = Uuid::new_v4();
    *service.executor.lock().unwrap() = Some(Arc::new(move |_, _| {
        let path = path.clone();
        Box::pin(async move {
            Ok(PipelineResult {
                project_id: project.to_string(),
                project_path: path.clone(),
                final_ply: path.join("final.ply"),
                file_size: 123,
                splat_count: 1,
                input_images: 2,
                registered_images: 2,
                registered_ratio: 1.0,
                points_3d: 1,
                duration_ms: 1,
                completed_at: Utc::now(),
                warning: None,
                logs_directory: path.join("logs"),
                source_duration_seconds: None,
            })
        })
    }));
    let receipt = service
        .start(task.task_id, default_engine_paths(None), None)
        .await
        .unwrap();
    let completed = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        service.wait(task.task_id),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(completed.status, TaskStatus::Completed);
    assert_eq!(completed.result.as_ref().unwrap()["fileSize"], 123);
    assert!(service.active.lock().await.is_none());
    service.on_event(
        task.task_id,
        receipt.run_id.unwrap(),
        PipelineEvent::mapped(PipelineStage::TrainingSplats, 0.5, "late progress"),
    );
    assert_eq!(
        service.get(task.task_id).await.unwrap().revision,
        completed.revision
    );
    let reopened = TaskService::at(root.path().join("registry"));
    assert_eq!(
        reopened.get(task.task_id).await.unwrap().status,
        TaskStatus::Completed
    );
}

// Drive the production Runner metric writers through TaskService::start_inner.
// Returning a synthetic result alone would miss the shared-tracker regression.
fn write_planner_fixture(
    runner: &PipelineRunner,
    task: &TaskRecord,
    selected: u64,
    complete: bool,
) -> crate::project::ProjectOutput {
    use crate::{
        engines::ColmapAccelerationStatus,
        presets::resolve_brush_training_preset,
        project::{FrameState, PipelineStateFile, ProjectInputType, ReshootState},
        reconstruction::validator::{ReconstructionQuality, ReconstructionReport},
        video::{ImageSequenceInfo, PlannedFrame, VideoInfo},
    };
    let is_images = task.input_path.is_dir();
    let is_reshoot = task.source_project_id.is_some();
    let mut state = PipelineStateFile::created_for(
        task.quality,
        if is_images {
            ProjectInputType::Images
        } else {
            ProjectInputType::Video
        },
    );
    state.planner_enabled = task.planner_enabled;
    state.frames = Some(FrameState {
        initial_extracted_frames: selected,
        extracted_frames: Some(selected),
        candidate_frames: (0..selected + 20)
            .map(|index| PlannedFrame {
                source_frame_index: index,
                timestamp_seconds: index as f64 / 30.0,
            })
            .collect(),
        ..Default::default()
    });
    if is_images {
        state.image_sequence = Some(ImageSequenceInfo {
            image_count: selected,
            width: 3840,
            height: 2160,
            has_alpha: false,
            requires_large_sequence_confirmation: false,
        });
    } else {
        state.video = Some(VideoInfo {
            width: 3840,
            height: 2160,
            total_frames: 300,
            fps: 30.0,
            duration: 10.0,
            codec: "h264".into(),
            rotation: 0,
            pixel_format: "rgb24".into(),
            has_alpha: false,
        });
    }
    if is_reshoot {
        state.reshoot = Some(ReshootState {
            source_image_count: 100,
            reshoot_frame_count: selected,
            supplemental_frames_complete: true,
            ..Default::default()
        });
    }
    let input_images = selected + if is_reshoot { 100 } else { 0 };
    let report = ReconstructionReport {
        input_images,
        registered_images: input_images - 2,
        registered_ratio: (input_images - 2) as f64 / input_images as f64,
        points_3d: selected * 100,
        quality: ReconstructionQuality::Good,
    };
    let initial = resolve_brush_training_preset(
        task.quality,
        task.planner_enabled,
        Some(24_576),
        3840,
        report.points_3d,
    );
    state.brush_training.initial_resolved = Some(initial);
    let resolved = if task.project_id.is_some() && task.planner_enabled {
        state.brush_training.oom_retry_used = true;
        initial.downgrade_after_oom(report.points_3d).unwrap()
    } else {
        initial
    };
    state.brush_training.resolved = Some(resolved);
    // Exercise the same saved checkpoint representation used on resume.
    let state: PipelineStateFile =
        serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    runner.record_frame_metrics(&state, selected, None);
    let acceleration: ColmapAccelerationStatus = serde_json::from_value(serde_json::json!({
        "backend": "gpu", "detectionState": "ready", "reasonCode": "gpuReady",
        "reason": "fixture", "device": null, "detectedNvidiaDeviceCount": 1,
        "requirements": { "minimumDriverVersion": "580", "minimumComputeCapability": "7.5" }
    }))
    .unwrap();
    runner.record_reconstruction_metrics(&report, &acceleration, false, is_reshoot);
    runner.record_brush_metrics(&state);
    let output = crate::project::ProjectOutput {
        final_ply: task.projects_root.join("final.ply"),
        file_size: selected * 1000,
        splat_count: selected * 10,
        input_images,
        registered_images: report.registered_images,
        registered_ratio: report.registered_ratio,
        points_3d: report.points_3d,
    };
    if complete {
        runner.record_final_metrics(&output);
    }
    output
}

#[tokio::test]
async fn task_start_reports_runner_metrics_for_generation_resume_and_reshoot() {
    let root = tempfile::tempdir().unwrap();
    let video = root.path().join("input.mp4");
    let images = root.path().join("images");
    std::fs::write(&video, b"fixture").unwrap();
    std::fs::create_dir(&images).unwrap();
    for name in ["a.png", "b.png"] {
        image::RgbImage::new(4, 3).save(images.join(name)).unwrap();
    }
    let service = TaskService::at(root.path().join("registry"));
    let (telemetry, captured) = TelemetryService::recording(root.path().join("telemetry.json"));
    telemetry.enable_for_test().await;
    let mut run_ids = std::collections::BTreeSet::new();
    for (index, (is_images, reshoot, resume, enabled, fails)) in [
        (false, false, false, true, false),
        (true, false, false, false, false),
        (false, false, true, true, false),
        (true, false, true, false, false),
        (false, true, false, true, false),
        (true, true, false, false, false),
        (true, true, true, true, false),
        (false, false, false, true, true),
        (true, true, false, true, true),
    ]
    .into_iter()
    .enumerate()
    {
        let selected = 30 + index as u64;
        let task = service
            .create(
                None,
                if is_images {
                    images.clone()
                } else {
                    video.clone()
                },
                Quality::High,
                root.path().join("projects"),
                enabled,
                "gui",
                None,
                None,
            )
            .await
            .unwrap();
        if reshoot {
            service
                .mark_reshoot(task.task_id, Uuid::new_v4())
                .await
                .unwrap();
        }
        if resume {
            let project = root.path().join(format!("saved-project-{index}"));
            std::fs::create_dir_all(&project).unwrap();
            let mut state = crate::project::PipelineStateFile::created(Quality::High);
            state.planner_enabled = enabled;
            state.configuration = Some(task.configuration.clone());
            atomic_write_json(&project.join("state.json"), &state)
                .await
                .unwrap();
            let task_id = {
                let mut records = service.records();
                let task = records.tasks.get_mut(&task.task_id).unwrap();
                task.project_id = Some(Uuid::new_v4());
                task.project_path = Some(project);
                // Model a saved legacy work order; continuation must not add policy v6.
                task.status = TaskStatus::Interrupted;
                task.planner_enabled = enabled;
                task.generation_policy = None;
                task.request_parameters = None;
                task.task_id
            };
            persist_bound_project_metadata(&service, task_id).await;
        }
        *service.executor.lock().unwrap() = Some(Arc::new(move |task, runner| {
            Box::pin(async move {
                let output = write_planner_fixture(&runner, &task, selected, !fails);
                if fails {
                    return Err(crate::error::SplatError::BrushOutOfMemory("fixture".into()));
                }
                Ok(PipelineResult {
                    project_id: task.project_id.unwrap_or_else(Uuid::new_v4).to_string(),
                    project_path: task.projects_root.clone(),
                    final_ply: output.final_ply,
                    file_size: output.file_size,
                    splat_count: output.splat_count,
                    input_images: output.input_images,
                    registered_images: output.registered_images,
                    registered_ratio: output.registered_ratio,
                    points_3d: output.points_3d,
                    duration_ms: 100,
                    completed_at: Utc::now(),
                    warning: None,
                    logs_directory: task.projects_root.join("logs"),
                    source_duration_seconds: (!is_images).then_some(10.0),
                })
            })
        }));
        service
            .start_inner(
                task.task_id,
                default_engine_paths(None),
                Some(telemetry.clone()),
                resume,
                None,
            )
            .await
            .unwrap();
        let finished = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            service.wait(task.task_id),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            finished.status,
            if fails {
                TaskStatus::Failed
            } else {
                TaskStatus::Completed
            }
        );
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let ready = {
                    let events = captured.lock().unwrap();
                    events
                        .iter()
                        .filter(|e| e["event"] == "planner_evaluation")
                        .count()
                        == index + 1
                        && events
                            .iter()
                            .filter(|e| {
                                e["event"] == "generation_completed"
                                    || e["event"] == "generation_failed"
                            })
                            .count()
                            == index + 1
                };
                if ready {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let events = captured.lock().unwrap();
        let evaluation = events
            .iter()
            .filter(|e| e["event"] == "planner_evaluation")
            .nth(index)
            .unwrap();
        if index == 0 || index == 4 {
            eprintln!("OOOSPLAT_TELEMETRY_DEBUG {evaluation}");
        }
        assert!(run_ids.insert(
            evaluation["properties"]["runId"]
                .as_str()
                .unwrap()
                .to_owned()
        ));
        let p = &evaluation["properties"];
        let expected_enabled = !resume || enabled;
        assert_eq!(p["plannerEnabled"], expected_enabled);
        assert_eq!(p["plannerVersion"].is_null(), !expected_enabled);
        assert_eq!(p["runKind"], if resume { "resume" } else { "new" });
        assert_eq!(p["inputType"], if is_images { "images" } else { "video" });
        assert_eq!(p["framePlan"]["sourceWidth"], 3840);
        assert_eq!(
            p["framePlan"]["sourceItemCount"],
            if is_images { selected } else { 300 }
        );
        assert_eq!(p["framePlan"]["initialSelectedCount"], selected);
        assert_eq!(
            p["initialReconstruction"]["inputImages"],
            selected + if reshoot { 100 } else { 0 }
        );
        assert_eq!(p["initialReconstruction"]["points3d"], selected * 100);
        assert!(!p["brush"]["finalProfile"].is_null());
        assert_eq!(p["brush"]["oomRetryUsed"], resume && enabled);
        if resume && enabled {
            assert_ne!(p["brush"]["initialProfile"], p["brush"]["finalProfile"]);
        }
        assert!(p["stageDurations"]["extractingFramesMs"].is_null());
        if reshoot {
            assert_eq!(p["bridge"]["status"], "notApplicable");
            assert_eq!(p["bridge"]["addedFrames"], 0);
            assert_eq!(p["framePlan"]["candidateCount"], selected);
            assert!(p["framePlan"]["configuredCandidateFps"].is_null());
        }
        if fails {
            assert_eq!(p["outcome"], "failed");
            assert_eq!(p["errorCode"], "brush_out_of_memory");
            assert!(p["finalResult"].is_null());
        } else {
            assert_eq!(p["outcome"], "completed");
            assert_eq!(p["finalResult"]["splatCount"], selected * 10);
            assert_eq!(p["finalResult"]["plySizeBytes"], selected * 1000);
        }
    }
}

#[tokio::test]
async fn task_cancellation_and_disabled_analytics_send_no_terminal_statistics() {
    for cancelled in [true, false] {
        let (root, service, task) = fixture().await;
        let (telemetry, events) = TelemetryService::recording(root.path().join("telemetry.json"));
        telemetry.enable_for_test().await;
        if !cancelled {
            telemetry.set_consent(false).await.unwrap();
        }
        *service.executor.lock().unwrap() = Some(Arc::new(move |task, runner| {
            Box::pin(async move {
                write_planner_fixture(&runner, &task, 30, false);
                Err(if cancelled {
                    crate::error::SplatError::Cancelled
                } else {
                    crate::error::SplatError::BrushDeviceLost("fixture".into())
                })
            })
        }));
        service
            .start(
                task.task_id,
                default_engine_paths(None),
                Some(telemetry.clone()),
            )
            .await
            .unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            service.wait(task.task_id),
        )
        .await
        .unwrap()
        .unwrap();
        let events = events.lock().unwrap();
        if cancelled {
            assert!(!events.iter().any(|e| e["event"] == "planner_evaluation"
                || e["event"] == "generation_failed"
                || e["event"] == "generation_completed"));
        } else {
            assert!(events.is_empty());
        }
    }
}

#[tokio::test]
async fn log_cursor_authentication_rotation_and_global_line_limit_are_enforced() {
    let (root, _, mut task) = fixture().await;
    let logroot = root.path().join("logs");
    std::fs::create_dir(&logroot).unwrap();
    let first = logroot.join("brush.log");
    let second = logroot.join("colmap.log");
    std::fs::write(
        &first,
        (0..150)
            .map(|n| format!("brush row {n}\n"))
            .collect::<String>(),
    )
    .unwrap();
    std::fs::write(&second, "colmap row\n").unwrap();
    let run = Uuid::new_v4();
    task.status = TaskStatus::Running;
    task.run_id = Some(run);
    task.runs.push(TaskRun {
        run_id: run,
        kind: "generation".into(),
        continued_from_status: None,
        started_at: Utc::now(),
        ended_at: None,
        logs: BTreeMap::from([
            ("brush".into(), logs::register(&logroot, &first, 0).unwrap()),
            (
                "colmap".into(),
                logs::register(&logroot, &second, 0).unwrap(),
            ),
        ]),
        error: None,
        last_engine: None,
        last_exit_code: None,
        brush_executions: Vec::new(),
        current_brush_execution: None,
    });
    let request = logs::LogRequest {
        task_id: task.task_id,
        ..Default::default()
    };
    let page = logs::read(&task, &request).unwrap();
    assert!(
        page.entries
            .iter()
            .map(|chunk| chunk.text.lines().count())
            .sum::<usize>()
            <= 100
    );
    let mut forged = page.next_cursor.clone().unwrap().into_bytes();
    forged[0] = if forged[0] == b'0' { b'1' } else { b'0' };
    assert!(logs::read(
        &task,
        &logs::LogRequest {
            cursor: Some(String::from_utf8(forged).unwrap()),
            ..request.clone()
        }
    )
    .is_err());
    let source_request = logs::LogRequest {
        sources: Some(vec!["brush".into()]),
        ..request
    };
    let before = logs::read(&task, &source_request).unwrap();
    std::fs::rename(&first, logroot.join("brush.log.1")).unwrap();
    std::fs::write(&first, "new generation of the same registered source\n").unwrap();
    task.runs
        .last_mut()
        .unwrap()
        .logs
        .insert("brush".into(), logs::register(&logroot, &first, 0).unwrap());
    let after = logs::read(
        &task,
        &logs::LogRequest {
            cursor: before.next_cursor,
            ..source_request
        },
    )
    .unwrap();
    assert!(after.cursor_reset);
    assert!(after.entries[0].text.contains("new generation"));
}

#[tokio::test]
async fn default_log_cursor_adopts_new_sources_but_explicit_filters_remain_strict() {
    let (root, service, task) = fixture().await;
    let run = notification_run(&service, task.task_id);
    let mut task = service.get(task.task_id).await.unwrap();
    let logs_root = root.path().join("logs");
    std::fs::create_dir(&logs_root).unwrap();
    let request = logs::LogRequest {
        task_id: task.task_id,
        run_id: Some(run),
        ..Default::default()
    };
    let empty = logs::read(&task, &request).unwrap();
    let first = logs_root.join("ffmpeg.log");
    std::fs::write(&first, "extracted source frames\n").unwrap();
    logs::insert_registered(
        &mut task.runs.last_mut().unwrap().logs,
        logs::register(&logs_root, &first, 0).unwrap(),
    );
    let initial = logs::read(
        &task,
        &logs::LogRequest {
            cursor: empty.next_cursor,
            ..request.clone()
        },
    )
    .unwrap();
    assert!(initial
        .entries
        .iter()
        .any(|entry| entry.text.contains("extracted source frames")));
    let second = logs_root.join("colmap.log");
    std::fs::write(&second, "matched source images\n").unwrap();
    logs::insert_registered(
        &mut task.runs.last_mut().unwrap().logs,
        logs::register(&logs_root, &second, 0).unwrap(),
    );
    let added = logs::read(
        &task,
        &logs::LogRequest {
            cursor: initial.next_cursor,
            ..request.clone()
        },
    )
    .unwrap();
    assert!(added
        .entries
        .iter()
        .any(|entry| entry.text.contains("matched source images")));
    let filter = logs::LogRequest {
        sources: Some(vec!["ffmpeg".into()]),
        ..request.clone()
    };
    let filtered = logs::read(&task, &filter).unwrap();
    let error = logs::read(
        &task,
        &logs::LogRequest {
            sources: Some(vec!["colmap".into()]),
            cursor: filtered.next_cursor,
            ..request
        },
    )
    .unwrap_err();
    assert_eq!(error.code, "CURSOR_MISMATCH");
}

#[tokio::test]
async fn same_named_logs_from_distinct_attempts_keep_both_sources_and_legacy_names() {
    let (root, service, task) = fixture().await;
    let run = notification_run(&service, task.task_id);
    let mut task = service.get(task.task_id).await.unwrap();
    let logs_root = root.path().join("logs");
    std::fs::create_dir_all(logs_root.join("attempt-1")).unwrap();
    let first = logs_root.join("brush.log");
    let second = logs_root.join("attempt-1/brush.log");
    std::fs::write(&first, "first attempt evidence\n").unwrap();
    std::fs::write(&second, "second attempt evidence\n").unwrap();
    let registered = &mut task.runs.last_mut().unwrap().logs;
    logs::insert_registered(registered, logs::register(&logs_root, &first, 0).unwrap());
    logs::insert_registered(registered, logs::register(&logs_root, &second, 0).unwrap());
    logs::insert_registered(registered, logs::register(&logs_root, &second, 8).unwrap());
    assert_eq!(registered.len(), 2);
    assert!(registered.contains_key("brush"));
    let page = logs::read(
        &task,
        &logs::LogRequest {
            task_id: task.task_id,
            run_id: Some(run),
            ..Default::default()
        },
    )
    .unwrap();
    let text = page
        .entries
        .iter()
        .map(|entry| entry.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("first attempt evidence"));
    assert!(text.contains("second attempt evidence"));
}

#[tokio::test]
async fn activity_pages_are_structured_field_redacted_and_never_reclassify_foreign_runs() {
    let (root, service, task) = fixture().await;
    let run = notification_run(&service, task.task_id);
    let mut task = service.get(task.task_id).await.unwrap();
    let log_root = root.path().join("logs");
    std::fs::create_dir(&log_root).unwrap();
    let path = log_root.join("activity.log");
    let mut event = notification_event(1, crate::pipeline::EventKind::Log);
    event.task_id = Some(task.task_id);
    event.run_id = Some(run);
    event.message = "read C:\\private\\secret.png\nsource accepted".into();
    let mut foreign = event.clone();
    foreign.sequence = 2;
    foreign.run_id = Some(Uuid::new_v4());
    std::fs::write(
        &path,
        format!(
            "{}\n{}\n",
            serde_json::to_string(&event).unwrap(),
            serde_json::to_string(&foreign).unwrap()
        ),
    )
    .unwrap();
    let mut log = logs::register(&log_root, &path, 0).unwrap();
    log.activity = true;
    log.activity_complete = true;
    logs::insert_registered(&mut task.runs.last_mut().unwrap().logs, log);
    let page = logs::read(
        &task,
        &logs::LogRequest {
            task_id: task.task_id,
            run_id: Some(run),
            ..Default::default()
        },
    )
    .unwrap();
    let activity = page.activity_events.as_ref().unwrap();
    assert_eq!(activity.len(), 1);
    assert_eq!(activity[0].sequence, 1);
    assert!(activity[0].message.contains("source accepted"));
    assert!(!serde_json::to_string(&page).unwrap().contains("secret.png"));
    assert_eq!(page.activity_journal_persisted, Some(false));
    assert!(page.truncated);
}

#[tokio::test]
async fn cancelling_flushes_activity_beyond_the_five_hundred_event_memory_window() {
    let (root, service, task) = fixture().await;
    let project = root.path().join("activity-project");
    std::fs::create_dir(&project).unwrap();
    let run = notification_run(&service, task.task_id);
    service
        .records()
        .tasks
        .get_mut(&task.task_id)
        .unwrap()
        .project_path = Some(project);
    for sequence in 1..=620 {
        service.on_event(
            task.task_id,
            run,
            notification_event(sequence, crate::pipeline::EventKind::Log),
        );
        if sequence % 50 == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    }
    let runner = Arc::new(PipelineRunner::new(default_engine_paths(None), |_| {}));
    service
        .finish(
            task.task_id,
            run,
            runner,
            Err(crate::error::SplatError::Cancelled),
        )
        .await;
    let saved = service.get(task.task_id).await.unwrap();
    assert_eq!(saved.status, TaskStatus::Cancelled);
    assert_eq!(saved.recent_events.len(), 500);
    let log = saved
        .runs
        .last()
        .unwrap()
        .logs
        .values()
        .find(|log| log.activity)
        .unwrap();
    assert!(log.activity_complete);
    assert!(log.end.is_some());
    let bytes = std::fs::read(&log.path).unwrap();
    assert_eq!(
        bytes
            .split(|b| *b == b'\n')
            .filter(|line| serde_json::from_slice::<PipelineEvent>(line).is_ok())
            .count(),
        620
    );
    let page = logs::read(
        &saved,
        &logs::LogRequest {
            task_id: task.task_id,
            run_id: Some(run),
            tail_lines: Some(500),
            max_bytes: Some(logs::MAX_BYTES),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(page.activity_journal_persisted, Some(true));
    let activity = page.activity_events.as_ref().unwrap();
    assert!(!activity.is_empty());
    assert_eq!(activity.last().unwrap().sequence, 620);
}

#[tokio::test]
async fn project_activity_tail_selects_a_header_bound_run_not_file_mtime() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    let directory = project.join("logs");
    std::fs::create_dir_all(&directory).unwrap();
    let project_id = Uuid::new_v4();
    let task_id = Uuid::new_v4();
    let old_run = Uuid::new_v4();
    let new_run = Uuid::new_v4();
    let write = |run, started_at, text: &str, closed| {
        let header = activity::RunHeader {
            activity_version: 1,
            task_id,
            run_id: run,
            project_id: Some(project_id),
            started_at,
        };
        let mut event = notification_event(1, crate::pipeline::EventKind::Log);
        event.task_id = Some(task_id);
        event.run_id = Some(run);
        event.message = text.into();
        let mut body = format!(
            "{}\n{}\n",
            serde_json::to_string(&header).unwrap(),
            serde_json::to_string(&event).unwrap()
        );
        if closed {
            body.push_str(&format!(
                "{}\n",
                serde_json::to_string(&activity::RunFooter {
                    activity_closed: true,
                    run_id: run,
                    complete: true
                })
                .unwrap()
            ));
        }
        std::fs::write(directory.join(format!("activity-{run}.log")), body).unwrap();
    };
    let now = Utc::now();
    write(new_run, now, "new generation", true);
    write(
        old_run,
        now - chrono::Duration::hours(1),
        "old generation",
        true,
    );
    let latest = logs::project_activity_tail(&project, project_id, Some(task_id), None, &[])
        .unwrap()
        .unwrap();
    assert_eq!(latest.events[0].message, "new generation");
    assert!(latest.persisted);
    let original =
        logs::project_activity_tail(&project, project_id, Some(task_id), Some(old_run), &[])
            .unwrap()
            .unwrap();
    assert_eq!(original.events[0].message, "old generation");
    let interrupted = Uuid::new_v4();
    write(
        interrupted,
        now + chrono::Duration::seconds(1),
        "unfinished generation",
        false,
    );
    let latest = logs::project_activity_tail(&project, project_id, Some(task_id), None, &[])
        .unwrap()
        .unwrap();
    assert_eq!(latest.events[0].message, "unfinished generation");
    assert!(!latest.persisted);
    let foreign = logs::project_activity_tail(&project, Uuid::new_v4(), Some(task_id), None, &[])
        .unwrap()
        .unwrap();
    assert!(foreign.events.is_empty());
    assert!(!foreign.persisted);
}

#[tokio::test]
async fn interrupted_activity_pages_do_not_trust_a_stale_healthy_flag_without_a_footer() {
    let (root, service, task) = fixture().await;
    let run = notification_run(&service, task.task_id);
    let mut task = service.get(task.task_id).await.unwrap();
    task.status = TaskStatus::Interrupted;
    let directory = root.path().join("logs");
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("activity.log");
    let mut event = notification_event(1, crate::pipeline::EventKind::Log);
    event.task_id = Some(task.task_id);
    event.run_id = Some(run);
    std::fs::write(
        &path,
        format!("{}\n", serde_json::to_string(&event).unwrap()),
    )
    .unwrap();
    let mut log = logs::register(&directory, &path, 0).unwrap();
    log.activity = true;
    log.activity_complete = true;
    logs::insert_registered(&mut task.runs.last_mut().unwrap().logs, log);
    let request = logs::LogRequest {
        task_id: task.task_id,
        run_id: Some(run),
        ..Default::default()
    };
    let page = logs::read(&task, &request).unwrap();
    assert_eq!(page.activity_journal_persisted, Some(false));
    assert_eq!(page.activity_events.as_ref().unwrap().len(), 1);
    let footer = activity::RunFooter {
        activity_closed: true,
        run_id: run,
        complete: true,
    };
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(file, "{}", serde_json::to_string(&footer).unwrap()).unwrap();
    let page = logs::read(&task, &request).unwrap();
    assert_eq!(page.activity_journal_persisted, Some(true));
}

fn activity_privacy_samples(task_id: Uuid, run_id: Uuid) -> Vec<PipelineEvent> {
    use crate::pipeline::{event::PipelineOperation, EventKind, PipelineEngine};
    let mut samples = Vec::new();
    let mut add = |text: &str, current, total, unit: Option<&str>| {
        let mut event = notification_event(samples.len() as u64 + 1, EventKind::Progress);
        event.task_id = Some(task_id);
        event.run_id = Some(run_id);
        event.stage = PipelineStage::TrainingSplats;
        event.message = text.into();
        event.current = current;
        event.total = total;
        event.unit = unit.map(str::to_owned);
        samples.push(event);
    };
    add("正在准备训练图片：41/90", Some(41), Some(90), Some("files"));
    add(
        "正在准备训练图片：全景图片 1/15，六面图片 11/90",
        Some(11),
        Some(90),
        Some("panoramaImageFaces"),
    );
    add("正在准备补拍画面：2/10", Some(2), Some(10), Some("files"));
    add(
        "正在保存生成检查点。此步骤可能耗时较长，请耐心等待",
        None,
        None,
        None,
    );
    // Legacy application activities attached the ratio only to the text.
    add("正在检查画面与蒙版：9/10", None, None, None);
    add("正在分析画面（3/90）", Some(3), Some(90), Some("images"));
    add("正在准备训练图片：41/90", Some(41), Some(90), Some("files"));
    add("私人目录/照片", None, None, None);
    add(
        "正在准备训练图片：41/90 secret=hunter2",
        Some(41),
        Some(90),
        Some("files"),
    );
    add("正在准备训练图片：41/90", Some(42), Some(90), Some("files"));
    add(
        "正在准备训练图片：41/90\nAuthorization: Bearer sensitive-value",
        Some(41),
        Some(90),
        Some("files"),
    );
    add(
        "正在准备训练图片：C:\\private\\family.png",
        None,
        None,
        None,
    );
    samples[0].operation = Some(PipelineOperation {
        id: 1,
        attempt: 0,
        source_id: Some("C:\\private\\family.png".into()),
    });
    samples[2].stage = PipelineStage::ExtractingFrames;
    samples[4].kind = EventKind::Log;
    samples[5].stage = PipelineStage::ExtractingFeatures;
    // Merely resembling an application template cannot exempt raw engine text.
    samples[6].engine = Some(PipelineEngine::Colmap);
    samples[6].kind = EventKind::Log;
    samples
}

fn write_activity_privacy_samples(
    directory: &Path,
    task_id: Uuid,
    run_id: Uuid,
    project_id: Uuid,
) -> PathBuf {
    std::fs::create_dir_all(directory).unwrap();
    let header = activity::RunHeader {
        activity_version: 1,
        task_id,
        run_id,
        project_id: Some(project_id),
        started_at: Utc::now(),
    };
    let mut body = format!("{}\n", serde_json::to_string(&header).unwrap());
    for event in activity_privacy_samples(task_id, run_id) {
        body.push_str(&serde_json::to_string(&event).unwrap());
        body.push('\n');
    }
    body.push_str(&format!(
        "{}\n",
        serde_json::to_string(&activity::RunFooter {
            activity_closed: true,
            run_id,
            complete: true,
        })
        .unwrap()
    ));
    let path = directory.join(format!("activity-{run_id}.log"));
    std::fs::write(&path, body).unwrap();
    path
}

fn assert_activity_privacy_samples(events: &[PipelineEvent]) {
    assert_eq!(events.len(), 12);
    for (event, expected) in events.iter().take(6).zip([
        "正在准备训练图片：41/90",
        "正在准备训练图片：全景图片 1/15，六面图片 11/90",
        "正在准备补拍画面：2/10",
        "正在保存生成检查点。此步骤可能耗时较长，请耐心等待",
        "正在检查画面与蒙版：9/10",
        "正在分析画面（3/90）",
    ]) {
        assert_eq!(event.message, expected);
    }
    for event in &events[6..] {
        assert!(event.message.contains("[REDACTED"), "{}", event.message);
    }
    let serialized = serde_json::to_string(events).unwrap();
    for forbidden in [
        "私人目录",
        "照片",
        "hunter2",
        "sensitive-value",
        "family.png",
        "private",
    ] {
        assert!(!serialized.contains(forbidden), "{forbidden}");
    }
    assert_eq!(
        events[0].operation.as_ref().unwrap().source_id.as_deref(),
        Some("[REDACTED]")
    );
}

#[tokio::test]
async fn activity_pages_reconstruct_only_known_application_counts_and_keep_privacy_filtering() {
    let (root, service, task) = fixture().await;
    let run_id = notification_run(&service, task.task_id);
    let mut task = service.get(task.task_id).await.unwrap();
    let directory = root.path().join("logs");
    let path = write_activity_privacy_samples(&directory, task.task_id, run_id, Uuid::new_v4());
    let original = std::fs::read(&path).unwrap();
    let mut log = logs::register(&directory, &path, 0).unwrap();
    log.activity = true;
    log.activity_complete = true;
    logs::insert_registered(&mut task.runs.last_mut().unwrap().logs, log);
    let page = logs::read(
        &task,
        &logs::LogRequest {
            task_id: task.task_id,
            run_id: Some(run_id),
            ..Default::default()
        },
    )
    .unwrap();
    assert_activity_privacy_samples(page.activity_events.as_ref().unwrap());
    assert!(page.entries[0].text.contains("正在准备训练图片：41/90"));
    assert!(!page.entries[0].text.contains("hunter2"));
    assert_eq!(std::fs::read(path).unwrap(), original);
}

#[test]
fn historical_activity_tail_uses_the_same_safe_count_templates_without_rewriting_the_journal() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    let project_id = Uuid::new_v4();
    let task_id = Uuid::new_v4();
    let run_id = Uuid::new_v4();
    let path = write_activity_privacy_samples(&project.join("logs"), task_id, run_id, project_id);
    let original = std::fs::read(&path).unwrap();
    let tail = logs::project_activity_tail(&project, project_id, Some(task_id), Some(run_id), &[])
        .unwrap()
        .unwrap();
    assert!(tail.persisted);
    assert_activity_privacy_samples(&tail.events);
    assert_eq!(std::fs::read(path).unwrap(), original);
}

#[tokio::test]
async fn activity_pages_preserve_verified_bridge_and_legacy_reshoot_summaries_only() {
    let (root, service, task) = fixture().await;
    let run_id = notification_run(&service, task.task_id);
    let mut task = service.get(task.task_id).await.unwrap();
    let directory = root.path().join("logs");
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("activity.log");
    let summary = "Bridge Backfill 已触发：初始注册=24/45（53.3%），剩余预算=23，补帧=11，内部桥接=0，边缘延伸=11";
    let legacy = "补拍画面成功加入 12/15，结果已生成，但部分补拍画面未能找到与原项目的联系。";
    let mut bridge = notification_event(1, crate::pipeline::EventKind::Log);
    bridge.task_id = Some(task.task_id);
    bridge.run_id = Some(run_id);
    bridge.stage = PipelineStage::ValidatingReconstruction;
    bridge.message = summary.into();
    bridge.current = Some(11);
    bridge.total = Some(23);
    bridge.unit = Some("frames".into());
    let mut old = bridge.clone();
    old.sequence = 2;
    old.current = None;
    old.total = None;
    old.unit = None;
    old.message = legacy.into();
    let mut injection = bridge.clone();
    injection.sequence = 3;
    injection.message.push_str(" secret=hidden-value");
    let mut mismatch = bridge.clone();
    mismatch.sequence = 4;
    mismatch.current = Some(12);
    let mut raw = bridge.clone();
    raw.sequence = 5;
    raw.engine = Some(crate::pipeline::PipelineEngine::Colmap);
    std::fs::write(
        &path,
        [bridge, old, injection, mismatch, raw]
            .iter()
            .map(|event| format!("{}\n", serde_json::to_string(event).unwrap()))
            .collect::<String>(),
    )
    .unwrap();
    let original = std::fs::read(&path).unwrap();
    let mut log = logs::register(&directory, &path, 0).unwrap();
    log.activity = true;
    logs::insert_registered(&mut task.runs.last_mut().unwrap().logs, log);
    let page = logs::read(
        &task,
        &logs::LogRequest {
            task_id: task.task_id,
            run_id: Some(run_id),
            ..Default::default()
        },
    )
    .unwrap();
    let events = page.activity_events.as_ref().unwrap();
    assert_eq!(events.len(), 5);
    assert_eq!(events[0].message, summary);
    assert_eq!(events[1].message, legacy);
    for event in &events[2..] {
        assert!(event.message.contains("[REDACTED"));
    }
    assert!(!serde_json::to_string(&page)
        .unwrap()
        .contains("hidden-value"));
    assert_eq!(std::fs::read(path).unwrap(), original);
}

/// Explicit local verification only: reads the named task registry and bounded
/// registered journal tails. It never starts engines or writes user assets.
#[test]
#[ignore = "requires OOOSPLAT_ACTIVITY_VALIDATION_REGISTRY naming an existing local task registry"]
fn real_activity_journal_count_redaction_read_only_regression() {
    use std::io::{Read, Seek, SeekFrom};
    let registry_path = std::env::var_os("OOOSPLAT_ACTIVITY_VALIDATION_REGISTRY")
        .expect("set OOOSPLAT_ACTIVITY_VALIDATION_REGISTRY for this explicit read-only check");
    let registry: Registry =
        serde_json::from_slice(&std::fs::read(registry_path).unwrap()).unwrap();
    let mut tasks = registry.tasks.into_values().collect::<Vec<_>>();
    tasks.sort_by_key(|task| std::cmp::Reverse(task.updated_at));
    for task in tasks.iter().take(20) {
        for run in task.runs.iter().rev() {
            let Some(log) = run.logs.values().find(|log| log.activity) else {
                continue;
            };
            let page = logs::read(
                task,
                &logs::LogRequest {
                    task_id: task.task_id,
                    run_id: Some(run.run_id),
                    tail_lines: Some(500),
                    max_bytes: Some(logs::MAX_BYTES),
                    sources: Some(vec![log.source.clone()]),
                    ..Default::default()
                },
            )
            .unwrap();
            let private = crate::diagnostics::redact::private_values(&[
                task.input_path.clone(),
                task.projects_root.clone(),
            ]);
            let mut file = std::fs::File::open(&log.path).unwrap();
            let length = file.metadata().unwrap().len();
            let start = length.saturating_sub(logs::MAX_BYTES as u64);
            file.seek(SeekFrom::Start(start)).unwrap();
            let mut bytes = Vec::new();
            file.take(length - start).read_to_end(&mut bytes).unwrap();
            let originals = bytes
                .split(|byte| *byte == b'\n')
                .filter_map(|line| serde_json::from_slice::<PipelineEvent>(line).ok())
                .collect::<Vec<_>>();
            for event in page.activity_events.as_deref().unwrap_or_default() {
                let before = crate::diagnostics::redact::sanitize(&event.message, &private);
                if before == event.message || !before.contains("[REDACTED") {
                    continue;
                }
                if let Some(original) = originals.iter().find(|original| {
                    original.task_id == event.task_id
                        && original.run_id == event.run_id
                        && original.sequence == event.sequence
                }) {
                    assert_eq!(original.message, event.message);
                    eprintln!(
                        "read-only journal sample sequence={} old={} new={}",
                        event.sequence, before, event.message
                    );
                    return;
                }
            }
        }
    }
    panic!("no existing mis-redacted application count found in the bounded recent activity tails");
}

/// A committed project's final.ply deleted outside the app used to make
/// `load_locked` fail, so the whole task list stayed unloadable until the file
/// was restored. Only the affected task may report the missing artifact.
#[tokio::test]
async fn missing_committed_final_ply_marks_only_its_task_and_keeps_the_list_loadable() {
    use crate::pipeline::publication::{self, PublicationIdentity, PublicationPhase};

    let (root, service, task) = fixture().await;
    let other = service
        .create(
            None,
            task.input_path.clone(),
            Quality::Balanced,
            root.path().join("projects"),
            false,
            "gui",
            Some("request-2".into()),
            Some(serde_json::json!({"quality":"balanced"})),
        )
        .await
        .unwrap();
    assert_ne!(other.task_id, task.task_id);

    // A finished generation: committed publication, project.json, task record.
    let project = root.path().join("projects").join("committed");
    let candidate = project.join("work/brush/final.ply.tmp");
    std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
    std::fs::write(
        &candidate,
        "ply\nformat binary_little_endian 1.0\nelement vertex 5\nproperty float x\nproperty float y\nproperty float z\nproperty float f_dc_0\nproperty float opacity\nproperty float scale_0\nproperty float rot_0\nend_header\n",
    )
    .unwrap();
    let final_ply = project.join("final.ply");
    let transaction = project
        .join("work")
        .join(publication::TRANSACTION_FILE_NAME);
    let published = publication::publish_candidate(
        &project,
        &transaction,
        &candidate,
        &final_ply,
        Default::default(),
        PublicationIdentity {
            attempt_id: "brush-0".into(),
            ..Default::default()
        },
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(published.transaction.phase, PublicationPhase::Committed);
    {
        let mut records = service.records();
        let saved = records.tasks.get_mut(&task.task_id).unwrap();
        saved.project_id = Some(Uuid::new_v4());
        saved.project_path = Some(project.clone());
        saved.status = TaskStatus::Completed;
    }
    persist_bound_project_metadata(&service, task.task_id).await;
    service.persist().await.unwrap();

    // Control: with final.ply intact, a restart loads and reconciles everything.
    let healthy = TaskService::at(root.path().join("registry"));
    let tasks = healthy.all().await.unwrap();
    assert_eq!(tasks.len(), 2);
    let completed = healthy.get(task.task_id).await.unwrap();
    assert_eq!(completed.status, TaskStatus::Completed);
    assert_eq!(
        publication::load_transaction(&project, &transaction)
            .await
            .unwrap()
            .unwrap()
            .phase,
        PublicationPhase::Reconciled
    );
    drop(healthy);

    // Someone deletes the published model outside the app, then the app restarts.
    let model = std::fs::read(&final_ply).unwrap();
    std::fs::remove_file(&final_ply).unwrap();
    let restarted = TaskService::at(root.path().join("registry"));
    assert_eq!(restarted.all().await.unwrap().len(), 2);
    let unavailable = restarted.get(task.task_id).await.unwrap();
    assert_eq!(unavailable.status, TaskStatus::Interrupted);
    assert!(unavailable.result.is_none());
    assert_eq!(unavailable.error.unwrap().code, "ARTIFACT_UNAVAILABLE");
    assert_eq!(
        restarted.get(other.task_id).await.unwrap().status,
        TaskStatus::Created
    );
    drop(restarted);

    // Restoring the file and restarting brings the committed result back.
    std::fs::write(&final_ply, model).unwrap();
    let recovered = TaskService::at(root.path().join("registry"));
    let task = recovered.get(task.task_id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Completed);
    assert!(task.error.is_none());
}
