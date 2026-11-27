use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

fn server(status: SessionMcpStatus, error: Option<&str>) -> SessionMcpServer {
    SessionMcpServer {
        identity: "github".into(),
        enabled: true,
        status,
        tool_count: 0,
        detail: "streamable HTTP · https://example.com/mcp".into(),
        error: error.map(str::to_owned),
    }
}

// Covers: a failed server's `error` rides beside its `detail` and reads
// back as written, and a server without one, as every server from a daemon
// that predates the field, leaves it out and reads as having none.
// Owner: protocol wire contract.
#[test]
fn a_session_mcp_servers_error_is_optional_on_the_wire() {
    let cases = [
        (
            server(SessionMcpStatus::Failed, Some("connection refused")),
            json!({
                "identity": "github",
                "enabled": true,
                "status": "failed",
                "tool_count": 0,
                "detail": "streamable HTTP · https://example.com/mcp",
                "error": "connection refused",
            }),
        ),
        (
            server(SessionMcpStatus::Connected, None),
            json!({
                "identity": "github",
                "enabled": true,
                "status": "connected",
                "tool_count": 0,
                "detail": "streamable HTTP · https://example.com/mcp",
            }),
        ),
    ];
    for (server, wire) in cases {
        assert_eq!(serde_json::to_value(&server).unwrap(), wire);
        assert_eq!(
            serde_json::from_value::<SessionMcpServer>(wire).unwrap(),
            server
        );
    }
}
