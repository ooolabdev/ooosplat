use super::*;
use rmcp::{
    transport::{
        streamable_http_client::StreamableHttpClientTransportConfig, StreamableHttpClientTransport,
    },
    ServiceExt,
};

#[tokio::test]
async fn official_client_streamable_http_smoke_and_auth_rejections() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("clip.mp4");
    std::fs::write(&input, b"fixture, not a real video").unwrap();
    let tasks = TaskService::at(temp.path().join("registry"));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}/mcp");
    let token = "test-only-token";
    let stop = CancellationToken::new();
    let tools = McpTools {
        tasks: tasks.clone(),
        engines: crate::pipeline::runner::default_engine_paths(Some(
            temp.path().join("missing-engines"),
        )),
        settings: McpSettings {
            enabled: true,
            port,
            input_roots: vec![temp.path().into()],
        },
        telemetry: None,
    };
    let app = router(
        tools,
        Auth {
            token: token.into(),
            port,
        },
        stop.clone(),
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let http = reqwest::Client::new();
    assert_eq!(
        http.post(&url)
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        http.post(&url)
            .bearer_auth(token)
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
            .bearer_auth(token)
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
        StreamableHttpClientTransportConfig::with_uri(url.clone()).auth_header(token),
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
    let arguments =
        json!({"input_path":input,"quality":"fast","client_request_id":"protocol-request-1"});
    let created = client
        .call_tool(
            CallToolRequestParams::new("create_generation_task")
                .with_arguments(arguments.as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_eq!(created.is_error, Some(false));
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
    client.cancel().await.unwrap(); // Explicitly disconnect the actual MCP client.
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
        StreamableHttpClientTransportConfig::with_uri(url).auth_header(token),
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
    client.cancel().await.unwrap();
    stop.cancel();
    server.abort();
    let _ = server.await;
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
