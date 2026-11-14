use super::*;

#[tokio::test]
async fn database_recovery_keeps_abandoned_gps_visual_only_without_reopening_source() {
    let root = tempfile::tempdir().unwrap();
    let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("project"));
    let manager = ProjectManager::for_diagnostics(root.path().into());
    let runner = PipelineRunner::new(
        EnginePaths::from_root(root.path().join("missing-engines")),
        |_| {},
    )
    .with_input_boundary(Some(root.path().join("authorized")));
    let metadata = ProjectMetadata {
        project_path: paths.project.clone(),
        ..Default::default()
    };
    let mut state = PipelineStateFile::created(Quality::Balanced);
    state.gps = Some(crate::reconstruction::gps::Checkpoint {
        status: "fallback".into(),
        fingerprint: "previous-observations".into(),
        input_identity: Some("previous-visual-model".into()),
        database_recovery_visual_only: true,
        ..crate::reconstruction::gps::Checkpoint::pending()
    });
    let mut checkpoint = InputCheckpoint::new(GenerationInputs::Materials { items: vec![] });
    let mut source = preparation_source("photos", &["source.png"], None);
    source.originals = vec![root.path().join("outside-authorization/missing.png")];
    checkpoint.sources.push(source);
    let visual = paths.colmap.join("new-visual");
    // No manifest, no source image, no engine, and even no readable model:
    // the frozen decision must return before GPS reads or launches anything.
    let before = serde_json::to_value(&state).unwrap();
    let result = runner
        .collection_gps(
            &manager,
            &paths,
            &metadata,
            &mut state,
            &mut checkpoint,
            &visual,
        )
        .await
        .unwrap();
    assert_eq!(result, visual);
    assert_eq!(serde_json::to_value(&state).unwrap(), before);
    assert!(!paths.project.exists());
    runner.process_manager.cancel();
    assert!(matches!(
        runner
            .collection_gps(
                &manager,
                &paths,
                &metadata,
                &mut state,
                &mut checkpoint,
                &visual
            )
            .await,
        Err(SplatError::Cancelled)
    ));
}

#[test]
fn collection_feature_limits_match_071_for_ordinary_and_erp_inputs() {
    for (quality, ordinary, panorama, features) in [
        (Quality::Fast, 1200, 2048, 4096),
        (Quality::Balanced, 1600, 3072, 8192),
        (Quality::High, 3200, 4096, 16384),
    ] {
        assert_eq!(
            collection_feature_limits(quality, None),
            (ordinary, features)
        );
        assert_eq!(
            collection_feature_limits(quality, Some(8192)),
            (panorama, features),
        );
        assert_eq!(
            collection_feature_limits(quality, Some(1024)),
            (1024, features),
        );
    }
}

fn matching_item(id: &str, strategy: crate::project::ImageMatchingStrategy) -> MaterialInput {
    MaterialInput {
        id: id.into(),
        source: InputSource::ImageFolder {
            path: PathBuf::from(id),
        },
        identity: Some(format!("saved-{id}")),
        projection: VideoProjection::Perspective,
        image_matching_strategy: strategy,
        camera_relation: None,
        confirmed_small_images: true,
        confirmed_image_order: true,
        frame_removal_fps: None,
    }
}

#[test]
fn single_input_matching_restores_native_071_without_crossing_source_boundaries() {
    use crate::project::ImageMatchingStrategy::{Exhaustive, Sequential};
    for strategy in [Exhaustive, Sequential] {
        let mut checkpoint = InputCheckpoint::new(GenerationInputs::Materials {
            items: vec![matching_item("first", strategy)],
        });
        checkpoint.sources.push(preparation_source(
            "first",
            &["first.png", "second.png"],
            None,
        ));
        assert_eq!(
            PipelineRunner::collection_native_matcher(&checkpoint),
            Some(strategy)
        );
        checkpoint.sources[0].projection = VideoProjection::Equirectangular;
        assert_eq!(
            PipelineRunner::collection_native_matcher(&checkpoint),
            Some(strategy)
        );
        checkpoint.sources[0].video = Some(VideoInfo {
            duration: 1.,
            width: 4,
            height: 2,
            fps: 30.,
            total_frames: 30,
            codec: "test".into(),
            rotation: 0,
            pixel_format: "rgb24".into(),
            has_alpha: false,
        });
        assert_eq!(
            PipelineRunner::collection_native_matcher(&checkpoint),
            Some(Sequential)
        );
        checkpoint
            .sources
            .push(preparation_source("__source", &["old.png"], None));
        assert_eq!(PipelineRunner::collection_native_matcher(&checkpoint), None);
        checkpoint.sources.pop();
        checkpoint.sources[0].id = "__source".into();
        assert_eq!(PipelineRunner::collection_native_matcher(&checkpoint), None);
        checkpoint.sources[0].id = "first".into();
        if let GenerationInputs::Materials { items } = &mut checkpoint.request {
            items.push(matching_item("second", strategy));
        }
        assert_eq!(PipelineRunner::collection_native_matcher(&checkpoint), None);
    }
}

#[test]
fn collection_sequential_pairs_use_native_filename_order_and_quadratic_overlap() {
    let names = (0..35)
        .rev()
        .map(|index| format!("frame_{index:04}.png"))
        .collect::<Vec<_>>();
    let mut bytes = Vec::new();
    PipelineRunner::write_collection_source_pairs(&mut bytes, &names, true).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    let first = text
        .lines()
        .filter(|line| line.starts_with("frame_0000.png "))
        .collect::<Vec<_>>();
    assert_eq!(
        first,
        [
            "frame_0000.png frame_0001.png",
            "frame_0000.png frame_0002.png",
            "frame_0000.png frame_0004.png",
            "frame_0000.png frame_0008.png",
            "frame_0000.png frame_0016.png",
            "frame_0000.png frame_0032.png",
        ]
    );
    assert!(!text
        .lines()
        .any(|line| line == "frame_0000.png frame_0003.png"));
}

#[test]
fn collection_exhaustive_pairs_preserve_native_blocks_orientation_and_unique_coverage() {
    let names = (0..90)
        .map(|index| format!("frame_{index:04}.png"))
        .collect::<Vec<_>>();
    let mut bytes = Vec::new();
    PipelineRunner::write_collection_source_pairs(&mut bytes, &names, false).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    let lines = text.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 4005);
    assert_eq!(lines[0], "frame_0000.png frame_0001.png");
    assert_eq!(lines[1224], "frame_0048.png frame_0049.png");
    assert_eq!(lines[1225], "frame_0000.png frame_0051.png");
    assert!(lines.contains(&"frame_0050.png frame_0000.png"));
    assert!(!lines.contains(&"frame_0000.png frame_0050.png"));
    let unique = lines
        .iter()
        .map(|line| {
            let (first, second) = line.split_once(' ').unwrap();
            assert_ne!(first, second);
            if first < second {
                (first, second)
            } else {
                (second, first)
            }
        })
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(unique.len(), 4005);
}

#[test]
fn collection_pair_order_uses_real_database_ids_and_refuses_missing_names() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("database.db");
    let database = rusqlite::Connection::open(&path).unwrap();
    database
        .execute_batch(
            "CREATE TABLE images(image_id INTEGER PRIMARY KEY,name TEXT UNIQUE);
        INSERT INTO images VALUES(20,'first.png'),(10,'second.png'),(30,'other-source.png');",
        )
        .unwrap();
    drop(database);
    let source = preparation_source("one", &["first.png", "second.png"], None);
    assert_eq!(
        PipelineRunner::collection_source_matching_names(&path, &source).unwrap(),
        ["second.png", "first.png"]
    );
    let missing = preparation_source("one", &["first.png", "missing.png"], None);
    assert!(PipelineRunner::collection_source_matching_names(&path, &missing).is_err());
    let duplicate = preparation_source("one", &["first.png", "first.png"], None);
    assert!(PipelineRunner::collection_source_matching_names(&path, &duplicate).is_err());
}

fn preparation_source(id: &str, names: &[&str], version: Option<u32>) -> SourceCheckpoint {
    SourceCheckpoint {
        id: id.into(),
        identity: format!("saved-{id}"),
        ordinary_image_preparation_version: version,
        camera_ids: vec![1],
        camera_assignment: None,
        projection: VideoProjection::Perspective,
        names: names.iter().map(|name| (*name).into()).collect(),
        originals: vec![],
        video: None,
        image_sequence: None,
        dimensions: vec![],
        working_dimensions: None,
        frames: None,
        panorama_records: vec![],
        adopted: 0,
        bridge_frames: vec![],
    }
}

fn preparation_state(sources: Vec<SourceCheckpoint>) -> PipelineStateFile {
    let mut state = PipelineStateFile::created_for(Quality::Fast, ProjectInputType::Images);
    state.ordinary_image_preparation_version = Some(ORDINARY_IMAGE_PREPARATION_VERSION);
    let mut inputs = InputCheckpoint::new(GenerationInputs::Materials { items: vec![] });
    inputs.ordinary_image_preparation_version = state.ordinary_image_preparation_version;
    inputs.prepared = true;
    let count = sources.iter().map(|source| source.names.len() as u64).sum();
    inputs.sources = sources;
    state.inputs = Some(inputs);
    state.frames = Some(FrameState {
        extracted_frames: Some(count),
        has_alpha: true,
        colmap_input_version: Some(COLMAP_INPUT_VERSION),
        ..Default::default()
    });
    state
}

fn write_preparation_frame(paths: &ProjectPaths, name: &str, version: Option<u32>) {
    for directory in [&paths.frames, &paths.colmap_frames, &paths.masks] {
        std::fs::create_dir_all(directory).unwrap();
    }
    let pixels = image::RgbaImage::from_fn(4, 2, |x, _| {
        image::Rgba([200, 100, 50, [0, 64, 128, 255][x as usize]])
    });
    let frame = paths.frames.join(name);
    pixels.save(&frame).unwrap();
    let mask = paths.masks.join(format!("{name}.png"));
    if version.is_some() {
        crate::video::image_sequence::write_ordinary_sfm_derivatives(
            &image::DynamicImage::ImageRgba8(pixels),
            &paths.colmap_frames.join(name),
            &mask,
            &tokio_util::sync::CancellationToken::new(),
        )
        .unwrap();
    } else {
        std::fs::copy(&frame, paths.colmap_frames.join(name)).unwrap();
        image::GrayImage::from_fn(4, 2, |x, _| image::Luma([if x < 2 { 0 } else { 255 }]))
            .save(mask)
            .unwrap();
    }
}

#[tokio::test]
async fn source_snapshot_deep_validation_keeps_old_binary_and_new_soft_derivatives_separate() {
    let root = tempfile::tempdir().unwrap();
    let paths = ProjectPaths::existing(uuid::Uuid::nil(), root.path().into());
    write_preparation_frame(&paths, "old.png", None);
    write_preparation_frame(&paths, "new.png", Some(1));
    let state = preparation_state(vec![
        preparation_source("old", &["old.png"], None),
        preparation_source("new", &["new.png"], Some(1)),
    ]);
    let original = std::fs::read(paths.frames.join("new.png")).unwrap();
    validate_current_colmap_input_snapshot(&paths.project, &state)
        .await
        .unwrap();
    let policies = ordinary_sfm_preparation_policies(&paths.project, &state).unwrap();
    assert_eq!(policies["old.png"], None);
    assert_eq!(policies["new.png"], Some(1));
    // A new task's global policy cannot make an inherited old soft Mask legal.
    image::GrayImage::from_pixel(4, 2, image::Luma([64]))
        .save(paths.masks.join("old.png.png"))
        .unwrap();
    assert!(
        validate_current_colmap_input_snapshot(&paths.project, &state)
            .await
            .is_err()
    );
    write_preparation_frame(&paths, "old.png", None);
    let mut changed = image::open(paths.colmap_frames.join("new.png"))
        .unwrap()
        .to_rgb8();
    changed.put_pixel(0, 0, image::Rgb([1, 2, 3]));
    changed.save(paths.colmap_frames.join("new.png")).unwrap();
    assert!(
        validate_current_colmap_input_snapshot(&paths.project, &state)
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(paths.frames.join("new.png")).unwrap(),
        original
    );
}

#[test]
fn source_preparation_policy_rejects_unknown_video_erp_and_unversioned_authorizations() {
    let root = tempfile::tempdir().unwrap();
    let paths = ProjectPaths::existing(uuid::Uuid::nil(), root.path().into());
    write_preparation_frame(&paths, "new.png", Some(1));
    let state = preparation_state(vec![preparation_source("new", &["new.png"], Some(1))]);
    assert!(ordinary_sfm_preparation_policies(&paths.project, &state).is_ok());
    for mutation in ["unknown", "erp", "video", "old-global", "inherited"] {
        let mut changed = state.clone();
        let inputs = changed.inputs.as_mut().unwrap();
        match mutation {
            "unknown" => inputs.sources[0].ordinary_image_preparation_version = Some(2),
            "erp" => inputs.sources[0].projection = VideoProjection::Equirectangular,
            "video" => {
                inputs.sources[0].video = Some(VideoInfo {
                    width: 4,
                    height: 2,
                    fps: 30.,
                    duration: 1.,
                    total_frames: 30,
                    codec: "test".into(),
                    rotation: 0,
                    pixel_format: "rgba".into(),
                    has_alpha: true,
                })
            }
            "old-global" => {
                changed.ordinary_image_preparation_version = None;
                inputs.ordinary_image_preparation_version = None;
            }
            "inherited" => inputs.sources[0].id = "__source".into(),
            _ => unreachable!(),
        }
        assert!(
            ordinary_sfm_preparation_policies(&paths.project, &changed).is_err(),
            "{mutation}"
        );
    }
}

#[tokio::test]
async fn inherited_preparation_policies_are_bound_to_frozen_batches_and_remain_header_only() {
    use crate::pipeline::reshoot_snapshot::{
        ReshootSnapshotBuilder, SnapshotAssetInput, SnapshotAssetRole,
    };
    let root = tempfile::tempdir().unwrap();
    let paths = ProjectPaths::existing(uuid::Uuid::nil(), root.path().into());
    for (name, version) in [
        ("old.png", None),
        ("new.png", Some(1)),
        ("third.png", Some(1)),
    ] {
        write_preparation_frame(&paths, name, version);
    }
    let batches = vec![
        FrozenReshootSourceBatch {
            id: "old-video".into(),
            identity: "old".into(),
            names: vec!["old.png".into()],
            projection: VideoProjection::Perspective,
            camera_ids: vec![1],
            panorama_records: vec![],
            video: true,
            ordinary_image_preparation_version: None,
        },
        FrozenReshootSourceBatch {
            id: "new-photos".into(),
            identity: "new".into(),
            names: vec!["new.png".into()],
            projection: VideoProjection::Perspective,
            camera_ids: vec![2],
            panorama_records: vec![],
            video: false,
            ordinary_image_preparation_version: Some(1),
        },
    ];
    assert!(serde_json::to_value(&batches[0])
        .unwrap()
        .get("ordinaryImagePreparationVersion")
        .is_none());
    let context = FrozenReshootSourceContext {
        source_identity: "source-identity".into(),
        names: vec!["old.png".into(), "new.png".into()],
        panorama_records: vec![],
        source_batches: batches,
        reshoot_eligible: true,
        generation_policy: None,
        configuration: None,
        gps: None,
        transform: Default::default(),
    };
    let context_file = root.path().join("source-context.json");
    std::fs::write(&context_file, serde_json::to_vec(&context).unwrap()).unwrap();
    let model = root.path().join("model");
    std::fs::create_dir(&model).unwrap();
    std::fs::write(
        model.join("cameras.txt"),
        "1 SIMPLE_RADIAL 4 2 4 2 1 0\n2 SIMPLE_RADIAL 4 2 4 2 1 0\n",
    )
    .unwrap();
    std::fs::write(
        model.join("images.txt"),
        "1 1 0 0 0 0 0 0 1 old.png\n\n2 1 0 0 0 0 0 0 2 new.png\n\n",
    )
    .unwrap();
    std::fs::write(
        model.join("points3D.txt"),
        "# no points needed for source policy metadata\n",
    )
    .unwrap();
    let snapshot = ReshootSnapshotBuilder::new(paths.work.join("reshoot-snapshot-v3"))
        .model(&model)
        .asset(SnapshotAssetInput {
            role: SnapshotAssetRole::Other,
            source: context_file,
            relative_path: "source/context.json".into(),
        })
        .build(&tokio_util::sync::CancellationToken::new())
        .await
        .unwrap();
    let inherited = preparation_source("__source", &["old.png", "new.png"], None);
    let mut state = preparation_state(vec![
        inherited,
        preparation_source("latest", &["third.png"], Some(1)),
    ]);
    state.reshoot = Some(crate::project::ReshootState {
        snapshot_v3_ready: true,
        snapshot_v3_identity: Some(snapshot.content_identity.clone()),
        ..Default::default()
    });
    let policies = ordinary_sfm_preparation_policies(&paths.project, &state).unwrap();
    assert_eq!(policies["old.png"], None);
    assert_eq!(policies["new.png"], Some(1));
    assert_eq!(policies["third.png"], Some(1));
    validate_current_colmap_input_snapshot(&paths.project, &state)
        .await
        .unwrap();
    let flattened = frozen_source_batches(
        &paths.project,
        &state,
        &state.inputs.as_ref().unwrap().sources[0],
    )
    .unwrap();
    assert_eq!(flattened.len(), 2);
    assert!(flattened.iter().all(|batch| batch.id != "__source"));
    assert_eq!(flattened[0].ordinary_image_preparation_version, None);
    assert_eq!(flattened[1].ordinary_image_preparation_version, Some(1));
    // Policy selection authenticates metadata, but never decodes image pixels
    // during the cheap source preflight. Deep validation still rejects damage.
    let original = std::fs::read(paths.frames.join("new.png")).unwrap();
    std::fs::write(paths.frames.join("new.png"), b"not a pixel stream").unwrap();
    assert_eq!(
        ordinary_sfm_preparation_policies(&paths.project, &state).unwrap(),
        policies
    );
    assert!(
        validate_current_colmap_input_snapshot(&paths.project, &state)
            .await
            .is_err()
    );
    std::fs::write(paths.frames.join("new.png"), original).unwrap();
    let mut changed = state.clone();
    changed.reshoot.as_mut().unwrap().snapshot_v3_identity = Some("a".repeat(64));
    assert!(ordinary_sfm_preparation_policies(&paths.project, &changed).is_err());
    changed.reshoot.as_mut().unwrap().snapshot_v3_identity = None;
    assert!(ordinary_sfm_preparation_policies(&paths.project, &changed).is_err());
    let mut changed = state;
    changed.inputs.as_mut().unwrap().sources[0].names.pop();
    assert!(ordinary_sfm_preparation_policies(&paths.project, &changed).is_err());
}

#[tokio::test]
async fn ordinary_transparent_preparation_reports_real_counts_and_preserves_source_pixels() {
    let root = tempfile::tempdir().unwrap();
    let frames = root.path().join("frames");
    let rgb = root.path().join("colmap_frames");
    let masks = root.path().join("masks");
    for path in [&frames, &rgb, &masks] {
        std::fs::create_dir(path).unwrap();
    }
    let mut originals = Vec::new();
    for index in 0..3 {
        let path = root.path().join(format!("image-{index}.png"));
        image::RgbaImage::from_fn(8, 6, |x, _| {
            image::Rgba([12, 34, 56, if x < 4 { 0 } else { 255 }])
        })
        .save(&path)
        .unwrap();
        let content = std::fs::read(&path).unwrap();
        originals.push((path, content));
    }
    let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
    let events = captured.clone();
    let runner = PipelineRunner::new(EnginePaths::from_root("not-used"), move |event| {
        events.lock().unwrap().push(event)
    });
    runner.events.source(Some("transparent-images"));
    let progress =
        runner.application_activity(PipelineStage::ExtractingFrames, "正在准备训练图片", Some(3));
    let names = prepare_images(
        &originals.iter().map(|v| v.0.clone()).collect::<Vec<_>>(),
        "batch-",
        &frames,
        &rgb,
        &masks,
        false,
        Quality::High,
        &runner.process_manager.child_token(),
        Some(&progress),
    )
    .unwrap();
    progress.finish();
    assert_eq!(names.len(), 3);
    for ((source, bytes), name) in originals.iter().zip(&names) {
        assert_eq!(&std::fs::read(source).unwrap(), bytes);
        assert_eq!(&std::fs::read(frames.join(name)).unwrap(), bytes);
        assert_eq!(&std::fs::read(rgb.join(name)).unwrap(), bytes);
        let mask = image::open(masks.join(format!("{name}.png")))
            .unwrap()
            .to_luma8();
        assert_eq!(mask.dimensions(), (8, 6));
        assert_eq!(mask.get_pixel(0, 0).0, [0]);
        assert_eq!(mask.get_pixel(7, 0).0, [255]);
    }
    let events = captured.lock().unwrap();
    assert!(events.iter().any(|event| event.message.ends_with("0/3")));
    assert!(events.iter().any(|event| event.message.ends_with("3/3")));
    assert!(events.iter().all(|event| event
        .operation
        .as_ref()
        .and_then(|op| op.source_id.as_deref())
        == Some("transparent-images")
        && event.stage == PipelineStage::ExtractingFrames
        && event.stage_progress.is_none()));
}

#[tokio::test]
async fn shared_image_preparation_records_source_before_engine_failure_and_restores_without_originals(
) {
    let root = tempfile::tempdir().unwrap();
    let images = root.path().join("images");
    std::fs::create_dir(&images).unwrap();
    for index in 0..2 {
        image::RgbImage::new(32, 24)
            .save(images.join(format!("{index}.jpg")))
            .unwrap();
    }
    let source = InputSource::ImageFolder {
        path: images.clone(),
    };
    let item = MaterialInput {
        id: "new-images".into(),
        identity: Some(source.identity().unwrap()),
        source,
        projection: VideoProjection::Perspective,
        image_matching_strategy: Default::default(),
        camera_relation: None,
        confirmed_small_images: true,
        confirmed_image_order: false,
        frame_removal_fps: None,
    };
    let manager = ProjectManager::for_diagnostics(root.path().join("projects"));
    let (paths, mut metadata) = manager
        .create_collection(
            Quality::Balanced,
            GenerationInputs::Materials { items: vec![item] },
        )
        .await
        .unwrap();
    let policy = crate::pipeline::generation_policy::GenerationPolicy::default();
    let mut state = manager.read_state(&paths.state).await.unwrap();
    state.generation_policy = Some(policy);
    let metrics = PlannerEffectivenessTracker::default();
    let (telemetry, captured) =
        crate::telemetry::TelemetryService::recording(root.path().join("telemetry.json"));
    telemetry.enable_for_test().await;
    let session = Arc::new(
        crate::telemetry::PipelineTelemetrySession::new_with_planner_evaluation(
            telemetry,
            Quality::Balanced,
            crate::telemetry::TelemetryInputType::Images,
            crate::telemetry::TelemetryRunKind::New,
            true,
            metrics.clone(),
        ),
    );
    let observe = session.clone();
    let runner = PipelineRunner::new(
        EnginePaths::from_root(root.path().join("missing-engines")),
        move |event| observe.observe(&event),
    )
    .with_effectiveness_tracker(metrics.clone());
    let acceleration = serde_json::from_value(serde_json::json!({
        "backend":"cpu","detectionState":"ready","reasonCode":"gpuReady","reason":"fixture",
        "device":null,"detectedNvidiaDeviceCount":0,
        "requirements":{"minimumDriverVersion":"580","minimumComputeCapability":"7.5"}
    }))
    .unwrap();
    let error = runner
        .run_inputs(&manager, &paths, &mut metadata, state, &acceleration)
        .await
        .unwrap_err();
    session.generation_failed(&error);
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if captured
                .lock()
                .unwrap()
                .iter()
                .any(|event| event["event"] == "planner_evaluation")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    {
        let events = captured.lock().unwrap();
        let event = events
            .iter()
            .find(|event| event["event"] == "planner_evaluation")
            .unwrap();
        assert_eq!(event["properties"]["framePlan"]["sourceWidth"], 32);
        assert_eq!(event["properties"]["framePlan"]["sourceItemCount"], 2);
        assert!(event["properties"]["finalResult"].is_null());
    }
    let mut saved = manager.read_state(&paths.state).await.unwrap();
    assert!(saved.inputs.as_ref().unwrap().prepared);
    let old_artifacts = serde_json::to_value(&saved.artifact_manifests).unwrap();
    // Observations cannot invalidate images/features or candidate recipe identities.
    saved
        .inputs
        .as_mut()
        .unwrap()
        .telemetry
        .as_mut()
        .unwrap()
        .bridge = Some(PlannerBridgeMetrics::default());
    crate::pipeline::artifacts::record_collection_artifacts(&paths, &mut saved, None).unwrap();
    assert_eq!(
        serde_json::to_value(&saved.artifact_manifests).unwrap(),
        old_artifacts
    );
    std::fs::rename(&images, root.path().join("moved-images")).unwrap();
    let restored_metrics = PlannerEffectivenessTracker::default();
    let restored = PipelineRunner::new(
        EnginePaths::from_root(root.path().join("no-engines")),
        |_| {},
    )
    .with_effectiveness_tracker(restored_metrics.clone());
    restored.restore_source_metrics(&saved);
    let frame = restored_metrics.snapshot().frame_plan.unwrap();
    assert_eq!(
        (
            frame.source_width,
            frame.source_height,
            frame.source_item_count
        ),
        (32, 24, 2)
    );
}

#[test]
fn single_new_source_observation_requires_its_saved_identity_and_ignores_the_seed() {
    let inputs: GenerationInputs = serde_json::from_value(serde_json::json!({
        "mode":"materials","items":[{"id":"new","source":{"kind":"imageFolder","path":"unused"},"identity":"new-identity"}]
    })).unwrap();
    let mut checkpoint = InputCheckpoint::new(inputs);
    let make_source = |id: &str, identity: &str| {
        serde_json::from_value::<SourceCheckpoint>(serde_json::json!({
        "id":id,"identity":identity,"projection":"perspective","names":[],"originals":[],
        "video":null,"imageSequence":{"imageCount":2,"width":32,"height":24,"hasAlpha":false,"requiresLargeSequenceConfirmation":false},
        "dimensions":[[32,24]],"frames":null
    })).unwrap()
    };
    checkpoint.sources = vec![
        make_source("__source", "old-identity"),
        make_source("new", "new-identity"),
    ];
    assert_eq!(checkpoint.single_requested_source().unwrap().id, "new");
    let mut state = PipelineStateFile::created(Quality::Balanced);
    state.inputs = Some(checkpoint.clone());
    let tracker = PlannerEffectivenessTracker::default();
    PipelineRunner::new(default_engine_paths(None), |_| {})
        .with_effectiveness_tracker(tracker.clone())
        .restore_source_metrics(&state);
    assert_eq!(tracker.snapshot().frame_plan.unwrap().source_item_count, 2);
    checkpoint.sources[1].image_sequence = None;
    checkpoint.sources[1].video = Some(VideoInfo {
        width: 1920,
        height: 1080,
        fps: 30.0,
        duration: 2.0,
        total_frames: 60,
        codec: "fixture".into(),
        rotation: 0,
        pixel_format: "rgb24".into(),
        has_alpha: false,
    });
    checkpoint.sources[1].frames = Some(FrameState {
        initial_extracted_frames: 30,
        estimated_frames: 31,
        extracted_frames: Some(33),
        sampling_fps: 15.0,
        candidate_frames: (0..3)
            .map(|source_frame_index| crate::video::PlannedFrame {
                source_frame_index,
                timestamp_seconds: source_frame_index as f64 / 30.0,
            })
            .collect(),
        ..Default::default()
    });
    state.inputs = Some(checkpoint.clone());
    let tracker = PlannerEffectivenessTracker::default();
    PipelineRunner::new(default_engine_paths(None), |_| {})
        .with_effectiveness_tracker(tracker.clone())
        .restore_source_metrics(&state);
    let frame = tracker.snapshot().frame_plan.unwrap();
    assert_eq!(frame.initial_selected_count, 30);
    assert_eq!(frame.effective_target_fps, Some(15.0));
    assert_eq!(frame.candidate_count, 3);
    if let GenerationInputs::Materials { items } = &mut checkpoint.request {
        items[0].identity = None;
    }
    assert!(checkpoint.single_requested_source().is_none());
    if let GenerationInputs::Materials { items } = &mut checkpoint.request {
        items[0].identity = Some("changed".into());
    }
    assert!(checkpoint.single_requested_source().is_none());
}

#[test]
fn source_mask_requirement_preserves_alpha_when_old_frame_fields_default_to_false() {
    let mut source = SourceCheckpoint {
        id: "video".into(),
        identity: "saved".into(),
        ordinary_image_preparation_version: None,
        camera_ids: vec![],
        camera_assignment: None,
        projection: VideoProjection::Equirectangular,
        names: vec![],
        originals: vec![],
        video: None,
        image_sequence: None,
        dimensions: vec![],
        working_dimensions: None,
        frames: Some(FrameState::default()),
        panorama_records: vec![],
        adopted: 0,
        bridge_frames: vec![],
    };
    assert!(!source_requires_feature_masks(&source));
    source.video = Some(VideoInfo {
        width: 200,
        height: 100,
        fps: 30.,
        duration: 1.,
        total_frames: 30,
        codec: "test".into(),
        rotation: 0,
        pixel_format: "rgba".into(),
        has_alpha: true,
    });
    assert!(source_requires_feature_masks(&source));
    source.video = None;
    source.image_sequence = Some(crate::video::ImageSequenceInfo {
        image_count: 2,
        width: 200,
        height: 100,
        has_alpha: true,
        requires_large_sequence_confirmation: false,
    });
    assert!(source_requires_feature_masks(&source));
    source.image_sequence = None;
    source.frames.as_mut().unwrap().has_alpha = true;
    assert!(source_requires_feature_masks(&source));
}

#[tokio::test]
async fn initial_and_bridge_feature_masks_use_only_the_current_group() {
    let root = tempfile::tempdir().unwrap();
    let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("project"));
    std::fs::create_dir_all(&paths.colmap_frames).unwrap();
    std::fs::create_dir_all(&paths.masks).unwrap();
    image::GrayImage::from_pixel(16, 8, image::Luma([255]))
        .save(paths.masks.join("alpha.png.png"))
        .unwrap();
    for (name, width, height) in [("ordinary.jpg", 16, 12), ("erp.jpg", 16, 8)] {
        image::RgbImage::new(width, height)
            .save(paths.colmap_frames.join(name))
            .unwrap();
        assert!(
            feature_masks_for_group(&paths, &[name.into()], false, Default::default())
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            feature_masks_for_group(&paths, &[name.into()], true, Default::default())
                .await
                .is_err()
        );
    }
    image::RgbImage::new(16, 8)
        .save(paths.colmap_frames.join("alpha.png"))
        .unwrap();
    assert_eq!(
        feature_masks_for_group(&paths, &["alpha.png".into()], true, Default::default())
            .await
            .unwrap(),
        Some(paths.masks)
    );
}

#[test]
fn caller_scope_only_inherits_the_bound_owned_project_not_referenced_paths() {
    let root = tempfile::tempdir().unwrap();
    let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("project"));
    std::fs::create_dir_all(&paths.source).unwrap();
    let owned = paths.source.join("input.mov");
    let external = root.path().join("external.mov");
    std::fs::write(&owned, b"owned").unwrap();
    std::fs::write(&external, b"external").unwrap();
    let runner = PipelineRunner::new(
        EnginePaths::from_root(root.path().join("no-engines")),
        |_| {},
    )
    .with_invocation_access(Some(crate::pipeline::access::InvocationAccess::restricted(
        vec![],
    )));
    assert!(runner.authorize_external_path(&owned).is_err());
    let metadata = ProjectMetadata {
        id: paths.id,
        source_path: external.clone(),
        ..Default::default()
    };
    runner.bind_project_context(&paths, &metadata);
    runner.authorize_external_path(&owned).unwrap();
    assert!(runner
        .authorize_external_path(&external)
        .unwrap_err()
        .to_string()
        .contains("PATH_NOT_AUTHORIZED"));
}

fn write_valid_brush_candidate(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        b"ply\nformat binary_little_endian 1.0\nelement vertex 1\nproperty float x\nproperty float y\nproperty float z\nproperty float f_dc_0\nproperty float opacity\nproperty float scale_0\nproperty float rot_0\nend_header\n",
    )
    .unwrap();
}

#[tokio::test]
async fn a_validated_candidate_continues_to_publication_without_engines_or_disposable_inputs() {
    let root = tempfile::tempdir().unwrap();
    let input =
        crate::reconstruction::direct_input::tests::fixture(&root.path().join("input"), false);
    let manager = ProjectManager::for_diagnostics(root.path().join("projects"));
    let (paths, mut metadata) = manager
        .create_collection(
            Quality::Balanced,
            GenerationInputs::Colmap {
                input: input.clone(),
            },
        )
        .await
        .unwrap();
    let model = read_model(&input.model).unwrap();
    let names = model
        .images
        .iter()
        .map(|image| image.pose.name.clone())
        .collect::<Vec<_>>();
    for name in &names {
        for directory in [&paths.frames, &paths.colmap_frames] {
            std::fs::copy(input.images.join(name), directory.join(name)).unwrap();
        }
    }
    model.write(&paths.colmap.join("authoritative")).unwrap();
    let dataset = paths.brush.join("dataset");
    std::fs::create_dir_all(dataset.join("images")).unwrap();
    for name in &names {
        std::fs::copy(input.images.join(name), dataset.join("images").join(name)).unwrap();
    }
    model.write(&dataset.join("sparse/0")).unwrap();
    std::fs::write(dataset.join("inputs-dataset.json"), serde_json::to_vec(&serde_json::json!([
        "frozen-dataset-key", {"path":dataset,"has_erp":false,"max_edge":64,"reference":names[0]}
    ])).unwrap()).unwrap();
    let candidate = paths.brush.join("collection-training-0/final.ply.tmp");
    write_valid_brush_candidate(&candidate);
    let info = inspect_gaussian_ply(&candidate).unwrap();
    let mut state = manager.read_state(&paths.state).await.unwrap();
    state.generation_policy = Some(Default::default());
    let checkpoint = state.inputs.as_mut().unwrap();
    checkpoint.prepared = true;
    checkpoint.imported_reconstruction = true;
    checkpoint.reference_image = Some(names[0].clone());
    checkpoint.sources.push(SourceCheckpoint {
        id: "imported".into(),
        identity: "snapshot".into(),
        ordinary_image_preparation_version: None,
        camera_ids: vec![1],
        camera_assignment: None,
        projection: VideoProjection::Perspective,
        names: names.clone(),
        originals: vec![],
        video: None,
        image_sequence: None,
        dimensions: vec![],
        working_dimensions: None,
        frames: None,
        panorama_records: vec![],
        adopted: names.len() as u64,
        bridge_frames: vec![],
    });
    state.reconstruction_complete = true;
    state.brush_complete = true;
    state.stage = PipelineStage::Failed;
    state.brush_training.dataset_identity = Some("frozen-dataset-key".into());
    state.validated_output = Some(ProjectOutput {
        final_ply: paths.project.join("final.ply"),
        file_size: info.file_size,
        splat_count: info.splat_count,
        input_images: names.len() as u64,
        registered_images: names.len() as u64,
        registered_ratio: 1.,
        points_3d: model.points.len() as u64,
    });
    crate::pipeline::artifacts::record_collection_artifacts(&paths, &mut state, None).unwrap();
    manager.write_state(&paths.state, &state).await.unwrap();
    for disposable in [&paths.frames, &paths.colmap_frames, &paths.colmap, &dataset] {
        std::fs::remove_dir_all(disposable).unwrap();
    }
    std::fs::remove_dir_all(root.path().join("input")).unwrap();
    metadata.duration_ms = Some(1000);
    metadata.started_at = Some(Utc::now() - chrono::Duration::days(3));
    let runner = PipelineRunner::new(
        EnginePaths::from_root(root.path().join("missing-engines")),
        |_| {},
    )
    .with_invocation_access(Some(crate::pipeline::access::InvocationAccess::restricted(
        vec![],
    )));
    let result = Box::pin(runner.execute_work_order(
        manager.clone(),
        paths.clone(),
        &mut metadata,
        state,
        None,
    ))
    .await
    .unwrap();
    assert_eq!(result.splat_count, info.splat_count);
    assert!(result.final_ply.is_file());
    assert!(
        candidate.is_file(),
        "The candidate remains immutable after publication"
    );
    assert!(
        result.duration_ms < 60_000,
        "Stopped time must not count as execution time"
    );
    let completed = manager.read_state(&paths.state).await.unwrap();
    assert_eq!(completed.stage, PipelineStage::Completed);
    // A subsequent recovery reconciles durable views only, even with no
    // source files, training data or engine executables available.
    let restored =
        Box::pin(runner.execute_work_order(manager, paths, &mut metadata, completed, None))
            .await
            .unwrap();
    assert_eq!(restored.splat_count, result.splat_count);
}

fn collection_focal_record() -> crate::pipeline::focal_preflight::FocalRecord {
    crate::pipeline::focal_preflight::FocalRecord {
        source_id: "photos".into(),
        group_index: 0,
        checkpoint: crate::video::temporary_png_focal_workaround::Checkpoint {
            version: 1,
            image_name: "first.png".into(),
            sha256: "a".repeat(64),
            width: 8192,
            height: 5464,
            camera_params: [22728.65980253445, 4096., 2732., 0.],
        },
        raw_focal_length: "100/1".into(),
        raw_x_resolution: "8192000/1419".into(),
        unit: 2,
    }
}

#[tokio::test]
async fn collection_focal_policy_archives_old_database_without_resetting_saved_recipe_or_pixels() {
    let root = tempfile::tempdir().unwrap();
    let manager = ProjectManager::for_diagnostics(root.path().to_path_buf());
    let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("project"));
    for path in [
        &paths.colmap,
        &paths.brush,
        &paths.frames,
        &paths.colmap_frames,
        &paths.masks,
    ] {
        std::fs::create_dir_all(path).unwrap();
        std::fs::write(path.join("marker"), b"original").unwrap();
    }
    std::fs::write(paths.project.join("final.ply"), b"published").unwrap();
    let mut state = PipelineStateFile::created(Quality::Balanced);
    state.inputs = Some(InputCheckpoint::new(GenerationInputs::Materials {
        items: vec![],
    }));
    state.inputs.as_mut().unwrap().prepared = true;
    state.inputs.as_mut().unwrap().bridge_used = true;
    state.inputs.as_mut().unwrap().expanded_matching = true;
    state.features_complete = true;
    state.matching_complete = true;
    state.reconstruction_complete = true;
    state.brush_complete = true;
    state.brush_training.resolved = Some(resolve_brush_training_preset(
        Quality::Balanced,
        true,
        Some(8192),
        1920,
        1024,
    ));
    state.brush_training.oom_retry_started = true;
    state.brush_training.oom_retry_used = true;
    state.brush_training.oom_retry_source_count = Some(500_000);
    let recipe = serde_json::to_value(&state.brush_training).unwrap();
    let runner = PipelineRunner::new(default_engine_paths(None), |_| {});
    let records = vec![collection_focal_record()];
    runner
        .persist_collection_focal_update(&manager, &paths, &mut state, records.clone())
        .await
        .unwrap();
    let checkpoint = state.inputs.as_ref().unwrap();
    assert!(checkpoint.prepared && checkpoint.bridge_used && checkpoint.expanded_matching);
    assert_eq!(checkpoint.temporary_png_focal_records, Some(records));
    assert!(checkpoint.temporary_png_focal_pending_records.is_none());
    assert!(
        !state.features_complete
            && !state.matching_complete
            && !state.reconstruction_complete
            && !state.brush_complete
    );
    assert_eq!(serde_json::to_value(&state.brush_training).unwrap(), recipe);
    assert!(state.retry_cache_pending.is_empty());
    for path in [&paths.frames, &paths.colmap_frames, &paths.masks] {
        assert_eq!(std::fs::read(path.join("marker")).unwrap(), b"original");
    }
    assert_eq!(
        std::fs::read(paths.project.join("final.ply")).unwrap(),
        b"published"
    );
    let archive = std::fs::read_dir(paths.work.join("cache-archive"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    for name in ["colmap", "brush"] {
        assert_eq!(
            std::fs::read(archive.join(name).join("marker")).unwrap(),
            b"original"
        );
    }
    let saved = serde_json::to_value(&state).unwrap();
    runner
        .apply_collection_focal_preflight(&manager, &paths, &mut state)
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(&state).unwrap(), saved);
}

#[tokio::test]
async fn focal_preparation_barrier_failure_has_atomic_context_and_moves_no_cache() {
    let root = tempfile::tempdir().unwrap();
    let manager = ProjectManager::for_diagnostics(root.path().into());
    let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("project"));
    for cache in [&paths.colmap, &paths.brush] {
        std::fs::create_dir_all(cache).unwrap();
        std::fs::write(cache.join("marker"), b"original").unwrap();
    }
    // Replacing a directory with the state file must fail. No fault injected
    // into an engine, real project, or globally shared filesystem hook.
    std::fs::create_dir(&paths.state).unwrap();
    let mut state = PipelineStateFile::created(Quality::Balanced);
    state.inputs = Some(InputCheckpoint::new(GenerationInputs::Materials {
        items: vec![],
    }));
    let runner = PipelineRunner::new(default_engine_paths(None), |_| {});
    let error = runner
        .persist_collection_focal_update(
            &manager,
            &paths,
            &mut state,
            vec![collection_focal_record()],
        )
        .await
        .unwrap_err();
    assert!(error.io_error().is_some());
    assert!(error
        .to_string()
        .contains("retry_preparation_operation=write_retry_reuse_barrier"));
    assert!(error.to_string().contains("io_substep=replace_state"));
    assert_eq!(
        runner
            .preparation_diagnostic_context()
            .unwrap()
            .safe_details()["ioSubstep"],
        "replace_state"
    );
    assert!(!paths.work.join("cache-archive").exists());
    for cache in [&paths.colmap, &paths.brush] {
        assert_eq!(std::fs::read(cache.join("marker")).unwrap(), b"original");
    }
}

#[tokio::test]
async fn collection_focal_policy_resumes_a_failed_archive_from_its_durable_intent() {
    let root = tempfile::tempdir().unwrap();
    let manager = ProjectManager::for_diagnostics(root.path().to_path_buf());
    let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("project"));
    std::fs::create_dir_all(&paths.colmap).unwrap();
    std::fs::write(paths.colmap.join("database.db"), b"old camera").unwrap();
    std::fs::write(paths.work.join("cache-archive"), b"blocked directory").unwrap();
    let mut state = PipelineStateFile::created(Quality::Balanced);
    state.inputs = Some(InputCheckpoint::new(GenerationInputs::Materials {
        items: vec![],
    }));
    state.inputs.as_mut().unwrap().prepared = true;
    state.features_complete = true;
    let runner = PipelineRunner::new(default_engine_paths(None), |_| {});
    let records = vec![collection_focal_record()];
    assert!(runner
        .persist_collection_focal_update(&manager, &paths, &mut state, records.clone())
        .await
        .is_err());
    let mut restored = manager.read_state(&paths.state).await.unwrap();
    assert!(!restored.features_complete);
    assert!(!restored.retry_cache_pending.is_empty());
    assert_eq!(
        restored
            .inputs
            .as_ref()
            .unwrap()
            .temporary_png_focal_pending_records,
        Some(records.clone())
    );
    assert!(restored
        .inputs
        .as_ref()
        .unwrap()
        .temporary_png_focal_records
        .is_none());
    std::fs::remove_file(paths.work.join("cache-archive")).unwrap();
    // No engine is installed. Recovery must use the committed policy intent,
    // not re-run a detector that may now be missing its archived base files.
    runner
        .apply_collection_focal_preflight(&manager, &paths, &mut restored)
        .await
        .unwrap();
    assert!(restored.retry_cache_pending.is_empty());
    assert_eq!(
        restored
            .inputs
            .as_ref()
            .unwrap()
            .temporary_png_focal_records,
        Some(records)
    );
    assert!(restored
        .inputs
        .as_ref()
        .unwrap()
        .temporary_png_focal_pending_records
        .is_none());
}

#[tokio::test]
#[ignore = "Bundled COLMAP + real Ballintoy pixels with derived front views; joint SfM smoke only, cancelled before training"]
async fn native_joint_erp_and_ordinary_reconstruction_from_real_pixels() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let run = repo
        .join(".cache/multi-input-native")
        .join(uuid::Uuid::new_v4().to_string());
    let erp = run.join("erp");
    let ordinary = run.join("ordinary");
    std::fs::create_dir_all(&erp).unwrap();
    std::fs::create_dir_all(&ordinary).unwrap();
    let originals =
        crate::video::list_images(Path::new("E:/GaussianSplatting/test/Ballintoy/Input")).unwrap();
    let mut hashes = vec![];
    for index in [0, 5, 10, 15] {
        let source = &originals[index];
        hashes.push((
            source.clone(),
            crate::video::panorama::fingerprint(source).unwrap(),
        ));
        std::fs::copy(source, erp.join(format!("{index:03}.jpg"))).unwrap();
        let image = image::open(source).unwrap().to_rgba8();
        let face = crate::video::panorama::render_face(
            &image,
            None,
            crate::video::panorama::FACES[0].1,
            512,
            &Default::default(),
        )
        .unwrap();
        image::DynamicImage::ImageRgba8(face)
            .to_rgb8()
            .save(ordinary.join(format!("{index:03}.png")))
            .unwrap();
    }
    let items = vec![
        ("sphere", erp, VideoProjection::Equirectangular),
        ("perspective", ordinary, VideoProjection::Perspective),
    ]
    .into_iter()
    .map(|(id, path, projection)| {
        let source = InputSource::ImageFolder { path };
        let identity = source.identity().unwrap();
        MaterialInput {
            id: id.into(),
            source,
            identity: Some(identity),
            projection,
            image_matching_strategy: Default::default(),
            camera_relation: None,
            confirmed_small_images: true,
            confirmed_image_order: false,
            frame_removal_fps: None,
        }
    })
    .collect();
    let manager = ProjectManager::for_diagnostics(run.join("projects"));
    let (paths, mut metadata) = manager
        .create_collection(Quality::Fast, GenerationInputs::Materials { items })
        .await
        .unwrap();
    let mut state = manager.read_state(&paths.state).await.unwrap();
    state.generation_policy = Some(crate::pipeline::generation_policy::GenerationPolicy {
        auto_retry_enabled: false,
        ..Default::default()
    });
    let events = Arc::new(std::sync::Mutex::new(vec![]));
    let captured = events.clone();
    let token = Arc::new(std::sync::Mutex::new(None::<ProcessManager>));
    let stop = token.clone();
    let runner = PipelineRunner::new(default_engine_paths(Some(repo.join("engines"))), move |e| {
        if e.stage == PipelineStage::TrainingSplats {
            if let Some(token) = stop.lock().unwrap().as_ref() {
                token.cancel();
            }
        }
        captured.lock().unwrap().push(e);
    });
    *token.lock().unwrap() = Some(runner.process_manager.clone());
    let acceleration = runner.verify_pipeline_engines().await.unwrap();
    let started = Instant::now();
    let result = runner
        .run_inputs(&manager, &paths, &mut metadata, state, &acceleration)
        .await;
    let model = read_model(&paths.colmap.join("authoritative")).unwrap();
    let sphere = model
        .images
        .iter()
        .filter(|i| {
            model
                .cameras
                .iter()
                .any(|c| c.id == i.pose.camera_id && c.model_id == 17)
        })
        .count();
    let normal = model.images.len() - sphere;
    for (path, hash) in hashes {
        assert_eq!(crate::video::panorama::fingerprint(&path).unwrap(), hash);
    }
    let proof = serde_json::json!({"elapsedMs":started.elapsed().as_millis(),"registeredRawImages":model.images.len(),"erpImages":sphere,"ordinaryImages":normal,"points":model.points.len(),"trainingExecuted":paths.logs.join("brush.log").is_file(),"cancelled":matches!(&result,Err(SplatError::Cancelled)),"sourceHashesUnchanged":true,"crossDeviceAcceptance":false});
    std::fs::write(
        run.join("proof.json"),
        serde_json::to_vec_pretty(&proof).unwrap(),
    )
    .unwrap();
    std::fs::write(
        run.join("events.json"),
        serde_json::to_vec_pretty(&*events.lock().unwrap()).unwrap(),
    )
    .unwrap();
    println!("{} {proof}", run.display());
    assert!(matches!(result, Err(SplatError::Cancelled)));
    assert!(
        sphere > 0 && normal > 0,
        "Both projections must register in one authoritative model"
    );
}
#[test]
fn v4_single_path_and_collection_panorama_budgets_agree_without_reinterpreting_old_policy() {
    let policy = crate::pipeline::generation_policy::GenerationPolicy::default();
    let single =
        initial_panorama_recipe(Some(policy), Quality::High, true, Some(8192), 2048, 1234).unwrap();
    let collection = policy
        .panorama_training(Quality::High, Some(8192), 2048, 1234, 1234)
        .unwrap();
    assert_eq!(single, collection);
    assert_eq!(single.preset.max_splats, Some(1234 * 40));
    let older = initial_panorama_recipe(
        Some(crate::pipeline::generation_policy::GenerationPolicy {
            version: 3,
            ..policy
        }),
        Quality::High,
        true,
        Some(8192),
        2048,
        1234,
    )
    .unwrap();
    assert_eq!(
        older,
        resolve_panorama_training_preset(Quality::High, true, Some(8192), 2048, 1234).unwrap()
    );
}

#[test]
fn undistortion_does_not_make_the_first_brush_process_a_training_retry() {
    let runner = PipelineRunner::new(default_engine_paths(None), |_| {});
    runner.events.panorama_phase.store(2, Ordering::Relaxed);
    assert_eq!(
        runner
            .events
            .begin_process(PipelineStage::TrainingSplats)
            .bound_operation
            .unwrap()
            .attempt,
        1
    );
    runner.events.panorama_phase.store(1, Ordering::Relaxed);
    assert_eq!(
        runner
            .events
            .begin_process(PipelineStage::TrainingSplats)
            .bound_operation
            .unwrap()
            .attempt,
        1
    );
    assert_eq!(
        runner
            .events
            .begin_process(PipelineStage::TrainingSplats)
            .bound_operation
            .unwrap()
            .attempt,
        2
    );
}
#[test]
fn mixed_ordinary_preparation_preserves_dimensions_alpha_and_source_bytes() {
    let root = tempfile::tempdir().unwrap();
    let frames = root.path().join("frames");
    let rgb = root.path().join("rgb");
    let masks = root.path().join("masks");
    for p in [&frames, &rgb, &masks] {
        std::fs::create_dir_all(p).unwrap();
    }
    let a = root.path().join("a.png");
    let b = root.path().join("b.jpg");
    image::RgbaImage::from_fn(20, 10, |x, _| {
        image::Rgba([128, 50, 30, if x < 10 { 0 } else { 255 }])
    })
    .save(&a)
    .unwrap();
    image::RgbImage::new(15, 21).save(&b).unwrap();
    let hashes = [
        crate::video::panorama::fingerprint(&a).unwrap(),
        crate::video::panorama::fingerprint(&b).unwrap(),
    ];
    let names = prepare_images(
        &[a.clone(), b.clone()],
        "batch_",
        &frames,
        &rgb,
        &masks,
        false,
        Quality::High,
        &Default::default(),
        None,
    )
    .unwrap();
    assert_eq!(
        image::image_dimensions(frames.join(&names[0])).unwrap(),
        (20, 10)
    );
    assert_eq!(
        image::image_dimensions(frames.join(&names[1])).unwrap(),
        (15, 21)
    );
    assert!(has_transparency(&frames, &names, &Default::default()).unwrap());
    assert!(!has_transparency(&frames, &names[1..], &Default::default()).unwrap());
    assert_eq!(crate::video::panorama::fingerprint(&a).unwrap(), hashes[0]);
    assert_eq!(crate::video::panorama::fingerprint(&b).unwrap(), hashes[1]);
    assert_eq!(
        image::open(masks.join(format!("{}.png", names[0])))
            .unwrap()
            .to_luma8()
            .get_pixel(0, 0)[0],
        0
    );
}
#[test]
fn connected_model_selection_uses_registered_sources_and_stable_ties() {
    let root = tempfile::tempdir().unwrap();
    let input =
        crate::reconstruction::direct_input::tests::fixture(&root.path().join("source"), false);
    let model = read_model(&input.model).unwrap();
    let out = root.path().join("models");
    model.write(&out.join("1")).unwrap();
    model.write(&out.join("0")).unwrap();
    let (selected, report) = best_model(&out, 10).unwrap();
    assert_eq!(selected, out.join("0"));
    assert_eq!(report.registered_images, 2);
    let mut changed = read_model(&out.join("1")).unwrap();
    changed.images[0].pose.tvec[0] = 1.;
    changed.write(&out.join("1")).unwrap();
    assert!(collection_coordinates_match(&input.model, &out.join("0")).unwrap());
    assert!(!collection_coordinates_match(&input.model, &out.join("1")).unwrap());
}

#[test]
fn reconstruction_fallback_is_persisted_as_pending_and_clears_downstream_state() {
    let mut checkpoint = InputCheckpoint::new(GenerationInputs::Materials { items: vec![] });
    checkpoint.reference_image = Some("images/reference.png".into());
    checkpoint.unused = vec!["not-adopted".into()];
    checkpoint.source_adoption = vec![SourceAdoption {
        source_id: "new-video".into(),
        planned_frames: 30,
        prepared_frames: 30,
        registered_frames: 0,
        adopted: false,
        omitted_reason: Some("not connected".into()),
    }];
    let mut state = PipelineStateFile::created(Quality::Balanced);
    state.inputs = Some(checkpoint.clone());
    state.reconstruction_complete = true;
    state.brush_complete = true;
    state.stage = PipelineStage::Completed;
    state.brush_training.oom_retry_used = true;
    state.gps = Some(crate::reconstruction::gps::Checkpoint {
        status: "complete".into(),
        ..Default::default()
    });

    prepare_collection_reconstruction_retry(&mut state, &mut checkpoint);

    assert!(!state.reconstruction_complete);
    assert!(!state.brush_complete);
    assert_eq!(state.stage, PipelineStage::Reconstructing);
    assert!(
        state.brush_training.oom_retry_used,
        "Reconstruction recovery cannot renew an already spent OOM allowance"
    );
    assert!(state.gps.is_none());
    assert!(checkpoint.reference_image.is_none());
    assert!(checkpoint.source_adoption.is_empty());
    assert!(checkpoint.unused.is_empty());
    let persisted = state.inputs.as_ref().unwrap();
    assert!(persisted.reference_image.is_none());
    assert!(persisted.source_adoption.is_empty());
    assert!(persisted.unused.is_empty());
}

#[test]
fn gps_only_anchors_frozen_cameras_for_incremental_execution_plan() {
    use crate::pipeline::execution_plan::{ReconstructionPlan, SnapshotModel};

    let root = tempfile::tempdir().unwrap();
    let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("owned"));
    let mut checkpoint = InputCheckpoint::new(GenerationInputs::Materials { items: vec![] });
    checkpoint.execution_plan = Some(crate::pipeline::execution_plan::ExecutionPlan::normal());
    assert_eq!(collection_gps_base_model(&paths, &checkpoint), None);

    let seed = SnapshotModel {
        relative_path: "reshoot-snapshot-v3/colmap/sparse".into(),
        sha256: "model".into(),
        coordinate_frame_id: "source-frame".into(),
        has_erp: false,
        fixed_images: vec![1],
        fixed_cameras: vec![2],
    };
    let plan = checkpoint.execution_plan.as_mut().unwrap();
    plan.source_model = Some(seed.clone());
    plan.reconstruction = ReconstructionPlan::Incremental { seed_model: seed };
    assert_eq!(
        collection_gps_base_model(&paths, &checkpoint),
        Some(paths.colmap.join("base-model"))
    );

    checkpoint.execution_plan.as_mut().unwrap().reconstruction = ReconstructionPlan::Full;
    assert_eq!(collection_gps_base_model(&paths, &checkpoint), None);
}

#[test]
fn execution_plan_authorizes_and_records_reconstruction_fallback() {
    use crate::pipeline::{
        execution_plan::{ExecutionPlan, ReconstructionPlan, SnapshotModel},
        reshoot::{
            CameraRelation, ModelCondition, ReshootOptions, ReshootPlan, ReshootRecipe,
            RECIPE_VERSION,
        },
    };

    let initial = ReshootPlan::resolve(ModelCondition::Appearance, true, true);
    let recipe = ReshootRecipe {
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
            image_matching_strategy: Default::default(),
        },
        initial,
        actual: initial,
        batch_id: "batch".into(),
        reconstruction_fallbacks: 0,
        training_fallbacks: 0,
        expanded_matching: false,
        source_names: vec![],
        new_names: vec![],
        warm_parameters: None,
        init_sha256: None,
        init_georeference: None,
        stored_input_identity: None,
        panorama_frames: vec![],
    };
    let seed = SnapshotModel {
        relative_path: "reshoot-snapshot-v3/colmap/sparse".into(),
        sha256: "model".into(),
        coordinate_frame_id: "source-frame".into(),
        has_erp: false,
        fixed_images: vec![1],
        fixed_cameras: vec![2],
    };
    let mut checkpoint = InputCheckpoint::new(GenerationInputs::Materials { items: vec![] });
    checkpoint.execution_plan = Some(ExecutionPlan::reshoot(&recipe, seed));
    let mut state = PipelineStateFile::created(Quality::Balanced);
    state.inputs = Some(checkpoint);
    state.reshoot = Some(ReshootState {
        recipe: Some(recipe),
        ..Default::default()
    });

    assert!(apply_execution_plan_fallback(
        &mut state,
        crate::pipeline::reshoot::AlgorithmFailure::NoNewRegistrations,
    ));
    let plan = state
        .inputs
        .as_ref()
        .unwrap()
        .execution_plan
        .as_ref()
        .unwrap();
    assert_eq!(plan.reconstruction, ReconstructionPlan::Full);
    assert_eq!(plan.fallback.reconstruction_fallbacks_used, 1);
}
#[tokio::test]
async fn continuation_ignores_requested_fps_changes_and_preserves_existing_collection_work() {
    let root = tempfile::tempdir().unwrap();
    let manager = ProjectManager::for_diagnostics(root.path().to_path_buf());
    let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("owned"));
    for p in [
        &paths.colmap,
        &paths.frames,
        &paths.colmap_frames,
        &paths.masks,
        &paths.brush,
        &paths.logs,
        &paths.source,
    ] {
        std::fs::create_dir_all(p).unwrap();
        std::fs::write(p.join("marker"), b"keep").unwrap();
    }
    std::fs::write(paths.project.join("final.ply"), b"published").unwrap();
    let input = GenerationInputs::Materials { items: vec![] };
    let mut state = PipelineStateFile::created(Quality::High);
    state.inputs = Some(InputCheckpoint::new(input));
    state.generation_policy = Some(crate::pipeline::generation_policy::GenerationPolicy::default());
    state.features_complete = true;
    state.reconstruction_complete = true;
    let saved = serde_json::to_value(&state).unwrap();
    let runner = PipelineRunner::new(default_engine_paths(None), |_| {});
    runner
        .prepare_inputs_retry(
            &manager,
            &paths,
            &mut state,
            Some(crate::pipeline::generation_policy::GenerationPolicy {
                frame_sampling_fps: Some(3.),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(&state).unwrap(), saved);
    assert!(state.features_complete && state.reconstruction_complete);
    assert_eq!(std::fs::read(paths.source.join("marker")).unwrap(), b"keep");
    assert_eq!(
        std::fs::read(paths.project.join("final.ply")).unwrap(),
        b"published"
    );
    for path in [
        &paths.colmap,
        &paths.frames,
        &paths.colmap_frames,
        &paths.masks,
        &paths.brush,
    ] {
        assert_eq!(std::fs::read(path.join("marker")).unwrap(), b"keep");
    }
    assert!(!paths.work.join("cache-archive").exists());
    assert!(state.retry_cache_pending.is_empty());
}

#[tokio::test]
async fn collection_continuation_preserves_material_settings_actual_brush_and_consumed_oom_allowance(
) {
    let root = tempfile::tempdir().unwrap();
    let manager = ProjectManager::for_diagnostics(root.path().to_path_buf());
    let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("owned"));
    for path in [&paths.project, &paths.colmap, &paths.brush] {
        std::fs::create_dir_all(path).unwrap();
    }
    std::fs::write(paths.colmap.join("database.db"), b"compatible").unwrap();
    std::fs::write(paths.brush.join("partial"), b"failed training").unwrap();
    let material = MaterialInput {
        id: "video-a".into(),
        source: InputSource::Video {
            path: root.path().join("video.mp4"),
        },
        identity: None,
        projection: VideoProjection::Perspective,
        image_matching_strategy: Default::default(),
        camera_relation: None,
        confirmed_small_images: false,
        confirmed_image_order: false,
        frame_removal_fps: Some(7.5),
    };
    let request = GenerationInputs::Materials {
        items: vec![material],
    };
    let policy = crate::pipeline::generation_policy::GenerationPolicy {
        version: 5,
        ..Default::default()
    };
    let initial =
        resolve_brush_training_preset(Quality::Balanced, true, Some(12_288), 1920, 10_000);
    let mut lowered = initial;
    lowered.preset.max_splats = Some(400_000);
    let mut state = PipelineStateFile::created(Quality::Balanced);
    state.inputs = Some(InputCheckpoint::new(request.clone()));
    state.generation_policy = Some(policy);
    state.configuration = Some((*crate::presets::pipeline_optimization_config()).clone());
    state.features_complete = true;
    state.matching_complete = true;
    state.reconstruction_complete = true;
    state.brush_training.initial_resolved = Some(initial);
    state.brush_training.resolved = Some(lowered);
    state.brush_training.oom_retry_used = true;
    state.brush_training.oom_retry_started = true;
    state.brush_training.oom_retry_source_count = Some(500_000);
    let runner = PipelineRunner::new(default_engine_paths(None), |_| {});

    runner
        .prepare_inputs_retry(&manager, &paths, &mut state, Some(policy))
        .await
        .unwrap();

    assert_eq!(
        serde_json::to_value(&state.inputs.as_ref().unwrap().request).unwrap(),
        serde_json::to_value(request).unwrap()
    );
    assert_eq!(state.generation_policy, Some(policy));
    assert!(state.features_complete && state.matching_complete);
    assert!(state.reconstruction_complete);
    assert_eq!(state.brush_training.initial_resolved, Some(initial));
    assert_eq!(state.brush_training.resolved, Some(lowered));
    assert!(state.brush_training.oom_retry_used);
    assert!(state.brush_training.oom_retry_started);
    assert_eq!(state.brush_training.oom_retry_source_count, Some(500_000));
    assert_eq!(
        std::fs::read(paths.brush.join("partial")).unwrap(),
        b"failed training"
    );
    assert!(paths.colmap.join("database.db").is_file());
}

#[tokio::test]
async fn collection_continuation_preserves_actual_reshoot_plan_and_exhausted_fallback_allowances() {
    use crate::pipeline::{
        execution_plan::{ExecutionPlan, SnapshotModel},
        reshoot::{
            ModelCondition, ReconstructionMode, ReshootOptions, ReshootPlan, ReshootRecipe,
            TrainingMode, RECIPE_VERSION,
        },
    };
    let root = tempfile::tempdir().unwrap();
    let manager = ProjectManager::for_diagnostics(root.path().to_path_buf());
    let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("owned"));
    std::fs::create_dir_all(&paths.project).unwrap();
    let recipe = ReshootRecipe {
        version: RECIPE_VERSION,
        normal_parameters_version: Some(1),
        camera_assignment: None,
        options: ReshootOptions {
            condition: ModelCondition::Good,
            camera_relation: None,
            allow_fallback: true,
            source_identity: "frozen-source".into(),
            input_identity: "frozen-input".into(),
            projection: VideoProjection::Perspective,
            image_matching_strategy: Default::default(),
        },
        initial: ReshootPlan {
            reconstruction: ReconstructionMode::Incremental,
            training: TrainingMode::Warm,
        },
        actual: ReshootPlan {
            reconstruction: ReconstructionMode::Full,
            training: TrainingMode::Cold,
        },
        batch_id: "batch-1".into(),
        reconstruction_fallbacks: 1,
        training_fallbacks: 1,
        expanded_matching: true,
        source_names: vec!["old.jpg".into()],
        new_names: vec!["new.jpg".into()],
        warm_parameters: None,
        init_sha256: None,
        init_georeference: None,
        stored_input_identity: None,
        panorama_frames: vec![],
    };
    let mut checkpoint = InputCheckpoint::new(GenerationInputs::Materials { items: vec![] });
    checkpoint.prepared = true;
    checkpoint.bridge_used = true;
    checkpoint.expanded_matching = true;
    checkpoint.execution_plan = Some(ExecutionPlan::reshoot(
        &recipe,
        SnapshotModel {
            relative_path: "reshoot-snapshot-v3/colmap/sparse".into(),
            sha256: "trusted-model".into(),
            coordinate_frame_id: "frozen-frame".into(),
            has_erp: true,
            fixed_images: vec![1],
            fixed_cameras: vec![1],
        },
    ));
    let mut state = PipelineStateFile::created(Quality::Balanced);
    state.inputs = Some(checkpoint);
    state.generation_policy = Some(crate::pipeline::generation_policy::GenerationPolicy {
        version: 5,
        ..Default::default()
    });
    state.reshoot = Some(ReshootState {
        recipe: Some(recipe),
        source_snapshot_complete: true,
        supplemental_frames_complete: true,
        incremental_reconstruction_complete: true,
        ..Default::default()
    });
    let expected = serde_json::to_value(&state).unwrap();
    let runner = PipelineRunner::new(default_engine_paths(None), |_| {});
    for _ in 0..2 {
        runner
            .prepare_inputs_retry(&manager, &paths, &mut state, Some(Default::default()))
            .await
            .unwrap();
        assert_eq!(serde_json::to_value(&state).unwrap(), expected);
    }
}

#[tokio::test]
async fn collection_publish_only_continuation_keeps_exact_candidate_and_retry_evidence() {
    let root = tempfile::tempdir().unwrap();
    let manager = ProjectManager::for_diagnostics(root.path().to_path_buf());
    let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("owned"));
    std::fs::create_dir_all(&paths.project).unwrap();
    let final_ply = paths.project.join("final.ply");
    write_valid_brush_candidate(&final_ply);
    let candidate = paths.brush.join("collection-training-0/final.ply.tmp");
    write_valid_brush_candidate(&candidate);
    let candidate_bytes = std::fs::read(&candidate).unwrap();
    let policy = crate::pipeline::generation_policy::GenerationPolicy {
        version: 5,
        ..Default::default()
    };
    let mut state = PipelineStateFile::created(Quality::High);
    state.inputs = Some(InputCheckpoint::new(GenerationInputs::Materials {
        items: vec![],
    }));
    state.generation_policy = Some(policy);
    state.brush_complete = true;
    state.brush_training.oom_retry_used = true;
    state.brush_training.oom_retry_started = true;
    state.stage = PipelineStage::Completed;
    let runner = PipelineRunner::new(default_engine_paths(None), |_| {})
        .with_failed_retry_policy(Some(Some(policy)));

    runner
        .prepare_inputs_retry(&manager, &paths, &mut state, Some(policy))
        .await
        .unwrap();

    let recovered = paths
        .brush
        .join("collection-training-0")
        .join("final.ply.tmp");
    assert!(final_ply.is_file());
    assert!(recovered.is_file());
    assert!(state.brush_complete);
    assert_eq!(std::fs::read(&candidate).unwrap(), candidate_bytes);
    assert!(!runner.finalize_manual_retry_markers(&mut state));
    assert!(state.brush_training.oom_retry_started);
    assert!(state.brush_training.oom_retry_used);
}

fn saved_collection_recipe_for_dataset_rebuild() -> PipelineStateFile {
    use crate::pipeline::{
        artifacts::{ArtifactManifest, ArtifactManifestSet, ARTIFACT_MANIFEST_VERSION},
        reshoot::{
            ModelCondition, ReconstructionMode, ReshootOptions, ReshootPlan, ReshootRecipe,
            TrainingMode, RECIPE_VERSION,
        },
        splat_budget::FineSplatBudget,
    };
    let mut state = PipelineStateFile::created(Quality::High);
    state.generation_policy = Some(Default::default());
    state.configuration = Some((*crate::presets::pipeline_optimization_config()).clone());
    let mut checkpoint = InputCheckpoint::new(GenerationInputs::Materials { items: vec![] });
    checkpoint.prepared = true;
    checkpoint.bridge_used = true;
    checkpoint.expanded_matching = true;
    state.inputs = Some(checkpoint);
    state.features_complete = true;
    state.matching_complete = true;
    state.reconstruction_complete = true;
    state.stage = PipelineStage::TrainingSplats;
    let mut initial = resolve_brush_training_preset(Quality::High, true, Some(8192), 4000, 10_464);
    initial.configured_max_splats = Some(1_248_642);
    initial.preset.max_splats = Some(1_248_642);
    let mut actual = initial;
    actual.preset.max_splats = Some(800_000);
    state.brush_training.initial_resolved = Some(initial);
    state.brush_training.resolved = Some(actual);
    state.brush_training.dataset_identity = Some("old-content-bound-dataset".into());
    state.brush_training.execution_options = Some(Default::default());
    state.brush_training.oom_retry_used = true;
    state.brush_training.oom_retry_started = true;
    state.brush_training.oom_retry_source_count = Some(1_000_000);
    state.brush_training.fine_splat_budget = Some(FineSplatBudget {
        version: 1,
        coefficient: 0.5,
        registered_images: 18,
        effective_pixels: 162_000_000,
        sparse_points: 10_464,
        tracked_points: 10_464,
        observations: 42_411,
        mean_track_length: 42_411. / 10_464.,
        requested_max_splats: 1_248_642,
        quality: None,
        training_max_resolution: None,
    });
    state.reshoot = Some(ReshootState {
        recipe: Some(ReshootRecipe {
            version: RECIPE_VERSION,
            normal_parameters_version: Some(1),
            camera_assignment: None,
            options: ReshootOptions {
                condition: ModelCondition::Good,
                camera_relation: None,
                allow_fallback: true,
                source_identity: "frozen-source".into(),
                input_identity: "frozen-input".into(),
                projection: VideoProjection::Perspective,
                image_matching_strategy: Default::default(),
            },
            initial: ReshootPlan {
                reconstruction: ReconstructionMode::Incremental,
                training: TrainingMode::Warm,
            },
            actual: ReshootPlan {
                reconstruction: ReconstructionMode::Full,
                training: TrainingMode::Cold,
            },
            batch_id: "frozen-batch".into(),
            reconstruction_fallbacks: 1,
            training_fallbacks: 1,
            expanded_matching: true,
            source_names: vec!["old.jpg".into()],
            new_names: vec!["new.jpg".into()],
            warm_parameters: None,
            init_sha256: None,
            init_georeference: None,
            stored_input_identity: Some("frozen-input".into()),
            panorama_frames: vec![],
        }),
        ..Default::default()
    });
    let entries = [
        ("prepared", "frozen-request", "trusted-pixels"),
        ("features", "trusted-pixels", "trusted-features"),
        ("matching", "trusted-features", "trusted-matches"),
        ("reconstruction", "trusted-matches", "trusted-geometry"),
    ]
    .into_iter()
    .map(|(name, upstream, identity)| {
        (
            name.into(),
            ArtifactManifest {
                attempt_id: "original-run".into(),
                upstream_identity: upstream.into(),
                expected_count: 18,
                semantic_identity: None,
                artifact_identity: identity.into(),
                files: vec![],
            },
        )
    })
    .collect();
    state.artifact_manifests = ArtifactManifestSet {
        version: ARTIFACT_MANIFEST_VERSION,
        entries,
    };
    state
}

#[test]
fn normalized_collection_rebuild_rebinds_only_dataset_identity_and_preserves_consumed_recipe() {
    let saved = saved_collection_recipe_for_dataset_rebuild();
    // Restore from JSON as a new app process would, without resetting any
    // actual recipe, initial cap, initialization or rescue allowance.
    let mut state: PipelineStateFile =
        serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
    bind_collection_training_recipe(&mut state, "rebuilt-content-bound-dataset".into(), false)
        .unwrap();
    let mut expected = saved;
    expected.brush_training.dataset_identity = Some("rebuilt-content-bound-dataset".into());
    assert_eq!(
        serde_json::to_value(&state).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
    assert!(state.reconstruction_complete);
    assert_eq!(
        state.brush_training.resolved.unwrap().preset.max_splats,
        Some(800_000)
    );
    assert!(state.brush_training.oom_retry_used && state.brush_training.oom_retry_started);
    let recipe = state.reshoot.as_ref().unwrap().recipe.as_ref().unwrap();
    assert_eq!(
        (recipe.reconstruction_fallbacks, recipe.training_fallbacks),
        (1, 1)
    );
}

#[test]
fn a_trusted_existing_training_dataset_cannot_be_silently_rebound() {
    let mut state = saved_collection_recipe_for_dataset_rebuild();
    let mut manifest = state.artifact_manifests.entries["reconstruction"].clone();
    manifest.upstream_identity = "trusted-geometry".into();
    manifest.artifact_identity = "trusted-training-dataset".into();
    state
        .artifact_manifests
        .entries
        .insert("trainingDataset".into(), manifest);
    state.inputs.as_mut().unwrap().reference_image = Some("old.jpg".into());
    let before = serde_json::to_value(&state).unwrap();
    let error =
        bind_collection_training_recipe(&mut state, "changed-dataset".into(), false).unwrap_err();
    assert!(error
        .to_string()
        .contains("Saved training recipe is incompatible"));
    assert_eq!(serde_json::to_value(&state).unwrap(), before);
}

#[test]
fn collection_dataset_rebinding_rejects_missing_stale_or_disconnected_normalization_proof() {
    for broken in [
        "legacy",
        "prepared",
        "features",
        "matching",
        "reconstruction",
        "broken-link",
        "reference",
        "complete",
        "unprepared",
        "unreconstructed",
    ] {
        let mut state = saved_collection_recipe_for_dataset_rebuild();
        match broken {
            "legacy" => state.artifact_manifests.version = 0,
            "prepared" | "features" | "matching" | "reconstruction" => {
                state.artifact_manifests.entries.remove(broken);
            }
            "broken-link" => {
                state
                    .artifact_manifests
                    .entries
                    .get_mut("reconstruction")
                    .unwrap()
                    .upstream_identity = "unknown-matches".into()
            }
            "reference" => state.inputs.as_mut().unwrap().reference_image = Some("old.jpg".into()),
            "complete" => state.brush_complete = true,
            "unprepared" => state.inputs.as_mut().unwrap().prepared = false,
            "unreconstructed" => state.reconstruction_complete = false,
            _ => unreachable!(),
        }
        let before = serde_json::to_value(&state).unwrap();
        assert!(
            bind_collection_training_recipe(&mut state, "changed-dataset".into(), false).is_err(),
            "{broken}"
        );
        assert_eq!(serde_json::to_value(&state).unwrap(), before, "{broken}");
    }
}

#[test]
fn pending_brush_archive_cannot_be_bypassed_even_after_upstream_rebuild() {
    for rebuilt in [false, true] {
        let mut state = saved_collection_recipe_for_dataset_rebuild();
        state.retry_cache_pending = vec!["brush".into()];
        let before = serde_json::to_value(&state).unwrap();
        assert!(
            bind_collection_training_recipe(&mut state, "changed-dataset".into(), rebuilt).is_err()
        );
        assert_eq!(serde_json::to_value(&state).unwrap(), before);
    }
}

#[test]
fn explicit_upstream_rebuild_preserves_the_saved_recipe_without_legacy_manifest_proof() {
    let mut state = saved_collection_recipe_for_dataset_rebuild();
    state.artifact_manifests = Default::default();
    let mut expected = state.clone();
    expected.brush_training.dataset_identity = Some("rebuilt-upstream-dataset".into());
    bind_collection_training_recipe(&mut state, "rebuilt-upstream-dataset".into(), true).unwrap();
    assert_eq!(
        serde_json::to_value(&state).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
}

#[test]
fn unchanged_dataset_identity_does_not_require_new_proof_or_initialize_a_new_recipe() {
    let mut state = saved_collection_recipe_for_dataset_rebuild();
    state.artifact_manifests = Default::default();
    let before = serde_json::to_value(&state).unwrap();
    bind_collection_training_recipe(&mut state, "old-content-bound-dataset".into(), false).unwrap();
    assert_eq!(serde_json::to_value(&state).unwrap(), before);
    let mut fresh = PipelineStateFile::created(Quality::High);
    bind_collection_training_recipe(&mut fresh, "fresh-dataset".into(), false).unwrap();
    assert!(fresh.brush_training.dataset_identity.is_none());
    assert!(fresh.brush_training.resolved.is_none());
}

#[tokio::test]
async fn invalidated_dataset_archives_then_rebinds_saved_recipe_without_rerunning_sfm() {
    use crate::pipeline::{
        artifacts::{normalize_collection, record_collection_artifacts},
        work_order::{next_action, GenerationAction, Readiness},
    };
    let root = tempfile::tempdir().unwrap();
    let input =
        crate::reconstruction::direct_input::tests::fixture(&root.path().join("input"), false);
    let manager = ProjectManager::for_diagnostics(root.path().join("projects"));
    let (paths, _) = manager
        .create_collection(
            Quality::High,
            GenerationInputs::Colmap {
                input: input.clone(),
            },
        )
        .await
        .unwrap();
    let model = read_model(&input.model).unwrap();
    let names = model
        .images
        .iter()
        .map(|image| image.pose.name.clone())
        .collect::<Vec<_>>();
    for name in &names {
        for directory in [&paths.frames, &paths.colmap_frames] {
            std::fs::copy(input.images.join(name), directory.join(name)).unwrap();
        }
    }
    let authority = paths.colmap.join("authoritative");
    model.write(&authority).unwrap();
    let geometry_identity = super::super::gps_runner::model_identity(&authority).unwrap();
    let dataset = paths.brush.join("dataset");
    std::fs::create_dir_all(dataset.join("images")).unwrap();
    for name in &names {
        std::fs::copy(input.images.join(name), dataset.join("images").join(name)).unwrap();
    }
    model.write(&dataset.join("sparse/0")).unwrap();
    std::fs::write(dataset.join("inputs-dataset.json"), serde_json::to_vec(&serde_json::json!([
        "old-content-bound-dataset", {"path":dataset,"has_erp":false,"max_edge":64,"reference":names[0],"derivation_version":2}
    ])).unwrap()).unwrap();
    let mut state = manager.read_state(&paths.state).await.unwrap();
    let saved = saved_collection_recipe_for_dataset_rebuild();
    state.generation_policy = saved.generation_policy;
    state.configuration = saved.configuration;
    state.brush_training = saved.brush_training;
    state.reconstruction_complete = true;
    let checkpoint = state.inputs.as_mut().unwrap();
    checkpoint.prepared = true;
    checkpoint.imported_reconstruction = true;
    checkpoint.bridge_used = true;
    checkpoint.expanded_matching = true;
    checkpoint.reference_image = Some(names[0].clone());
    checkpoint.sources.push(
        serde_json::from_value(serde_json::json!({
            "id":"imported", "identity":"frozen-source", "projection":"perspective",
            "names":names, "originals":[], "video":null, "frames":null
        }))
        .unwrap(),
    );
    record_collection_artifacts(&paths, &mut state, None).unwrap();
    let frozen = state.brush_training.clone();
    // The derived cache is disposable, but its frozen training recipe is not.
    std::fs::remove_file(dataset.join("inputs-dataset.json")).unwrap();
    assert!(normalize_collection(&paths, &mut state, None)
        .await
        .unwrap());
    assert_eq!(state.retry_cache_pending, ["brush"]);
    assert_eq!(state.brush_training, frozen);
    assert!(state.reconstruction_complete);
    assert!(!state
        .artifact_manifests
        .entries
        .contains_key("trainingDataset"));
    assert!(state.inputs.as_ref().unwrap().reference_image.is_none());
    let runner = PipelineRunner::new(
        EnginePaths::from_root(root.path().join("no-engines")),
        |_| {},
    );
    runner
        .prepare_inputs_retry(&manager, &paths, &mut state, None)
        .await
        .unwrap();
    assert!(state.retry_cache_pending.is_empty());
    assert!(!paths.brush.exists());
    assert!(paths.work.join("cache-archive").is_dir());
    assert_eq!(
        next_action(
            &state,
            Readiness {
                reconstruction: true,
                ..Default::default()
            }
        ),
        GenerationAction::PrepareTraining
    );
    bind_collection_training_recipe(
        &mut state,
        "regenerated-content-bound-dataset".into(),
        false,
    )
    .unwrap();
    let mut expected = frozen;
    expected.dataset_identity = Some("regenerated-content-bound-dataset".into());
    assert_eq!(state.brush_training, expected);
    assert_eq!(
        super::super::gps_runner::model_identity(&authority).unwrap(),
        geometry_identity
    );
    assert_eq!(
        next_action(
            &state,
            Readiness {
                reconstruction: true,
                training_dataset: true,
                candidate: false
            }
        ),
        GenerationAction::Train
    );
    assert!(!paths.logs.join("colmap.log").exists());
    assert!(!paths.logs.join("brush.log").exists());
}
