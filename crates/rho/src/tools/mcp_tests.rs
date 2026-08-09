use std::{collections::BTreeMap, fs};

use pretty_assertions::assert_eq;

use super::{
    config::{McpConfig, McpFilesystemPolicy, McpServerConfig, McpToolFilter, McpTransport},
    parse_remote_url,
    progress::McpProgressRouter,
    session::prepare_server_filesystem,
    tool::{call_remote_tool, namespaced_tool_name, McpCall},
    McpBundle, McpRoots, McpServerStatus, McpSessionOptions, MCP_RUNTIME_CONSTRUCTIONS,
};
use crate::tools::sdk_registry::ToolBundle;
use rho_sdk::{
    tool::{ToolContext, ToolErrorKind},
    CancellationToken,
};

static MCP_CONNECT_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Connect options for tests: a generous output cap and no advertised roots, so
/// nothing depends on the machine's working directory.
fn test_options() -> McpSessionOptions {
    McpSessionOptions::new(12_000, McpRoots::default())
}

// Covers: an empty or fully disabled MCP config must not construct runtime work.
// Owner: MCP runtime initialization boundary.
#[tokio::test]
async fn zero_server_path_is_inert() {
    let _guard = MCP_CONNECT_TEST_LOCK.lock().await;
    let before = MCP_RUNTIME_CONSTRUCTIONS.load(std::sync::atomic::Ordering::Relaxed);

    let outcome = McpBundle::connect(&McpConfig::default(), test_options()).await;

    assert!(outcome.bundle.is_none());
    assert!(outcome.report.servers.is_empty());
    assert_eq!(
        MCP_RUNTIME_CONSTRUCTIONS.load(std::sync::atomic::Ordering::Relaxed),
        before
    );
}

// Covers: disabled servers appear in inventory without starting a runtime.
// Owner: MCP runtime initialization boundary.
#[tokio::test]
async fn disabled_servers_are_reported_without_runtime() {
    let _guard = MCP_CONNECT_TEST_LOCK.lock().await;
    let before = MCP_RUNTIME_CONSTRUCTIONS.load(std::sync::atomic::Ordering::Relaxed);
    let config = McpConfig {
        servers: BTreeMap::from([(
            "off".into(),
            McpServerConfig {
                enabled: false,
                tools: McpToolFilter::default(),
                log_level: None,
                transport: McpTransport::Stdio {
                    command: "false".into(),
                    args: Vec::new(),
                    cwd: None,
                    env: BTreeMap::new(),
                    env_from_env: BTreeMap::new(),
                },
                filesystem: None,
            },
        )]),
        invalid_servers: Vec::new(),
    };

    let outcome = McpBundle::connect(&config, test_options()).await;

    assert!(outcome.bundle.is_none());
    assert_eq!(outcome.report.servers.len(), 1);
    assert_eq!(
        outcome.report.servers[0].status(),
        McpServerStatus::Disabled
    );
    assert_eq!(
        MCP_RUNTIME_CONSTRUCTIONS.load(std::sync::atomic::Ordering::Relaxed),
        before
    );
}

// Covers: one malformed server must not prevent valid sibling config from loading.
// Owner: MCP configuration parser.
#[test]
fn malformed_servers_are_isolated() {
    let config: McpConfig = toml::from_str(
        r#"
        [servers.good]
        transport = "stdio"
        command = "server"

        [servers.bad]
        transport = "stdio"
        args = ["missing-command"]

        [servers.blank]
        transport = "stdio"
        command = " "

        [servers.cleartext]
        transport = "streamable_http"
        url = "http://example.com/mcp"

        [servers.duplicate-header]
        transport = "streamable_http"
        url = "https://example.com/mcp"
        headers_from_env = { Authorization = "TOKEN_A", authorization = "TOKEN_B" }

        [servers."bad id"]
        transport = "stdio"
        command = "server"
        "#,
    )
    .unwrap();

    assert_eq!(
        (
            config.servers.keys().cloned().collect::<Vec<_>>(),
            config
                .invalid_servers
                .iter()
                .map(|invalid| invalid.identity.clone())
                .collect::<Vec<_>>()
        ),
        (
            vec!["good".to_string()],
            vec![
                "bad".to_string(),
                "bad id".to_string(),
                "blank".to_string(),
                "cleartext".to_string(),
                "duplicate-header".to_string(),
            ],
        )
    );

    let persisted = toml::to_string(&config).unwrap();
    let reloaded: McpConfig = toml::from_str(&persisted).unwrap();
    assert_eq!(
        reloaded.servers.keys().cloned().collect::<Vec<_>>(),
        vec!["good"]
    );
    assert!(reloaded.invalid_servers.is_empty());
}

// Covers: package filesystem authority cannot leak into, or be forged through,
// the user-owned MCP configuration format.
// Owner: MCP configuration boundary.
#[test]
fn package_filesystem_policy_is_internal_only() {
    let server = McpServerConfig {
        enabled: true,
        tools: McpToolFilter::default(),
        log_level: None,
        transport: McpTransport::Stdio {
            command: "server".into(),
            args: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            env_from_env: BTreeMap::new(),
        },
        filesystem: Some(McpFilesystemPolicy {
            directory_root: "/private/plugins".into(),
            directory_relative_to_root: "data/demo".into(),
            allowed_roots: vec!["/private/plugins/demo".into()],
        }),
    };

    let serialized = toml::to_string(&server).unwrap();
    assert!(!serialized.contains("filesystem"));
    let reloaded: McpServerConfig = toml::from_str(&serialized).unwrap();
    assert!(reloaded.filesystem.is_none());

    let forged = format!("filesystem = 'package authority'\n{serialized}");
    assert!(toml::from_str::<McpServerConfig>(&forged).is_err());
}

// Covers: an escaping symlink already present at activation is rejected before
// the package data directory is created.
// Owner: MCP filesystem policy.
#[cfg(unix)]
#[test]
fn package_filesystem_rejects_observed_symlink_escape() {
    let directory = tempfile::tempdir().unwrap();
    let storage = directory.path().join("plugins");
    let plugin = storage.join("demo");
    std::fs::create_dir_all(&plugin).unwrap();
    let storage = std::fs::canonicalize(storage).unwrap();
    let data = storage.join("data/demo");
    let server = McpServerConfig {
        enabled: true,
        tools: McpToolFilter::default(),
        log_level: None,
        transport: McpTransport::Stdio {
            command: "server".into(),
            args: Vec::new(),
            cwd: Some(plugin.clone()),
            env: BTreeMap::new(),
            env_from_env: BTreeMap::new(),
        },
        filesystem: Some(McpFilesystemPolicy {
            directory_root: storage.clone(),
            directory_relative_to_root: "data/demo".into(),
            allowed_roots: vec![plugin, data.clone()],
        }),
    };

    let outside = directory.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, storage.join("data")).unwrap();
    let error = prepare_server_filesystem(&server).unwrap_err();
    assert!(error.to_string().contains("escapes its storage root"));
}

// Covers: remote credentials must not be sent over cleartext non-loopback URLs,
// and exported names and filters must remain deterministic.
// Owner: MCP pure security policy.
#[test]
fn remote_and_tool_policy_is_deterministic() {
    let url_cases = [
        ("https://example.com/mcp", true),
        ("http://localhost:3000/mcp", true),
        ("http://127.0.0.1:3000/mcp", true),
        ("http://[::1]:3000/mcp", true),
        ("http://example.com/mcp", false),
        ("file:///tmp/mcp", false),
    ];
    for (url, expected) in url_cases {
        assert_eq!(parse_remote_url(url).is_ok(), expected, "{url}");
    }

    assert_eq!(
        namespaced_tool_name("git-hub", "issues/list"),
        "mcp___rho_6769742d687562___rho_6973737565732f6c697374"
    );
    assert_ne!(
        namespaced_tool_name("devtools/validator", "lint"),
        namespaced_tool_name("devtools_validator", "lint")
    );
    let filter = McpToolFilter {
        allow: vec!["read".into(), "write".into()],
        deny: vec!["write".into()],
    };
    assert_eq!(
        ["read", "write", "other"].map(|name| filter.includes(name)),
        [true, false, false]
    );
}

// Covers: Streamable HTTP handshake and discovery must register remote tools.
// Owner: MCP Streamable HTTP transport boundary.
#[tokio::test]
async fn streamable_http_discovery() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let _guard = MCP_CONNECT_TEST_LOCK.lock().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let (headers_end, content_length) = loop {
                let mut chunk = [0; 4096];
                let read = stream.read(&mut chunk).await.unwrap();
                assert!(read > 0);
                request.extend_from_slice(&chunk[..read]);
                let Some(headers_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n")
                else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&request[..headers_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                break (headers_end, content_length);
            };
            let body_start = headers_end + 4;
            while request.len() < body_start + content_length {
                let mut chunk = [0; 4096];
                let read = stream.read(&mut chunk).await.unwrap();
                assert!(read > 0);
                request.extend_from_slice(&chunk[..read]);
            }
            let message: serde_json::Value =
                serde_json::from_slice(&request[body_start..body_start + content_length]).unwrap();
            let method = message["method"].as_str().unwrap();
            if method == "notifications/initialized" {
                stream
                    .write_all(
                        b"HTTP/1.1 202 Accepted\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                    )
                    .await
                    .unwrap();
                continue;
            }
            let (result, discovery_complete) = match method {
                "initialize" => (
                    serde_json::json!({
                        "protocolVersion": message["params"]["protocolVersion"],
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "rho-http-test", "version": "1"}
                    }),
                    false,
                ),
                "tools/list" => (
                    serde_json::json!({"tools": [{
                        "name": "remote_echo",
                        "description": "echo remotely",
                        "inputSchema": {"type": "object", "properties": {}}
                    }]}),
                    true,
                ),
                method => panic!("unexpected method {method}"),
            };
            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": message["id"],
                "result": result,
            })
            .to_string();
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response}",
                        response.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            if discovery_complete {
                break;
            }
        }
    });
    let config = McpConfig {
        servers: BTreeMap::from([(
            "remote".into(),
            McpServerConfig {
                enabled: true,
                tools: McpToolFilter::default(),
                log_level: None,
                transport: McpTransport::StreamableHttp {
                    url: format!("http://{address}/mcp"),
                    headers: BTreeMap::new(),
                    headers_from_env: BTreeMap::new(),
                },
                filesystem: None,
            },
        )]),
        invalid_servers: Vec::new(),
    };
    let outcome = McpBundle::connect(&config, test_options()).await;
    let bundle = outcome.bundle.unwrap();
    assert_eq!(bundle.tools()[0].spec().name, "mcp__remote__remote_echo");
    assert_eq!(
        outcome.report.servers[0].status(),
        McpServerStatus::Connected
    );
    bundle.shutdown().await;
    server.await.unwrap();
}

// Covers: stdio handshake, discovery, calls, cancellation, and shutdown must
// work end to end, and a failed sibling must not suppress a healthy server.
// Prerequisite: `python3` must be available on PATH (Unix-gated test).
// Owner: MCP stdio process lifecycle.
#[cfg(unix)]
#[tokio::test]
async fn stdio_lifecycle_and_failure_isolation() {
    let directory = tempfile::tempdir().unwrap();
    let _guard = MCP_CONNECT_TEST_LOCK.lock().await;
    let script = directory.path().join("server.py");
    let closed = directory.path().join("closed");
    let data = directory.path().join("data/demo");
    fs::write(
        &script,
        r#"import json, os, sys
assert os.environ["PLUGIN_DATA"] == sys.argv[2] and os.path.isdir(os.environ["PLUGIN_DATA"])
for line in sys.stdin:
    message = json.loads(line)
    if "id" not in message:
        continue
    method = message.get("method")
    if method == "initialize":
        result = {
            "protocolVersion": message["params"]["protocolVersion"],
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "rho-test", "version": "1"}
        }
    elif method == "tools/list":
        result = {"tools": [{
            "name": "echo/value",
            "description": "echo a value",
            "inputSchema": {"type": "object", "properties": {}}
        }]}
    elif method == "tools/call":
        result = {"content": [{"type": "text", "text": "ok"}], "isError": False}
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": result}), flush=True)
open(sys.argv[1], "w").close()
"#,
    )
    .unwrap();

    let healthy = McpServerConfig {
        enabled: true,
        tools: McpToolFilter::default(),
        log_level: None,
        transport: McpTransport::Stdio {
            command: "python3".into(),
            args: vec![
                script.display().to_string(),
                closed.display().to_string(),
                data.display().to_string(),
            ],
            cwd: None,
            env: BTreeMap::from([("PLUGIN_DATA".into(), data.display().to_string())]),
            env_from_env: BTreeMap::new(),
        },
        filesystem: Some(McpFilesystemPolicy {
            directory_root: directory.path().to_path_buf(),
            directory_relative_to_root: "data/demo".into(),
            allowed_roots: vec![directory.path().to_path_buf(), data.clone()],
        }),
    };
    let failed = McpServerConfig {
        enabled: true,
        tools: McpToolFilter::default(),
        log_level: None,
        transport: McpTransport::Stdio {
            command: "rho-mcp-command-that-does-not-exist".into(),
            args: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            env_from_env: BTreeMap::new(),
        },
        filesystem: None,
    };
    let config = McpConfig {
        servers: BTreeMap::from([
            ("devtools/validator".into(), healthy.clone()),
            ("devtools_validator".into(), healthy),
            ("failed".into(), failed),
        ]),
        invalid_servers: Vec::new(),
    };
    assert!(!data.exists());
    let outcome = McpBundle::connect(&config, test_options()).await;
    assert!(data.is_dir());
    let bundle = outcome.bundle.unwrap();
    assert_eq!(
        bundle
            .tools()
            .iter()
            .map(|tool| tool.spec().name)
            .collect::<Vec<_>>(),
        vec![
            "mcp___rho_646576746f6f6c732f76616c696461746f72___rho_6563686f2f76616c7565",
            "mcp__devtools_validator___rho_6563686f2f76616c7565",
        ]
    );
    let statuses = outcome
        .report
        .servers
        .iter()
        .map(|server| (server.identity.as_str(), server.status()))
        .collect::<Vec<_>>();
    assert_eq!(
        statuses,
        vec![
            ("devtools/validator", McpServerStatus::Connected),
            ("devtools_validator", McpServerStatus::Connected),
            ("failed", McpServerStatus::Failed),
        ]
    );
    let peer = bundle.sessions.lock().await[0].peer().clone();
    let progress = McpProgressRouter::new();
    let echo_call = || McpCall {
        peer: &peer,
        progress: &progress,
        remote_name: "echo/value".into(),
        arguments: serde_json::Map::new(),
    };
    let cancellation = CancellationToken::new();
    let content = call_remote_tool(echo_call(), &cancellation, None, 12_000)
        .await
        .unwrap();
    let content: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(content["content"][0]["text"], "ok");

    cancellation.cancel();
    let error = call_remote_tool(echo_call(), &cancellation, None, 12_000)
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ToolErrorKind::Cancelled);

    bundle.shutdown().await;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !closed.exists() {
        if std::time::Instant::now() >= deadline {
            panic!("MCP server did not create shutdown marker in time");
        }
        tokio::task::yield_now().await;
    }
    assert!(closed.exists());
}

// Covers: server-initiated protocol traffic must reach the session. Server
// instructions from `initialize` are captured, progress notifications reach the
// live tool-progress channel, `tools/list_changed` refreshes definitions and
// withdraws removed tools, added tools are reported rather than silently
// dropped, and a cancelled call tells the server to stop.
// Prerequisite: `python3` must be available on PATH (Unix-gated test).
// Owner: MCP server-to-client protocol boundary.
#[cfg(unix)]
#[tokio::test]
async fn server_initiated_protocol_traffic_is_handled() {
    use std::num::NonZeroUsize;

    let _guard = MCP_CONNECT_TEST_LOCK.lock().await;
    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("server.py");
    let cancelled_marker = directory.path().join("cancelled");
    fs::write(
        &script,
        r#"import json, sys

mutated = False

FIXED = [
    {"name": "mutate", "description": "revise the tool list", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "hang", "description": "never answers", "inputSchema": {"type": "object", "properties": {}}},
]

def send(message):
    print(json.dumps(message), flush=True)

def tools():
    if mutated:
        return [
            {"name": "echo", "description": "echo v2", "inputSchema": {"type": "object", "properties": {}}},
            {"name": "added", "description": "arrived late", "inputSchema": {"type": "object", "properties": {}}},
        ] + FIXED
    return [
        {"name": "echo", "description": "echo v1", "inputSchema": {"type": "object", "properties": {}}},
        {"name": "removed", "description": "goes away", "inputSchema": {"type": "object", "properties": {}}},
    ] + FIXED

for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    if "id" not in message:
        if method == "notifications/cancelled":
            open(sys.argv[1], "w").close()
        continue
    if method == "initialize":
        result = {
            "protocolVersion": message["params"]["protocolVersion"],
            "capabilities": {"tools": {"listChanged": True}, "logging": {}},
            "serverInfo": {"name": "rho-test", "version": "1"},
            "instructions": "Prefer echo over shout.",
        }
    elif method == "tools/list":
        result = {"tools": tools()}
    elif method == "tools/call":
        name = message["params"]["name"]
        token = message["params"].get("_meta", {}).get("progressToken")
        if token is not None:
            send({"jsonrpc": "2.0", "method": "notifications/progress", "params": {
                "progressToken": token, "progress": 2, "total": 4, "message": "halfway"}})
        if name == "hang":
            continue
        if name == "mutate":
            mutated = True
            send({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"})
        result = {"content": [{"type": "text", "text": "ok"}], "isError": False}
    else:
        result = {}
    send({"jsonrpc": "2.0", "id": message["id"], "result": result})
"#,
    )
    .unwrap();

    let config = McpConfig {
        servers: BTreeMap::from([(
            "live".into(),
            McpServerConfig {
                enabled: true,
                tools: McpToolFilter::default(),
                log_level: Some(crate::tools::mcp::config::McpLogLevel::Info),
                transport: McpTransport::Stdio {
                    command: "python3".into(),
                    args: vec![
                        script.display().to_string(),
                        cancelled_marker.display().to_string(),
                    ],
                    cwd: None,
                    env: BTreeMap::new(),
                    env_from_env: BTreeMap::new(),
                },
                filesystem: None,
            },
        )]),
        invalid_servers: Vec::new(),
    };
    let outcome = McpBundle::connect(&config, test_options()).await;
    let server = &outcome.report.servers[0];
    assert_eq!(server.instructions(), Some("Prefer echo over shout."));
    let bundle = outcome.bundle.unwrap();
    let tool_of = |name: &str| {
        bundle
            .tools()
            .iter()
            .find(|tool| tool.spec().name == name)
            .cloned()
            .unwrap_or_else(|| panic!("{name} was not exported"))
    };
    assert_eq!(
        tool_of("mcp__live__echo").spec().description,
        "MCP server `live`: echo v1"
    );

    // Progress raised against the call's token reaches the invocation's channel.
    let (sender, mut progress_events) =
        rho_sdk::tool::tool_progress_channel(NonZeroUsize::new(8).unwrap());
    let cancellation = CancellationToken::new();
    tool_of("mcp__live__echo")
        .call(
            invocation(),
            ToolContext::new(None, cancellation.clone(), sender),
        )
        .await
        .unwrap();
    let reported = progress_events.recv().await.unwrap();
    assert_eq!(
        (
            reported.text(),
            reported.completed_units(),
            reported.total_units()
        ),
        ("halfway", Some(2), Some(4))
    );

    // The server revises its tool list and announces the change.
    tool_of("mcp__live__mutate")
        .call(invocation(), discarding_context(cancellation.clone()))
        .await
        .unwrap();
    await_condition("tool list refresh", || {
        server.live().added_tools == vec!["added".to_string()]
    })
    .await;
    assert_eq!(server.live().removed_tools, vec!["removed".to_string()]);
    assert_eq!(
        tool_of("mcp__live__echo").spec().description,
        "MCP server `live`: echo v2"
    );

    // A withdrawn tool stays registered and fails with a reason, because the
    // registry is fixed for the session.
    let withdrawn = tool_of("mcp__live__removed")
        .call(invocation(), discarding_context(cancellation))
        .await
        .unwrap_err();
    assert_eq!(withdrawn.kind(), ToolErrorKind::Execution);
    assert!(withdrawn.message().contains("withdrew tool `removed`"));

    // Cancelling an in-flight call notifies the server instead of abandoning it.
    let cancel_token = CancellationToken::new();
    let hanging_tool = tool_of("mcp__live__hang");
    let hanging = hanging_tool.call(invocation(), discarding_context(cancel_token.clone()));
    let (cancelled, ()) = tokio::join!(hanging, async { cancel_token.cancel() });
    assert_eq!(cancelled.unwrap_err().kind(), ToolErrorKind::Cancelled);
    await_condition("cancellation notification", || cancelled_marker.exists()).await;

    bundle.shutdown().await;
}

/// One MCP invocation with no arguments.
#[cfg(unix)]
fn invocation() -> rho_sdk::tool::ToolInvocation {
    rho_sdk::tool::ToolInvocation::new(rho_sdk::ToolCallId::new(), serde_json::json!({}))
}

/// A context whose progress goes nowhere, for calls the test does not inspect.
#[cfg(unix)]
fn discarding_context(cancellation: CancellationToken) -> ToolContext {
    let (sender, _receiver) =
        rho_sdk::tool::tool_progress_channel(std::num::NonZeroUsize::new(1).unwrap());
    ToolContext::new(None, cancellation, sender)
}

/// Wait for an out-of-band effect to land, bounded so a real failure reports
/// what it was waiting for instead of hanging the suite.
#[cfg(unix)]
async fn await_condition(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !ready() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::task::yield_now().await;
    }
}
