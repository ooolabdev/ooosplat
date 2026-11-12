//! Producer-level display regressions; no engine, images or project are opened.
use super::*;

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

fn report(observer: &ProcessObserver, current: u64, total: u64) {
    observer(ProcessUpdate::Line {
        stream: crate::process::ProcessStream::Stderr,
        line: format!("[native prefix] Processed file [{current}/{total}]"),
    });
}

#[test]
fn ninety_independent_photo_groups_publish_global_baselines_not_ninety_retries() {
    let (runner, events) = fixture();
    let scope =
        FeatureProgressScope::new((0..90).map(|index| (format!("collection:photos:{index}"), 1)));
    runner.events.source(Some("photos"));
    for index in 0..90 {
        let observer = runner.feature_process_observer(
            PipelineStage::ExtractingFeatures,
            &scope,
            format!("collection:photos:{index}"),
        );
        observer(ProcessUpdate::Started {
            process_id: index as u32 + 1,
        });
        report(&observer, 1, 1);
        observer(ProcessUpdate::Exited {
            executable: PathBuf::from("colmap"),
            exit_code: Some(0),
        });
    }
    let events = events.lock().unwrap();
    let starts = events
        .iter()
        .filter(|event| event.kind == EventKind::Stage)
        .collect::<Vec<_>>();
    assert_eq!(starts.len(), 90);
    for (index, start) in starts.iter().enumerate() {
        assert_eq!((start.current, start.total), (Some(index as u64), Some(90)));
        assert!(!start.indeterminate);
        assert!((start.stage_progress.unwrap() - index as f32 / 90.0 * 100.0).abs() < 0.001);
        let operation = start.operation.as_ref().unwrap();
        assert_eq!(operation.attempt, 1);
        assert_eq!(operation.source_id.as_deref(), Some("photos"));
        if index > 0 {
            assert!(operation.id > starts[index - 1].operation.as_ref().unwrap().id);
        }
    }
    let progress = events
        .iter()
        .filter(|event| event.kind == EventKind::Progress)
        .collect::<Vec<_>>();
    assert!(progress
        .iter()
        .all(|event| event.engine == Some(PipelineEngine::System)
            && event.total == Some(90)
            && event.unit.as_deref() == Some("images")));
    for current in 1..=90 {
        assert!(progress
            .iter()
            .any(|event| event.message == format!("正在分析画面（{current}/90）")));
    }
    assert!(progress
        .iter()
        .all(|event| event.stage_progress.unwrap() < 100.0));
}

#[test]
fn video_erp_mixed_and_reshoot_sources_share_only_their_new_raw_image_total() {
    for specs in [
        vec![("video", 4)],
        vec![("erp-video", 5)],
        vec![("erp-images", 3)],
        vec![("video-a", 3), ("video-b", 4)],
        vec![("photo-a", 1), ("photo-b", 1), ("video", 4), ("erp", 3)],
        vec![("reshoot-new-photo", 1), ("reshoot-new-video", 4)],
    ] {
        let (runner, events) = fixture();
        let total = specs.iter().map(|(_, count)| count).sum::<u64>();
        let scope = FeatureProgressScope::new(
            specs
                .iter()
                .map(|(source, count)| (format!("collection:{source}:0"), *count)),
        );
        let mut completed = 0;
        for (index, (source, count)) in specs.iter().enumerate() {
            runner.events.source(Some(source));
            let observer = runner.feature_process_observer(
                PipelineStage::ExtractingFeatures,
                &scope,
                format!("collection:{source}:0"),
            );
            observer(ProcessUpdate::Started {
                process_id: index as u32 + 1,
            });
            let start = events
                .lock()
                .unwrap()
                .iter()
                .rev()
                .find(|event| event.kind == EventKind::Stage)
                .unwrap()
                .clone();
            assert_eq!((start.current, start.total), (Some(completed), Some(total)));
            assert_eq!(start.operation.as_ref().unwrap().attempt, 1);
            report(&observer, *count, *count);
            completed += count;
            assert_eq!(
                events
                    .lock()
                    .unwrap()
                    .iter()
                    .rev()
                    .find(|event| event.kind == EventKind::Progress)
                    .unwrap()
                    .current,
                Some(completed)
            );
        }
        // Scope contains only requested original pictures, never old __source
        // or an ERP-to-six-face multiplication.
        assert!(events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event.kind == EventKind::Progress)
            .all(|event| event.total == Some(total)));
    }
}

#[test]
fn only_a_real_same_group_retry_increments_attempt_and_late_groups_cannot_change_the_sum() {
    let (runner, events) = fixture();
    let scope =
        FeatureProgressScope::new([("collection:a:0".into(), 2), ("collection:b:0".into(), 1)]);
    runner.events.source(Some("a"));
    let first = runner.feature_process_observer(
        PipelineStage::ExtractingFeatures,
        &scope,
        "collection:a:0".into(),
    );
    first(ProcessUpdate::Started { process_id: 1 });
    report(&first, 1, 2);
    let retry = runner.feature_process_observer(
        PipelineStage::ExtractingFeatures,
        &scope,
        "collection:a:0".into(),
    );
    retry(ProcessUpdate::Started { process_id: 2 });
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|event| event.kind == EventKind::Stage)
            .unwrap()
            .operation
            .as_ref()
            .unwrap()
            .attempt,
        2
    );
    let before = events.lock().unwrap().len();
    report(&first, 2, 2);
    assert_eq!(events.lock().unwrap().len(), before);
    report(&retry, 1, 99); // known marker but wrong group is not evidence
    report(&retry, 2, 2);
    runner.events.source(Some("b"));
    let next = runner.feature_process_observer(
        PipelineStage::ExtractingFeatures,
        &scope,
        "collection:b:0".into(),
    );
    next(ProcessUpdate::Started { process_id: 3 });
    let start = events
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|event| event.kind == EventKind::Stage)
        .unwrap()
        .clone();
    assert_eq!((start.current, start.total), (Some(2), Some(3)));
    assert_eq!(start.operation.as_ref().unwrap().attempt, 1);
    report(&retry, 2, 2);
    report(&next, 1, 1);
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|event| event.kind == EventKind::Progress)
            .unwrap()
            .current,
        Some(3)
    );
}

#[test]
fn silent_validated_groups_advance_but_success_exit_error_and_cancel_do_not_fill_counts() {
    let (runner, events) = fixture();
    let scope = FeatureProgressScope::new([("cached".into(), 2), ("new".into(), 1)]);
    let cached =
        runner.feature_process_observer(PipelineStage::ExtractingFeatures, &scope, "cached".into());
    cached(ProcessUpdate::Started { process_id: 1 });
    cached(ProcessUpdate::Exited {
        executable: PathBuf::from("colmap"),
        exit_code: Some(0),
    });
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|event| event.kind == EventKind::Progress)
            .unwrap()
            .current,
        Some(0)
    );
    // Production calls this only after extract_preflighted_group's existing
    // per-group database validation has returned successfully.
    runner.complete_feature_group(PipelineStage::ExtractingFeatures, &scope, "cached".into());
    let next =
        runner.feature_process_observer(PipelineStage::ExtractingFeatures, &scope, "new".into());
    next(ProcessUpdate::Started { process_id: 2 });
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|event| event.kind == EventKind::Stage)
            .unwrap()
            .current,
        Some(2)
    );
    next(ProcessUpdate::Line {
        stream: crate::process::ProcessStream::Stderr,
        line: "ERROR: Processed file [1/1] failed".into(),
    });
    next(ProcessUpdate::Exited {
        executable: PathBuf::from("colmap"),
        exit_code: Some(1),
    });
    assert!(!events
        .lock()
        .unwrap()
        .iter()
        .any(|event| event.current == Some(3)));
    runner.process_manager.cancel();
    runner.complete_feature_progress(PipelineStage::ExtractingFeatures, &scope);
    assert!(!events
        .lock()
        .unwrap()
        .iter()
        .any(|event| event.current == Some(3)));
}

#[test]
fn old_single_process_bridge_and_non_feature_retry_semantics_are_preserved() {
    let (runner, events) = fixture();
    let initial = runner.process_observer(
        PipelineStage::ExtractingFeatures,
        PipelineEngine::Colmap,
        Some(90),
        ObserverMode::BracketProgress,
    );
    initial(ProcessUpdate::Started { process_id: 1 });
    report(&initial, 90, 90);
    let bridge = runner.process_observer(
        PipelineStage::ValidatingReconstruction,
        PipelineEngine::Colmap,
        Some(18),
        ObserverMode::BracketProgress,
    );
    bridge(ProcessUpdate::Started { process_id: 2 });
    let start = events
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|event| event.kind == EventKind::Stage)
        .unwrap()
        .clone();
    assert_eq!((start.current, start.total), (Some(0), Some(18)));
    assert_eq!(start.operation.unwrap().attempt, 1);
    report(&bridge, 18, 18);
    let map = runner.process_observer(
        PipelineStage::Reconstructing,
        PipelineEngine::Colmap,
        None,
        ObserverMode::Mapper,
    );
    map(ProcessUpdate::Started { process_id: 3 });
    let fallback = runner.process_observer(
        PipelineStage::Reconstructing,
        PipelineEngine::Colmap,
        None,
        ObserverMode::Mapper,
    );
    fallback(ProcessUpdate::Started { process_id: 4 });
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|event| event.kind == EventKind::Stage)
            .unwrap()
            .operation
            .as_ref()
            .unwrap()
            .attempt,
        2
    );
}
