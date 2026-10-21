use pretty_assertions::assert_eq;
use serde_json::{json, Value};

use super::{AcpServer, Workspace};

/// `(sessionUpdate, messageId)` of each message chunk, in order.
fn message_ids(updates: &[Value]) -> Vec<(String, String)> {
    updates
        .iter()
        .filter(|update| {
            update["sessionUpdate"]
                .as_str()
                .is_some_and(|kind| kind.ends_with("_chunk"))
        })
        .map(|update| {
            let id = update["messageId"]
                .as_str()
                .unwrap_or_else(|| panic!("chunk without messageId: {update}"));
            (
                update["sessionUpdate"].as_str().unwrap().to_owned(),
                id.to_owned(),
            )
        })
        .collect()
}

// Covers: hosts get a messageId on every message chunk: each live reply is
// its own message, and session/load replays user and agent messages with
// distinct IDs that are the same on every load, so a reloading host can
// match the messages it already has.
// Owner: ACP wire contract, through the real server and saved sessions
#[test]
fn message_chunks_carry_ids_live_and_across_loads() {
    let workspace = Workspace::new();
    let mut server = workspace.server();
    server
        .request("initialize", json!({"protocolVersion": 1}))
        .0
        .unwrap();
    let created = server
        .request(
            "session/new",
            json!({"cwd": workspace.project, "mcpServers": []}),
        )
        .0
        .unwrap();
    let session_id = created["sessionId"].as_str().unwrap().to_owned();
    let mut live = Vec::new();
    for _ in 0..2 {
        let (reply, updates) = server.request(
            "session/prompt",
            json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "read the shared notes"}]}),
        );
        reply.unwrap();
        let mut ids = message_ids(&updates);
        ids.dedup();
        assert_eq!(ids.len(), 1, "one reply message: {updates:#?}");
        live.extend(ids);
    }
    assert_ne!(live[0], live[1]);

    let mut loads = Vec::new();
    for _ in 0..2 {
        server
            .request("session/close", json!({"sessionId": session_id}))
            .0
            .unwrap();
        drop(server);
        server = workspace.server();
        server
            .request("initialize", json!({"protocolVersion": 1}))
            .0
            .unwrap();
        let (loaded, updates) = server.request(
            "session/load",
            json!({"sessionId": session_id, "cwd": workspace.project, "mcpServers": []}),
        );
        loaded.unwrap();
        loads.push(message_ids(&updates));
    }
    let kinds = loads[0]
        .iter()
        .map(|(kind, _)| kind.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        [
            "user_message_chunk",
            "agent_message_chunk",
            "user_message_chunk",
            "agent_message_chunk"
        ]
    );
    let distinct = loads[0]
        .iter()
        .map(|(_, id)| id)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(distinct.len(), 4, "{loads:#?}");
    assert_eq!(loads[0], loads[1]);
}

// Covers: tool calls point hosts at lines the real tools know, as the 0-based
// rows Zed reads: a read at its offset, a grep at each matched file's first
// hit, and an edit at its first changed line, all as absolute paths.
// Owner: ACP wire contract, through the real server and coding tools
#[test]
fn tool_call_locations_name_known_lines() {
    let workspace = Workspace::new();
    let config = workspace.home.path().join("config.toml");
    let settings = std::fs::read_to_string(&config).unwrap().replace(
        "permission_mode = \"plan\"",
        "permission_mode = \"bypass\"\nedit_tool = \"str_replace\"",
    );
    std::fs::write(config, settings).unwrap();
    std::fs::write(workspace.project.join("a.txt"), "alpha\nbeta\ngamma\n").unwrap();
    std::fs::write(workspace.project.join("b.txt"), "one\ntwo\nbeta\n").unwrap();
    let mut server = AcpServer::spawn(
        workspace.home.path(),
        &workspace.project,
        "tool-call",
        &workspace.project,
    );
    server
        .request("initialize", json!({"protocolVersion": 1}))
        .0
        .unwrap();
    let created = server
        .request(
            "session/new",
            json!({"cwd": workspace.project, "mcpServers": []}),
        )
        .0
        .unwrap();
    let a = workspace.project.join("a.txt");
    let b = workspace.project.join("b.txt");
    for (call, update, expected) in [
        (
            json!({"name": "read_file", "arguments": {"path": "a.txt", "offset": 2}}),
            "tool_call",
            json!([{"path": a, "line": 1}]),
        ),
        (
            json!({"name": "grep", "arguments": {"pattern": "beta"}}),
            "tool_call_update",
            json!([{"path": a, "line": 1}, {"path": b, "line": 2}]),
        ),
        (
            json!({"name": "str_replace", "arguments": {"path": "a.txt", "old_string": "gamma", "new_string": "delta"}}),
            "tool_call_update",
            json!([{"path": a, "line": 2}]),
        ),
    ] {
        let mut call = call;
        call["id"] = json!("located");
        let (reply, updates) = server.request(
            "session/prompt",
            json!({
                "sessionId": created["sessionId"],
                "prompt": [{"type": "text", "text": call.to_string()}],
            }),
        );
        reply.unwrap();
        let located = updates
            .iter()
            .find(|candidate| {
                candidate["sessionUpdate"] == update && candidate.get("locations").is_some()
            })
            .unwrap_or_else(|| panic!("no {update} with locations: {updates:#?}"));
        assert_eq!(located["locations"], expected, "{}", call["name"]);
    }
}
