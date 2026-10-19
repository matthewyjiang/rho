use std::sync::{Arc, Mutex};

use pretty_assertions::assert_eq;
use rho_sdk::{tool::ToolErrorKind, ToolHost, ToolHostCall};
use rmcp::{model::Tool as RemoteTool, service::RunningService, RoleClient, ServiceExt};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::{
    list_tool_definitions, refresh_tools, McpServerOffers, McpToolSlot, SessionMaintenance,
};
use crate::tools::mcp::{
    catalog::McpCatalog, config::McpToolFilter, inflight::McpInFlightCalls,
    progress::McpProgressRouter, report::McpLiveServerState, tool::McpTool, McpImageDelivery,
};

fn remote(schema: Value) -> RemoteTool {
    let mut tool = RemoteTool::new("count", "count things", Arc::new(Default::default()));
    tool.output_schema = Some(Arc::new(schema.as_object().unwrap().clone()));
    tool
}

// Covers: a schema batch must yield between tools so its enclosing discovery
// deadline can cancel it without publishing a partially compiled catalog.
// Owner: MCP discovery compilation and deadline boundary.
#[tokio::test(start_paused = true)]
async fn schema_compilation_yields_to_discovery_deadline() {
    let filter = McpToolFilter::default();
    let tools = vec![
        remote(json!({"type": "object"})),
        remote(json!({"type": "object"})),
    ];
    let deadline = tokio::time::Instant::now() + super::MCP_MAINTENANCE_REQUEST_BUDGET;
    let compilation = super::compile_tool_definitions("fixture", tools, &filter, deadline);
    tokio::pin!(compilation);
    assert!(futures_util::poll!(compilation.as_mut()).is_pending());
    assert!(futures_util::poll!(compilation.as_mut()).is_pending());
    tokio::time::advance(super::MCP_MAINTENANCE_REQUEST_BUDGET).await;
    assert!(matches!(
        compilation.await,
        Err(super::ToolDiscoveryError::TimedOut)
    ));
    // Even a batch with no iterations must not publish after its deadline.
    assert!(matches!(
        super::compile_tool_definitions("fixture", Vec::new(), &filter, deadline).await,
        Err(super::ToolDiscoveryError::TimedOut)
    ));
}

/// Real client discovery and calls over a local byte stream, without a child
/// process or server feature. The one-byte pipe deliberately exercises partial IO.
async fn listing_session(
    tools: Arc<Mutex<Vec<RemoteTool>>>,
) -> (RunningService<RoleClient, ()>, tokio::task::JoinHandle<()>) {
    let (server, client) = tokio::io::duplex(1);
    let task = tokio::spawn(async move {
        let (reader, mut writer) = tokio::io::split(server);
        let mut lines = BufReader::new(reader).lines();
        while let Some(line) = lines.next_line().await.unwrap() {
            let request: Value = serde_json::from_str(&line).unwrap();
            let result = match request["method"].as_str().unwrap() {
                "initialize" => json!({
                    "protocolVersion": request["params"]["protocolVersion"],
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "schema-fixture", "version": "1"},
                }),
                "tools/list" => json!({"tools": *tools.lock().unwrap()}),
                "tools/call" => json!({
                    "content": [],
                    "structuredContent": request["params"]["arguments"],
                }),
                "notifications/initialized" | "notifications/cancelled" => continue,
                method => panic!("unexpected MCP method: {method}"),
            };
            let response = json!({"jsonrpc": "2.0", "id": request["id"], "result": result});
            writer
                .write_all(response.to_string().as_bytes())
                .await
                .unwrap();
            writer.write_all(b"\n").await.unwrap();
        }
    });
    (().serve(client).await.unwrap(), task)
}

// Covers: malformed, externally resolving, and over-budget declarations must
// fail tools/list only when selected, before a tool can be exported or called.
// Excluded declarations must not block valid selected tools or lose their count.
// Owner: MCP discovery and output-schema security boundary.
#[tokio::test]
async fn tools_list_validates_only_selected_output_schemas() {
    // Exceeds the schema node budget without reaching its byte budget.
    let properties = (0..1_100)
        .map(|index| (format!("f{index}"), json!({"type": "string"})))
        .collect::<serde_json::Map<_, _>>();
    let tools = Arc::new(Mutex::new(Vec::new()));
    let (mut client, server) = listing_session(Arc::clone(&tools)).await;
    for schema in [
        json!({"type": "not-a-json-schema-type"}),
        json!({"$ref": "https://example.invalid/schema.json"}),
        json!({"$ref": "file:///untrusted-schema.json"}),
        json!({"description": "x".repeat(64 * 1024)}),
        json!({"type": "object", "properties": properties}),
    ] {
        let mut bad = remote(schema);
        bad.name = "broken".into();
        *tools.lock().unwrap() = vec![remote(json!({"type": "object"})), bad];
        let error = list_tool_definitions(
            "fixture",
            client.peer(),
            &McpToolFilter::default(),
            tokio::time::Instant::now() + super::MCP_SERVER_STARTUP_BUDGET,
        )
        .await
        .unwrap_err();
        let super::ToolDiscoveryError::Failed(error) = error else {
            panic!("invalid schema unexpectedly timed out");
        };
        assert_eq!(
            error
                .downcast_ref::<rho_sdk::tool::ToolError>()
                .unwrap()
                .kind(),
            ToolErrorKind::Execution
        );
        for filter in [
            McpToolFilter {
                allow: vec!["count".into()],
                deny: vec![],
            },
            McpToolFilter {
                allow: vec!["count".into(), "broken".into()],
                deny: vec!["broken".into()],
            },
        ] {
            let discovered = list_tool_definitions(
                "fixture",
                client.peer(),
                &filter,
                tokio::time::Instant::now() + super::MCP_SERVER_STARTUP_BUDGET,
            )
            .await
            .unwrap();
            assert_eq!(
                (
                    discovered
                        .definitions
                        .into_iter()
                        .map(|(name, _)| name)
                        .collect::<Vec<_>>(),
                    discovered.filtered_out_count,
                ),
                (vec!["count".to_owned()], 1),
            );
        }
    }
    client.close().await.unwrap();
    server.await.unwrap();
}

// Covers: refreshed calls must use the new compiled validator; a rejected
// refresh must leave the last complete selected set live, excluded invalid
// declarations must not block refresh, and rediscovering an equal declaration
// must not report a change just because it was recompiled.
// Owner: MCP live tool-definition reconciliation.
#[tokio::test]
async fn refreshed_output_schema_replaces_validator_atomically() {
    let integer = json!({
        "type": "object", "required": ["count"],
        "properties": {"count": {"type": "integer"}},
    });
    let string = json!({
        "type": "object", "required": ["count"],
        "properties": {"count": {"type": "string"}},
    });
    let mut excluded = remote(json!({"type": "invalid"}));
    excluded.name = "excluded".into();
    let tools = Arc::new(Mutex::new(vec![remote(integer.clone()), excluded.clone()]));
    let (mut client, server_task) = listing_session(Arc::clone(&tools)).await;
    let server: super::McpServerConfig = serde_json::from_value(json!({
        "transport": "stdio", "command": "unused",
        "tools": {"deny": ["excluded"]},
    }))
    .unwrap();
    let (_, definition) = list_tool_definitions(
        "fixture",
        client.peer(),
        &server.tools,
        tokio::time::Instant::now() + super::MCP_SERVER_STARTUP_BUDGET,
    )
    .await
    .unwrap()
    .definitions
    .pop()
    .unwrap();
    let name = definition.spec.name.clone();
    let slot = Arc::new(McpToolSlot::new(definition));
    let transport = server.transport.clone();
    let catalog = McpCatalog::default();
    let (_events, receiver) = tokio::sync::mpsc::unbounded_channel();
    let maintenance = SessionMaintenance {
        identity: "fixture".into(),
        peer: client.peer().clone(),
        server,
        slots: [("count".into(), Arc::clone(&slot))].into(),
        live: McpLiveServerState::default(),
        events: receiver,
        catalog: catalog.register(
            "fixture".into(),
            client.peer().clone(),
            McpServerOffers::default(),
        ),
        offers: McpServerOffers::default(),
    };
    let tool = McpTool {
        slot: Arc::clone(&slot),
        identity: "fixture".into(),
        remote_name: "count".into(),
        peer: client.peer().clone(),
        progress: McpProgressRouter::new(),
        calls: McpInFlightCalls::new(),
        transport,
        max_output_bytes: rho_tools::DEFAULT_MAX_OUTPUT_BYTES,
        image_delivery: McpImageDelivery::PresentationOnly,
    };
    let host = ToolHost::builder()
        .tool_shared(Arc::new(tool))
        .build()
        .unwrap();
    assert!(host
        .invoke(ToolHostCall::new(&name, json!({"count": 7})))
        .await
        .is_ok());

    *tools.lock().unwrap() = vec![remote(string), excluded];
    refresh_tools(&maintenance).await;
    assert!(host
        .invoke(ToolHostCall::new(&name, json!({"count": 7})))
        .await
        .is_err());
    assert!(host
        .invoke(ToolHostCall::new(&name, json!({"count": "seven"})))
        .await
        .is_ok());

    let (_, equal) = list_tool_definitions(
        "fixture",
        client.peer(),
        &maintenance.server.tools,
        tokio::time::Instant::now() + super::MCP_SERVER_STARTUP_BUDGET,
    )
    .await
    .unwrap()
    .definitions
    .pop()
    .unwrap();
    assert!(!slot.refresh(equal));

    // The first declaration is valid but changed; the later bad one must keep
    // the entire refresh from partially replacing an already-live validator.
    let mut bad = remote(json!({"type": "invalid"}));
    bad.name = "broken".into();
    *tools.lock().unwrap() = vec![remote(integer), bad];
    refresh_tools(&maintenance).await;
    assert!(host
        .invoke(ToolHostCall::new(&name, json!({"count": "seven"})))
        .await
        .is_ok());
    assert!(host
        .invoke(ToolHostCall::new(&name, json!({"count": 7})))
        .await
        .is_err());
    assert_eq!(maintenance.live.snapshot(), Default::default());

    client.close().await.unwrap();
    server_task.await.unwrap();
}
