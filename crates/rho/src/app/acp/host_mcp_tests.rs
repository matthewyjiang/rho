use agent_client_protocol::schema::v1::{
    EnvVariable, HttpHeader, McpServer, McpServerHttp, McpServerSse, McpServerStdio,
};
use pretty_assertions::assert_eq;
use serde_json::{json, Value};

use super::{host_mcp_config, overlay_host_mcp};
use crate::tools::mcp::config::{McpConfig, McpSamplingPolicy, McpTransport};

/// Projects a converted config onto what a host controls: each accepted
/// server's transport, plus the identities rejected as invalid.
fn summary(config: &McpConfig) -> Value {
    let servers: serde_json::Map<String, Value> = config
        .servers
        .iter()
        .map(|(name, server)| {
            assert!(server.enabled, "{name} must start enabled");
            assert_eq!(server.sampling, McpSamplingPolicy::Deny, "{name}");
            let transport = match &server.transport {
                McpTransport::Stdio {
                    command, args, env, ..
                } => json!({ "stdio": { "command": command, "args": args, "env": env } }),
                McpTransport::StreamableHttp {
                    url,
                    headers,
                    allow_insecure_http,
                    ..
                } => json!({ "http": { "url": url, "headers": headers, "insecure": allow_insecure_http } }),
            };
            (name.clone(), transport)
        })
        .collect();
    let invalid: Vec<&str> = config
        .invalid_servers
        .iter()
        .map(|server| server.identity.as_str())
        .collect();
    json!({ "servers": servers, "invalid": invalid })
}

fn stdio(name: &str, env: &[(&str, &str)]) -> McpServer {
    McpServer::Stdio(
        McpServerStdio::new(name, "/opt/t3/t3")
            .args(vec!["acp-mcp-bridge".into()])
            .env(
                env.iter()
                    .map(|(name, value)| EnvVariable::new(*name, *value))
                    .collect(),
            ),
    )
}

fn http(name: &str, url: &str) -> McpServer {
    McpServer::Http(
        McpServerHttp::new(name, url)
            .headers(vec![HttpHeader::new("Authorization", "Bearer token")]),
    )
}

// Covers: host `mcpServers` must become connectable session servers with their
// credentials intact, while unsafe or malformed entries are reported instead of
// connected, and SSE (not spoken by Rho) is skipped.
// Owner: ACP host MCP conversion
#[test]
fn host_servers_convert_to_session_config() {
    let cases = [
        (
            "t3 stdio bridge keeps env credentials",
            vec![stdio(
                "t3-code",
                &[
                    ("T3_ACP_MCP_ENDPOINT", "http://127.0.0.1:3773/mcp"),
                    ("T3_ACP_MCP_AUTHORIZATION", "Bearer secret"),
                ],
            )],
            json!({
                "servers": { "t3-code": { "stdio": {
                    "command": "/opt/t3/t3",
                    "args": ["acp-mcp-bridge"],
                    "env": {
                        "T3_ACP_MCP_AUTHORIZATION": "Bearer secret",
                        "T3_ACP_MCP_ENDPOINT": "http://127.0.0.1:3773/mcp",
                    },
                } } },
                "invalid": [],
            }),
        ),
        (
            "loopback http keeps headers",
            vec![http("local", "http://127.0.0.1:3773/mcp")],
            json!({
                "servers": { "local": { "http": {
                    "url": "http://127.0.0.1:3773/mcp",
                    "headers": { "Authorization": "Bearer token" },
                    "insecure": false,
                } } },
                "invalid": [],
            }),
        ),
        (
            "cleartext http to a remote host is rejected",
            vec![http("remote", "http://100.75.19.168:3773/mcp")],
            json!({ "servers": {}, "invalid": ["remote"] }),
        ),
        (
            "names outside MCP identity rules are rejected",
            vec![stdio("t3 code", &[])],
            json!({ "servers": {}, "invalid": ["t3 code"] }),
        ),
        (
            "repeated env names are rejected",
            vec![stdio("dup", &[("A", "1"), ("A", "2")])],
            json!({ "servers": {}, "invalid": ["dup"] }),
        ),
        (
            "sse is skipped",
            vec![McpServer::Sse(McpServerSse::new(
                "sse",
                "https://example.com/sse",
            ))],
            json!({ "servers": {}, "invalid": [] }),
        ),
    ];
    for (label, servers, expected) in cases {
        assert_eq!(summary(&host_mcp_config(&servers)), expected, "{label}");
    }
}

// Covers: a host entry owns its name for the session. An invalid replacement
// must not fall back to the configured server's endpoint and credentials, and
// a repeated host name keeps only its last entry, valid or not.
// Owner: ACP host MCP overlay
#[test]
fn host_entries_own_their_name_over_config_servers() {
    let configured = || {
        let mut config = McpConfig::default();
        overlay_host_mcp(&mut config, &[http("bridge", "https://config.example/mcp")]);
        config
    };
    let cases = [
        (
            "valid host entry replaces the config server",
            vec![http("bridge", "https://host.example/mcp")],
            json!({
                "servers": { "bridge": { "http": {
                    "url": "https://host.example/mcp",
                    "headers": { "Authorization": "Bearer token" },
                    "insecure": false,
                } } },
                "invalid": [],
            }),
        ),
        (
            "invalid host entry suppresses the config server",
            vec![http("bridge", "http://100.75.19.168:3773/mcp")],
            json!({ "servers": {}, "invalid": ["bridge"] }),
        ),
        (
            "invalid repeat overrides an earlier valid host entry",
            vec![
                http("bridge", "https://host.example/mcp"),
                http("bridge", "http://100.75.19.168:3773/mcp"),
            ],
            json!({ "servers": {}, "invalid": ["bridge"] }),
        ),
        (
            "valid repeat clears an earlier invalid host entry",
            vec![
                http("bridge", "http://100.75.19.168:3773/mcp"),
                http("bridge", "https://host.example/mcp"),
            ],
            json!({
                "servers": { "bridge": { "http": {
                    "url": "https://host.example/mcp",
                    "headers": { "Authorization": "Bearer token" },
                    "insecure": false,
                } } },
                "invalid": [],
            }),
        ),
    ];
    for (label, servers, expected) in cases {
        let mut config = configured();
        overlay_host_mcp(&mut config, &servers);
        assert_eq!(summary(&config), expected, "{label}");
    }
}
