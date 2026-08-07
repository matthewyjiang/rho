use std::{collections::BTreeMap, fs};

use pretty_assertions::assert_eq;

use super::{
    call_remote_tool,
    config::{McpConfig, McpServerConfig, McpToolFilter, McpTransport},
    namespaced_tool_name, validate_remote_url, McpBundle, McpServerStatus,
    MCP_RUNTIME_CONSTRUCTIONS,
};
use crate::tools::sdk_registry::ToolBundle;
use rho_sdk::{tool::ToolErrorKind, CancellationToken};

static MCP_CONNECT_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// Covers: an empty or fully disabled MCP config must not construct runtime work.
// Owner: MCP runtime initialization boundary.
#[tokio::test]
async fn zero_server_path_is_inert() {
    let _guard = MCP_CONNECT_TEST_LOCK.lock().await;
    let before = MCP_RUNTIME_CONSTRUCTIONS.load(std::sync::atomic::Ordering::Relaxed);

    let outcome = McpBundle::connect(&McpConfig::default()).await;

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
                transport: McpTransport::Stdio {
                    command: "false".into(),
                    args: Vec::new(),
                    cwd: None,
                    env: BTreeMap::new(),
                    env_from_env: BTreeMap::new(),
                },
            },
        )]),
        invalid_servers: Vec::new(),
    };

    let outcome = McpBundle::connect(&config).await;

    assert!(outcome.bundle.is_none());
    assert_eq!(outcome.report.servers.len(), 1);
    assert_eq!(outcome.report.servers[0].status, McpServerStatus::Disabled);
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
        assert_eq!(validate_remote_url(url).is_ok(), expected, "{url}");
    }

    assert_eq!(
        namespaced_tool_name("git-hub", "issues/list"),
        "mcp__git_hub__issues_list"
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
                transport: McpTransport::StreamableHttp {
                    url: format!("http://{address}/mcp"),
                    headers_from_env: BTreeMap::new(),
                },
            },
        )]),
        invalid_servers: Vec::new(),
    };
    let outcome = McpBundle::connect(&config).await;
    let bundle = outcome.bundle.unwrap();
    assert_eq!(bundle.tools()[0].spec().name, "mcp__remote__remote_echo");
    assert_eq!(outcome.report.servers[0].status, McpServerStatus::Connected);
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
    fs::write(
        &script,
        r#"import json, sys
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
        transport: McpTransport::Stdio {
            command: "python3".into(),
            args: vec![script.display().to_string(), closed.display().to_string()],
            cwd: None,
            env: BTreeMap::new(),
            env_from_env: BTreeMap::new(),
        },
    };
    let failed = McpServerConfig {
        enabled: true,
        tools: McpToolFilter::default(),
        transport: McpTransport::Stdio {
            command: "rho-mcp-command-that-does-not-exist".into(),
            args: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            env_from_env: BTreeMap::new(),
        },
    };
    let config = McpConfig {
        servers: BTreeMap::from([("failed".into(), failed), ("test".into(), healthy)]),
        invalid_servers: Vec::new(),
    };
    let outcome = McpBundle::connect(&config).await;
    let bundle = outcome.bundle.unwrap();
    assert_eq!(
        bundle
            .tools()
            .iter()
            .map(|tool| tool.spec().name)
            .collect::<Vec<_>>(),
        vec!["mcp__test__echo_value"]
    );
    let statuses = outcome
        .report
        .servers
        .iter()
        .map(|server| (server.identity.as_str(), server.status))
        .collect::<Vec<_>>();
    assert_eq!(
        statuses,
        vec![
            ("failed", McpServerStatus::Failed),
            ("test", McpServerStatus::Connected),
        ]
    );

    let peer = bundle.sessions.lock().await[0].peer().clone();
    let cancellation = CancellationToken::new();
    let content = call_remote_tool(
        &peer,
        "echo/value".into(),
        serde_json::Map::new(),
        &cancellation,
    )
    .await
    .unwrap();
    let content: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(content["content"][0]["text"], "ok");

    cancellation.cancel();
    let error = call_remote_tool(
        &peer,
        "echo/value".into(),
        serde_json::Map::new(),
        &cancellation,
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind(), ToolErrorKind::Cancelled);

    bundle.shutdown().await;
    assert!(closed.exists());
}
