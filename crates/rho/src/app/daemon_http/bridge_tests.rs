use std::collections::BTreeMap;

use pretty_assertions::assert_eq;
use rho_protocol::MAX_LINE_BYTES;
use serde_json::{json, Value};

use super::prepare_daemon_line;

fn sample_host_env() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("PATH".to_owned(), "/usr/local/bin:/usr/bin".to_owned()),
        ("HOME".to_owned(), "/home/host".to_owned()),
    ])
}

// Covers: incoming WebSocket text frames are compacted to one NDJSON line,
// environment-bearing methods inherit host_env with client overrides, malformed
// non-object environment values pass through for daemon validation, and raw
// newlines in invalid JSON cannot smuggle a second JSON-RPC line.
// Owner: daemon_http::bridge.
#[test]
fn prepare_daemon_line_compacts_overlays_env_and_prevents_smuggling() {
    let host_env = sample_host_env();

    // 1. Omitted environment on rho/session/new gets host_env.
    let line = prepare_daemon_line(
        r#"{"jsonrpc":"2.0","id":1,"method":"rho/session/new","params":{"cwd":"/tmp"}}"#,
        &host_env,
    )
    .unwrap();
    assert!(!line.contains(&b'\n'));
    let parsed: Value = serde_json::from_slice(&line).unwrap();
    assert_eq!(
        parsed,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "rho/session/new",
            "params": {
                "cwd": "/tmp",
                "environment": {
                    "HOME": "/home/host",
                    "PATH": "/usr/local/bin:/usr/bin"
                }
            }
        })
    );

    // 2. Client environment overlays host_env (client wins on collision).
    let line = prepare_daemon_line(
        r#"{"jsonrpc":"2.0","id":2,"method":"rho/session/prompt","params":{"session":"s1","content":[],"environment":{"PATH":"/custom/bin","EXTRA":"1"}}}"#,
        &host_env,
    )
    .unwrap();
    let parsed: Value = serde_json::from_slice(&line).unwrap();
    assert_eq!(
        parsed["params"]["environment"],
        json!({
            "HOME": "/home/host",
            "PATH": "/custom/bin",
            "EXTRA": "1"
        })
    );

    // 3. Non-object environment stays untouched so the daemon returns InvalidParams.
    let line = prepare_daemon_line(
        r#"{"jsonrpc":"2.0","id":3,"method":"rho/auth/list","params":{"environment":"bad"}}"#,
        &host_env,
    )
    .unwrap();
    let parsed: Value = serde_json::from_slice(&line).unwrap();
    assert_eq!(parsed["params"]["environment"], json!("bad"));

    // 4. Pretty-printed non-environment method compacts to one line without
    // injecting an environment field.
    let pretty = "{\n  \"jsonrpc\": \"2.0\",\n  \"id\": 4,\n  \"method\": \"rho/daemon/status\",\n  \"params\": {}\n}";
    let line = prepare_daemon_line(pretty, &host_env).unwrap();
    assert!(!line.contains(&b'\n'));
    let parsed: Value = serde_json::from_slice(&line).unwrap();
    assert_eq!(parsed["params"], json!({}));

    // 5. Invalid JSON with embedded newline stays one single line (no smuggling).
    let smuggled = "not json\n{\"jsonrpc\":\"2.0\",\"id\":5,\"method\":\"rho/daemon/stop\"}";
    let line = prepare_daemon_line(smuggled, &host_env).unwrap();
    assert!(!line.contains(&b'\n'));
    assert!(!line.contains(&b'\r'));

    // 6. Oversized frame is rejected before reaching the daemon.
    let huge = "x".repeat(MAX_LINE_BYTES + 1);
    assert!(prepare_daemon_line(&huge, &host_env).is_err());
}
