use super::*;
use rmcp::{
    transport::{
        streamable_http_client::StreamableHttpClientTransportConfig, StreamableHttpClientTransport,
    },
    ServiceExt,
};

#[tokio::test]
async fn official_client_token_free_http_smoke_and_local_request_rejections() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("clip.mp4");
    std::fs::write(&input, b"fixture, not a real video").unwrap();
    let tasks = TaskService::at(temp.path().join("registry"));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}/mcp");
    let release = Arc::new(tokio::sync::Notify::new());
    let worker_release = release.clone();
    *tasks.executor.lock().unwrap() = Some(Arc::new(move |_| {
        let release = worker_release.clone();
        Box::pin(async move {
            release.notified().await;
            Err(crate::error::SplatError::Process(
                "controlled protocol fixture failure; no GPU executed".into(),
            ))
        })
    }));
    let stop = CancellationToken::new();
    let tools = McpTools {
        tasks: tasks.clone(),
        engines: crate::pipeline::runner::default_engine_paths(Some(
            temp.path().join("missing-engines"),
        )),
        settings: McpSettings {
            enabled: true,
            port,
            input_roots: vec![std::fs::canonicalize(temp.path()).unwrap()],
        },
        telemetry: None,
    };
    let app = router(tools, LocalRequestGuard { port }, stop.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let http = reqwest::Client::new();
    assert_eq!(
        http.post(&url)
            .header("Origin", "https://untrusted.example")
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        http.post(&url)
            .header("Host", "untrusted.example")
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let transport = StreamableHttpClientTransport::with_client(
        reqwest13::Client::new(),
        StreamableHttpClientTransportConfig::with_uri(url.clone()),
    );
    let client = ().serve(transport).await.unwrap();
    let definitions = client.list_tools(None).await.unwrap();
    assert_eq!(definitions.tools.len(), 7);
    assert!(definitions.tools.iter().all(|t| t.output_schema.is_some()));
    let app_status = client
        .call_tool(CallToolRequestParams::new("get_app_status"))
        .await
        .unwrap();
    assert_eq!(app_status.is_error, Some(false));
    assert_eq!(
        app_status.structured_content.as_ref().unwrap()["authorized_input_roots"][0],
        serde_json::to_value(std::fs::canonicalize(temp.path()).unwrap()).unwrap()
    );
    let arguments =
        json!({"input_path":input,"quality":"fast","client_request_id":"protocol-request-1"});
    let created = client
        .call_tool(
            CallToolRequestParams::new("create_generation_task")
                .with_arguments(arguments.as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_eq!(created.is_error, Some(false), "{created:#?}");
    let task_id = created.structured_content.as_ref().unwrap()["task_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let duplicate = client
        .call_tool(
            CallToolRequestParams::new("create_generation_task")
                .with_arguments(arguments.as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        duplicate.structured_content.as_ref().unwrap()["task_id"],
        task_id
    );
    let args = json!({"task_id":task_id});
    let started = client
        .call_tool(
            CallToolRequestParams::new("start_task")
                .with_arguments(args.as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_eq!(started.is_error, Some(false));
    let run_id = started.structured_content.as_ref().unwrap()["run_id"].clone();
    assert_eq!(
        started.structured_content.as_ref().unwrap()["accepted"],
        true
    );
    client.cancel().await.unwrap(); // Explicitly disconnect while execution is still blocked.
    assert!(tasks
        .get(Uuid::parse_str(&task_id).unwrap())
        .await
        .unwrap()
        .status
        .active());
    release.notify_one();
    let terminal = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tasks.wait(Uuid::parse_str(&task_id).unwrap()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(terminal.status, crate::tasks::TaskStatus::Failed);
    assert!(terminal.error.is_some());
    let reconnect = StreamableHttpClientTransport::with_client(
        reqwest13::Client::new(),
        StreamableHttpClientTransportConfig::with_uri(url.clone()),
    );
    let client = ().serve(reconnect).await.unwrap();
    for name in ["get_task_status", "read_task_logs"] {
        let result = client
            .call_tool(
                CallToolRequestParams::new(name).with_arguments(args.as_object().unwrap().clone()),
            )
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(false), "{name}");
    }
    assert_eq!(
        client
            .call_tool(CallToolRequestParams::new("list_tasks"))
            .await
            .unwrap()
            .is_error,
        Some(false)
    );
    let cancel = json!({"task_id":task_id,"run_id":run_id});
    assert_eq!(
        client
            .call_tool(
                CallToolRequestParams::new("cancel_task")
                    .with_arguments(cancel.as_object().unwrap().clone())
            )
            .await
            .unwrap()
            .is_error,
        Some(false)
    );
    let unauthorized = json!({"input_path":temp.path().parent().unwrap(),"quality":"high","client_request_id":"outside"});
    let rejection = client
        .call_tool(
            CallToolRequestParams::new("create_generation_task")
                .with_arguments(unauthorized.as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_eq!(rejection.is_error, Some(true));
    assert_eq!(
        rejection.structured_content.unwrap()["error"]["code"],
        "PATH_NOT_AUTHORIZED"
    );
    let invalid_cursor = client
        .call_tool(
            CallToolRequestParams::new("read_task_logs").with_arguments(
                json!({"task_id":task_id,"run_id":run_id,"cursor":"invalid-cursor"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(invalid_cursor.is_error, Some(true));
    assert_eq!(
        invalid_cursor.structured_content.unwrap()["error"]["code"],
        "INVALID_CURSOR"
    );
    // A token left in an older client configuration is ignored, not required.
    let legacy_transport = StreamableHttpClientTransport::with_client(
        reqwest13::Client::new(),
        StreamableHttpClientTransportConfig::with_uri(url).auth_header("obsolete-client-token"),
    );
    let legacy = ().serve(legacy_transport).await.unwrap();
    assert_eq!(
        legacy
            .call_tool(CallToolRequestParams::new("get_app_status"))
            .await
            .unwrap()
            .is_error,
        Some(false)
    );
    legacy.cancel().await.unwrap();
    // Turning off the HTTP entry point also leaves a second accepted execution alive.
    let created=client.call_tool(CallToolRequestParams::new("create_generation_task").with_arguments(json!({"input_path":input,"quality":"fast","client_request_id":"protocol-request-2"}).as_object().unwrap().clone())).await.unwrap();
    let second = created.structured_content.unwrap()["task_id"]
        .as_str()
        .unwrap()
        .to_owned();
    client
        .call_tool(
            CallToolRequestParams::new("start_task")
                .with_arguments(json!({"task_id":second}).as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    client.cancel().await.unwrap();
    stop.cancel();
    assert!(tasks
        .get(Uuid::parse_str(&second).unwrap())
        .await
        .unwrap()
        .status
        .active());
    release.notify_one();
    tasks.wait(Uuid::parse_str(&second).unwrap()).await.unwrap();
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn default_input_directory_is_created_authorized_and_kept_separate_from_saved_scopes() {
    let temp = tempfile::tempdir().unwrap();
    let projects = temp.path().join("Projects");
    let custom = temp.path().join("UserMedia");
    std::fs::create_dir(&custom).unwrap();
    let settings = McpSettings {
        input_roots: vec![std::fs::canonicalize(&custom).unwrap()],
        ..Default::default()
    };
    let (default, effective) = with_default_input_root(&settings, &projects).await.unwrap();
    assert_eq!(
        default,
        std::fs::canonicalize(projects.join("Inputs")).unwrap()
    );
    assert!(default.is_dir());
    assert_eq!(settings.input_roots.len(), 1);
    assert_eq!(effective.input_roots.len(), 2);
    let video = default.join("clip.mov");
    std::fs::write(&video, b"fixture").unwrap();
    assert!(authorize_input(&video, &effective.input_roots).is_ok());
    let outside = projects.join("private.mov");
    std::fs::write(&outside, b"fixture").unwrap();
    assert_eq!(
        authorize_input(&outside, &effective.input_roots)
            .unwrap_err()
            .code,
        "PATH_NOT_AUTHORIZED"
    );
    let (_, again) = with_default_input_root(&settings, &projects).await.unwrap();
    assert_eq!(effective.input_roots, again.input_roots);
    let (moved, updated) = with_default_input_root(&settings, &temp.path().join("OtherProjects"))
        .await
        .unwrap();
    assert!(updated.input_roots.contains(&moved));
    assert!(!updated.input_roots.contains(&default));
}

#[tokio::test]
async fn default_input_directory_rejects_relative_roots() {
    assert_eq!(
        with_default_input_root(
            &McpSettings::default(),
            std::path::Path::new("relative-projects")
        )
        .await
        .unwrap_err()
        .code,
        "INVALID_PATH"
    );
}

#[tokio::test]
async fn default_input_directory_rejects_redirected_folders() {
    let temp = tempfile::tempdir().unwrap();
    let projects = temp.path().join("Projects");
    let outside = temp.path().join("Outside");
    std::fs::create_dir(&projects).unwrap();
    std::fs::create_dir(&outside).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&outside, projects.join("Inputs"))
        .expect("This test needs permission for its temporary symlink fixture");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, projects.join("Inputs")).unwrap();
    assert_eq!(
        with_default_input_root(&McpSettings::default(), &projects)
            .await
            .unwrap_err()
            .code,
        "INVALID_INPUT_ROOT"
    );
}

#[test]
fn local_request_guard_keeps_host_and_origin_checks_without_credentials() {
    let guard = LocalRequestGuard { port: 39877 };
    for host in ["127.0.0.1:39877", "localhost:39877"] {
        for origin in [
            None,
            Some("http://127.0.0.1:39877"),
            Some("http://localhost:39877"),
        ] {
            let mut request = Request::builder()
                .header("Host", host)
                .header("Authorization", "Bearer old-unused-token");
            if let Some(origin) = origin {
                request = request.header("Origin", origin);
            }
            assert!(local_request_allowed(
                &request.body(axum::body::Body::empty()).unwrap(),
                &guard
            ));
        }
    }
    for (host, origin) in [
        ("untrusted.example", "http://127.0.0.1:39877"),
        ("127.0.0.1:39878", "http://127.0.0.1:39877"),
        ("127.0.0.1:39877", "https://untrusted.example"),
        ("127.0.0.1:39877", "null"),
    ] {
        let request = Request::builder()
            .header("Host", host)
            .header("Origin", origin)
            .body(axum::body::Body::empty())
            .unwrap();
        assert!(!local_request_allowed(&request, &guard));
    }
    assert!(!local_request_allowed(
        &Request::new(axum::body::Body::empty()),
        &guard
    ));
}

#[test]
fn schemas_and_annotations_match_the_seven_tool_contracts() {
    for tool in definitions() {
        let annotation = tool.annotations.unwrap();
        assert_eq!(annotation.open_world_hint, Some(false));
        assert_eq!(annotation.idempotent_hint, Some(true));
        assert_eq!(
            annotation.destructive_hint,
            Some(tool.name == "cancel_task")
        );
        assert_eq!(tool.input_schema["additionalProperties"], false);
    }
}
