use super::*;
use crate::pipeline::runner::default_engine_paths;

type RecordedUpdates = Arc<SyncMutex<Vec<TaskUpdate>>>;

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
        started_at: Utc::now(),
        ended_at: None,
        logs: Default::default(),
        last_engine: None,
        last_exit_code: None,
        error: None,
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
    let mut newer = notification_event(10, crate::pipeline::EventKind::Progress);
    newer.stage = PipelineStage::TrainingSplats;
    newer.current = Some(10);
    newer.total = Some(100);
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
    *service.executor.lock().unwrap() = Some(Arc::new(move |_| {
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
    *service.executor.lock().unwrap() = Some(Arc::new(move |task| {
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
    *service.executor.lock().unwrap() = Some(Arc::new(|_| {
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
        started_at: Utc::now(),
        ended_at: None,
        logs: BTreeMap::from([("brush".into(), logs::register(&logroot, &path, 0).unwrap())]),
        last_engine: None,
        last_exit_code: None,
        error: None,
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
        started_at: Utc::now(),
        ended_at: Some(Utc::now()),
        logs: BTreeMap::from([("brush".into(), logs::register(&logroot, &path, 0).unwrap())]),
        last_engine: None,
        last_exit_code: None,
        error: None,
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
    *service.executor.lock().unwrap() = Some(Arc::new(move |_| {
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
