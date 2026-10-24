use pretty_assertions::assert_eq;
use serde_json::{json, Value};

use super::{AcpServer, Workspace};

/// The `initialize` T3 Code sends: v2 fields, plus the v1 ones so a v1-only
/// agent can still answer.
fn negotiating_initialize() -> Value {
    json!({
        "protocolVersion": 2,
        "info": {"name": "test-host", "version": "1.0.0"},
        "capabilities": {},
        "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}},
        "clientInfo": {"name": "test-host", "version": "1.0.0"},
    })
}

impl AcpServer {
    /// Sends a v2 `session/prompt` and collects every update until the
    /// session reports `idle`. Returns the reply and those updates; the reply
    /// may arrive before or after them.
    fn prompt_v2(&mut self, session_id: &str, text: &str) -> (Result<Value, Value>, Vec<Value>) {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({
            "jsonrpc": "2.0", "id": id, "method": "session/prompt",
            "params": {"sessionId": session_id, "prompt": [{"type": "text", "text": text}]},
        }));
        let mut reply = None;
        let mut updates = Vec::new();
        loop {
            let message = self.recv("v2 prompt traffic");
            if message["id"] == id && message.get("method").is_none() {
                let answer = match message.get("error") {
                    Some(error) => Err(error.clone()),
                    None => Ok(message["result"].clone()),
                };
                // A refused prompt is never inserted, so no `idle` follows.
                if answer.is_err() {
                    return (answer, updates);
                }
                reply = Some(answer);
            } else {
                assert_eq!(message["method"], "session/update", "{message}");
                updates.push(message["params"]["update"].clone());
            }
            let idle = updates.last().is_some_and(|update| {
                update["sessionUpdate"] == "state_update" && update["state"] == "idle"
            });
            if let (Some(reply), true) = (&reply, idle) {
                return (reply.clone(), updates);
            }
        }
    }
}

fn kinds(updates: &[Value]) -> Vec<&str> {
    updates
        .iter()
        .filter_map(|update| update["sessionUpdate"].as_str())
        .collect()
}

// Covers: a v2 host (T3 Code) must get v2 back from `initialize`, a
// `session/prompt` answer that only acknowledges the inserted message, the
// turn's progress and stop reason as `state_update`s, tool calls created by
// their first `tool_call_update`, and file changes as structured diffs with
// a Git patch.
// Owner: ACP v2 boundary, over the real stdio server
#[test]
fn v2_prompt_reports_its_lifecycle_through_state_updates() {
    let workspace = Workspace::new();
    let config = workspace.home.path().join("config.toml");
    let settings = std::fs::read_to_string(&config)
        .unwrap()
        .replace("permission_mode = \"plan\"", "permission_mode = \"bypass\"");
    std::fs::write(config, settings).unwrap();
    let mut server = AcpServer::spawn(
        workspace.home.path(),
        &workspace.project,
        "tool-call",
        &workspace.project,
    );
    let initialized = server
        .request("initialize", negotiating_initialize())
        .0
        .unwrap();
    assert_eq!(
        (
            &initialized["protocolVersion"],
            &initialized["info"]["name"],
            &initialized["capabilities"]["session"]["prompt"],
            initialized.get("agentCapabilities"),
        ),
        (
            &json!(2),
            &json!("rho"),
            &json!({"image": {}, "embeddedContext": {}}),
            None
        )
    );

    // v2 makes `mcpServers` optional.
    let created = server
        .request("session/new", json!({"cwd": workspace.project}))
        .0
        .unwrap();
    let session_id = created["sessionId"].as_str().unwrap().to_owned();
    assert!(
        created["configOptions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|option| option["configId"].is_string() && option.get("id").is_none()),
        "{created:#}"
    );
    assert!(
        created["availableCommands"]
            .as_array()
            .is_some_and(|commands| !commands.is_empty()),
        "setup carries the commands: {created:#}"
    );
    assert_eq!(created.get("modes"), None);

    let call = json!({
        "id": "file-change", "name": "write",
        "arguments": {"path": "new.txt", "content": "created\n"},
    });
    let (reply, updates) = server.prompt_v2(&session_id, &call.to_string());
    let reply = reply.unwrap();
    let message_id = reply["messageId"]
        .as_str()
        .expect("prompt acknowledges a message id");

    assert_eq!(
        &updates[..2],
        &[
            json!({
                "sessionUpdate": "user_message",
                "messageId": message_id,
                "content": [{"type": "text", "text": call.to_string()}],
            }),
            json!({"sessionUpdate": "state_update", "state": "running"}),
        ]
    );
    assert_eq!(
        updates.last().unwrap(),
        &json!({"sessionUpdate": "state_update", "state": "idle", "stopReason": "end_turn"})
    );
    let kinds = kinds(&updates);
    for v1_only in ["tool_call", "plan", "current_mode_update"] {
        assert!(!kinds.contains(&v1_only), "{v1_only} in {kinds:?}");
    }
    let tool_updates: Vec<_> = updates
        .iter()
        .filter(|update| update["sessionUpdate"] == "tool_call_update")
        .collect();
    let created_call = tool_updates.first().expect("tool call");
    assert!(created_call["title"].is_string(), "{created_call:#}");
    let finished = tool_updates
        .iter()
        .find(|update| update["status"] == "completed")
        .unwrap_or_else(|| panic!("tool did not complete: {updates:#?}"));
    let path = workspace.project.join("new.txt");
    let path = path.to_str().unwrap();
    let diff = finished["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|content| content["type"] == "diff")
        .unwrap_or_else(|| panic!("no diff: {finished:#}"));
    assert_eq!(
        diff,
        &json!({
            "type": "diff",
            "changes": [{"operation": "add", "path": path, "fileType": "text"}],
            "patch": {
                "format": "git_patch",
                "text": format!(
                    "diff --git {path} {path}\nnew file mode 100644\n--- /dev/null\n+++ {path}\n@@ -0,0 +1 @@\n+created\n"
                ),
            },
        })
    );
}

// Covers: v2 reserves an error answer to `session/prompt` for prompts that
// were never inserted, so a refused prompt gets no updates; `session/resume`
// with `replayFrom: start` replays the conversation, keeping each user
// message's ID, and then reports the current state before answering; an
// unknown replay cursor is invalid params.
// Owner: ACP v2 boundary, over the real stdio server
#[test]
fn v2_resume_replays_and_refused_prompts_send_nothing() {
    let workspace = Workspace::new();
    let mut server = workspace.server();
    server
        .request("initialize", negotiating_initialize())
        .0
        .unwrap();

    let (refused, updates) = server.prompt_v2("missing-session", "hello");
    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(updates, Vec::<Value>::new());

    let created = server
        .request("session/new", json!({"cwd": workspace.project}))
        .0
        .unwrap();
    let session_id = created["sessionId"].as_str().unwrap().to_owned();
    let (reply, _) = server.prompt_v2(&session_id, "read the shared notes");
    let message_id = reply.unwrap()["messageId"].clone();
    server
        .request("session/close", json!({"sessionId": session_id}))
        .0
        .unwrap();

    let (unknown, _) = server.request(
        "session/resume",
        json!({"sessionId": session_id, "cwd": workspace.project, "replayFrom": {"type": "_later"}}),
    );
    assert_eq!(unknown.unwrap_err()["code"], -32602);

    let (resumed, replay) = server.request(
        "session/resume",
        json!({"sessionId": session_id, "cwd": workspace.project, "replayFrom": {"type": "start"}}),
    );
    resumed.unwrap();
    let kinds = kinds(&replay);
    assert_eq!(kinds.first(), Some(&"user_message_chunk"), "{replay:#?}");
    // A replayed user message keeps the ID its prompt was answered with.
    assert_eq!(replay[0]["messageId"], message_id);
    assert!(kinds.contains(&"tool_call_update"), "{kinds:?}");
    assert_eq!(
        replay.last().unwrap(),
        &json!({"sessionUpdate": "state_update", "state": "idle"})
    );
}
