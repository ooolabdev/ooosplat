use super::*;
use crate::pipeline::runner::default_engine_paths;

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
    drop(receipt);
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
    task.run_id = Some(run);
    task.runs.push(TaskRun {
        run_id: run,
        kind: "new".into(),
        started_at: Utc::now(),
        ended_at: None,
        logs: BTreeMap::from([("brush".into(), logs::register(&logroot, &path, 0).unwrap())]),
        last_engine: None,
        last_exit_code: None,
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
    task.run_id = Some(run);
    task.runs.push(TaskRun {
        run_id: run,
        kind: "new".into(),
        started_at: Utc::now(),
        ended_at: Some(Utc::now()),
        logs: BTreeMap::from([("brush".into(), logs::register(&logroot, &path, 0).unwrap())]),
        last_engine: None,
        last_exit_code: None,
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
    if link.is_ok() {
        assert_eq!(
            logs::register(root.path(), &root.path().join("brush.log"), 0)
                .unwrap_err()
                .code,
            "LOG_ACCESS_DENIED"
        );
    }
}
