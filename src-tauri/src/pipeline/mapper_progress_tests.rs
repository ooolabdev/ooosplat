//! Replay display estimates without running an engine or touching a project.
use super::*;

#[test]
fn validation_invalidation_follows_actual_colmap_operation_not_parent_timeline_stage() {
    use crate::pipeline::artifacts::ColmapWrite;
    for (mode, expected) in [
        (ObserverMode::BracketProgress, Some(ColmapWrite::Database)),
        (ObserverMode::Matching, Some(ColmapWrite::Database)),
        (ObserverMode::Mapper, Some(ColmapWrite::Reconstruction)),
        (
            ObserverMode::OrdinaryMapper,
            Some(ColmapWrite::Reconstruction),
        ),
        (
            ObserverMode::TrainingPreparation,
            Some(ColmapWrite::TrainingDataset),
        ),
        (ObserverMode::Generic, None),
    ] {
        assert_eq!(mode.colmap_write(), expected);
    }
}

fn fixture() -> (PipelineRunner, Arc<std::sync::Mutex<Vec<PipelineEvent>>>) {
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let out = events.clone();
    (
        PipelineRunner::new(
            EnginePaths::from_root(PathBuf::from("missing-engines")),
            move |event| {
                out.lock().unwrap().push(event);
            },
        ),
        events,
    )
}

fn line(observer: &ProcessObserver, value: &str) {
    observer(ProcessUpdate::Line {
        stream: crate::process::ProcessStream::Stderr,
        line: value.into(),
    });
}

#[test]
fn ordinary_estimate_reserves_validation_and_uses_max_single_model_not_sum_or_ba_time() {
    let mut estimate = MapperProgressEstimate::default();
    assert_eq!(
        estimate.observe("Registering initial image pair", 0, Some(100)),
        Some(0.0)
    );
    assert_eq!(
        estimate.observe("Global bundle adjustment", 0, Some(100)),
        Some(0.0)
    );
    assert_eq!(
        estimate.observe("Registering image #8 (num_reg_frames=40)", 40, Some(100)),
        Some(0.4 * 0.90)
    );
    assert_eq!(
        estimate.observe(
            "Retriangulation and Global bundle adjustment",
            40,
            Some(100)
        ),
        Some(0.4 * 0.90)
    );
    assert_eq!(
        estimate.observe("Initializing with image pair", 0, Some(100)),
        Some(0.4 * 0.90)
    );
    assert_eq!(
        estimate.observe("Registering image #9 (num_reg_frames=30)", 30, Some(100)),
        Some(0.4 * 0.90)
    );
    assert_eq!(
        estimate.observe("Registering image #10 (num_reg_frames=100)", 100, Some(100)),
        Some(0.90)
    );
    assert_eq!(
        estimate.observe(
            "Global bundle adjustment num_reg_frames=100",
            100,
            Some(100)
        ),
        Some(0.90)
    );
    assert!(estimate
        .observe("num_reg_frames=200", 200, Some(100))
        .is_none());
    assert!(estimate.observe("num_reg_frames=40", 40, Some(0)).is_none());
    assert!(estimate.observe("num_reg_frames=40", 40, None).is_none());
}

#[test]
fn native_estimate_replay_is_monotonic_per_process_and_retry_starts_at_zero() {
    let (runner, events) = fixture();
    let observer = runner.process_observer(
        PipelineStage::Reconstructing,
        PipelineEngine::Colmap,
        Some(100),
        ObserverMode::OrdinaryMapper,
    );
    observer(ProcessUpdate::Started { process_id: 1 });
    let start = events
        .lock()
        .unwrap()
        .iter()
        .find(|event| event.kind == EventKind::Stage)
        .unwrap()
        .clone();
    assert_eq!(start.stage_progress, Some(0.0));
    assert!(!start.indeterminate && start.current.is_none());
    line(&observer, "Registering initial image pair");
    line(&observer, "Global bundle adjustment");
    line(&observer, "Registering image #50 (num_reg_frames=40)");
    line(&observer, "Retriangulation and Global bundle adjustment");
    line(&observer, "Registering image #51 (num_reg_frames=200)");
    line(&observer, "Registering image #52 (num_reg_frames=-2)");
    line(&observer, "Initializing with image pair");
    line(&observer, "Registering image #53 (num_reg_frames=30)");
    observer(ProcessUpdate::Exited {
        executable: PathBuf::from("colmap"),
        exit_code: Some(7),
    });
    {
        let captured = events.lock().unwrap();
        let progress = captured
            .iter()
            .filter(|event| event.kind == EventKind::Progress)
            .collect::<Vec<_>>();
        assert!(progress
            .iter()
            .all(|event| !event.indeterminate && event.stage_progress.is_some()));
        assert!(progress
            .windows(2)
            .all(|pair| pair[0].stage_progress <= pair[1].stage_progress));
        let final_count = progress.last().unwrap();
        assert_eq!(final_count.current, Some(30));
        assert!((final_count.stage_progress.unwrap() - 36.0).abs() < 0.001);
        assert!(final_count.progress > 45.0 && final_count.progress < 58.0);
        assert!(progress.iter().all(|event| event.current != Some(200)));
        assert_eq!(
            captured.last().unwrap().runtime.as_ref().unwrap().phase,
            "stopped"
        );
    }
    let previous_overall = events.lock().unwrap().last().unwrap().progress;
    observer(ProcessUpdate::Started { process_id: 2 });
    let retry = events
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|event| event.kind == EventKind::Stage)
        .unwrap()
        .clone();
    assert_eq!(retry.operation.as_ref().unwrap().attempt, 2);
    assert_eq!(retry.stage_progress, Some(0.0));
    line(&observer, "Registering image #4 (num_reg_frames=20)");
    observer(ProcessUpdate::Exited {
        executable: PathBuf::from("colmap"),
        exit_code: Some(0),
    });
    let captured = events.lock().unwrap();
    let progress = captured
        .iter()
        .rev()
        .find(|event| event.kind == EventKind::Progress)
        .unwrap();
    assert!((progress.stage_progress.unwrap() - 18.0).abs() < 0.001);
    assert!(progress.progress >= previous_overall && progress.progress < 100.0);
    assert_eq!(
        captured
            .last()
            .unwrap()
            .runtime
            .as_ref()
            .unwrap()
            .process_id,
        2
    );
}

#[test]
fn erp_mixed_unknown_and_missing_totals_do_not_opt_in_via_ui_context() {
    assert_eq!(
        ObserverMode::mapper_for_projection(true),
        ObserverMode::Mapper
    );
    assert_eq!(
        ObserverMode::mapper_for_projection(false),
        ObserverMode::OrdinaryMapper
    );
    for (mode, total) in [
        (ObserverMode::Mapper, Some(100)),
        (ObserverMode::OrdinaryMapper, None),
        (ObserverMode::OrdinaryMapper, Some(0)),
    ] {
        let (runner, events) = fixture();
        // Ordinary currently selected in a mixed task is not proof that the
        // entire reconstruction is ordinary. Only the explicit mode opts in.
        runner.events.panorama_phase.store(0, Ordering::Relaxed);
        let observer = runner.process_observer(
            PipelineStage::Reconstructing,
            PipelineEngine::Colmap,
            total,
            mode,
        );
        observer(ProcessUpdate::Started { process_id: 1 });
        line(&observer, "Registering image #50 (num_reg_frames=40)");
        observer(ProcessUpdate::Exited {
            executable: PathBuf::from("colmap"),
            exit_code: Some(0),
        });
        let captured = events.lock().unwrap();
        assert!(captured
            .iter()
            .filter(|event| matches!(event.kind, EventKind::Stage | EventKind::Progress))
            .all(|event| event.indeterminate && event.stage_progress.is_none()));
    }
}

#[test]
fn known_input_without_registration_stays_at_zero_and_bad_tokens_are_not_counters() {
    let (runner, events) = fixture();
    let observer = runner.process_observer(
        PipelineStage::Reconstructing,
        PipelineEngine::Colmap,
        Some(100),
        ObserverMode::OrdinaryMapper,
    );
    observer(ProcessUpdate::Started { process_id: 1 });
    line(&observer, "Global bundle adjustment");
    observer(ProcessUpdate::Exited {
        executable: PathBuf::from("colmap"),
        exit_code: Some(0),
    });
    let captured = events.lock().unwrap();
    assert!(!captured
        .iter()
        .any(|event| event.kind == EventKind::Progress));
    assert_eq!(
        captured
            .iter()
            .find(|event| event.kind == EventKind::Stage)
            .unwrap()
            .stage_progress,
        Some(0.0)
    );
    let counter = AtomicU64::new(40);
    for invalid in ["-2", "+2", "2.5", "2tail", "200", "18446744073709551616"] {
        assert!(parse_mapper_progress(
            &format!("Registering image #9 (num_reg_frames={invalid})"),
            &counter,
            Some(100)
        )
        .is_none());
        assert_eq!(counter.load(Ordering::Relaxed), 40);
    }
}
