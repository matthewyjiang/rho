use pretty_assertions::assert_eq;
use rho_protocol::McpServer;
use serde_json::{json, Value};

use super::host_mcp_config;
use crate::{
    app::session_runtime::session_config,
    config::Config,
    tools::mcp::config::{McpConfig, McpSamplingPolicy, McpTransport},
};

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
    McpServer::Stdio {
        name: name.into(),
        command: "/opt/t3/t3".into(),
        args: vec!["acp-mcp-bridge".into()],
        env: env
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
    }
}

fn http(name: &str, url: &str) -> McpServer {
    McpServer::Http {
        name: name.into(),
        url: url.into(),
        headers: [("Authorization".to_owned(), "Bearer token".to_owned())]
            .into_iter()
            .collect(),
    }
}

// Covers: client-provided MCP servers must become connectable session servers
// with their credentials intact, while unsafe or malformed entries are
// reported instead of connected.
// Owner: host MCP conversion
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
    ];
    for (label, servers, expected) in cases {
        assert_eq!(summary(&host_mcp_config(&servers)), expected, "{label}");
    }
}

// Covers: a host entry owns its name for the session. An invalid replacement
// must not fall back to the configured server's endpoint and credentials, and
// a repeated host name keeps only its last entry, valid or not.
// Owner: host MCP conversion, through the session runtime's overlay
#[test]
fn host_entries_own_their_name_over_config_servers() {
    // A process config whose own `bridge` server the host entries meet.
    let process = || Config {
        mcp: session_config(
            &Config::default(),
            &host_mcp_config(&[http("bridge", "https://config.example/mcp")]),
        )
        .mcp,
        ..Config::default()
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
        let config = session_config(&process(), &host_mcp_config(&servers));
        assert_eq!(summary(&config.mcp), expected, "{label}");
    }
}
