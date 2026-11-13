use std::collections::BTreeMap;

use pretty_assertions::assert_eq;
use serde_json::{json, Value};

use super::{AcpServer, Workspace};

/// The `initialize` T3 Code sends: v2 fields, plus the v1 ones so a v1-only
/// agent can still answer.
fn negotiating_initialize() -> Value {
    json!({
        "protocolVersion": 2,
        "info": {"name": "t3-code", "version": "1.0.0"},
        "capabilities": {},
        "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}},
        "clientInfo": {"name": "t3-code", "version": "1.0.0"},
    })
}

impl AcpServer {
    /// Sends a v2 `session/prompt` and collects every update until the
    /// session reports `idle`. Returns the reply and those updates; the reply
    /// may arrive before or after them.
    pub(super) fn prompt_v2(
        &mut self,
        session_id: &str,
        text: &str,
    ) -> (Result<Value, Value>, Vec<Value>) {
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

/// Applies `updates` to message texts as a v2 host holds them by ID: a whole
/// message replaces its content, and a chunk appends to it.
fn hold_messages(held: &mut BTreeMap<String, String>, updates: &[Value]) {
    let text = |block: &Value| block["text"].as_str().unwrap_or_default().to_owned();
    for update in updates {
        let Some(id) = update["messageId"].as_str() else {
            continue;
        };
        match update["sessionUpdate"].as_str() {
            Some("user_message" | "agent_message" | "agent_thought") => {
                if let Some(content) = update["content"].as_array() {
                    held.insert(id.to_owned(), content.iter().map(text).collect());
                }
            }
            Some("user_message_chunk" | "agent_message_chunk" | "agent_thought_chunk") => {
                held.entry(id.to_owned())
                    .or_default()
                    .push_str(&text(&update["content"]));
            }
            _ => {}
        }
    }
}

/// The terminal and state updates among `updates`: the session's title,
/// generated after its first turn, may land among them.
fn background_traffic(updates: &[Value]) -> Vec<Value> {
    updates
        .iter()
        .filter(|update| {
            matches!(
                update["sessionUpdate"].as_str(),
                Some("terminal_update" | "state_update")
            )
        })
        .cloned()
        .collect()
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
// a Git patch where the path allows one.
// Owner: ACP v2 boundary, over the real stdio server
#[test]
fn t3code_v2_prompt_reports_its_lifecycle_through_state_updates() {
    let workspace = Workspace::new();
    let config = workspace.home.config_path.clone();
    let settings = std::fs::read_to_string(&config)
        .unwrap()
        .replace("permission_mode = \"plan\"", "permission_mode = \"bypass\"");
    std::fs::write(config, settings).unwrap();
    let mut server = AcpServer::from_command(
        AcpServer::command(&workspace.home, "tool-call", &workspace.project).arg("--t3code"),
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
            json!({"sessionUpdate": "state_update", "state": "running", "background": []}),
        ]
    );
    assert_eq!(
        updates.last().unwrap(),
        &json!({
            "sessionUpdate": "state_update",
            "state": "idle",
            "stopReason": "end_turn",
            "background": [],
        })
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
    let mut expected = json!({
        "type": "diff",
        "changes": [{"operation": "add", "path": path, "fileType": "text"}],
    });
    // Windows paths need Git's quoting, so no patch is offered there; the
    // change list stays authoritative.
    if cfg!(unix) {
        expected["patch"] = json!({
            "format": "git_patch",
            "text": format!(
                "diff --git {path} {path}\nnew file mode 100644\n--- /dev/null\n+++ {path}\n@@ -0,0 +1 @@\n+created\n"
            ),
        });
    }
    assert_eq!(diff, &expected);
}

// Covers: v2 reserves an error answer to `session/prompt` for prompts that
// were never inserted, so a refused prompt gets no updates; `session/resume`
// with `replayFrom: start` replays the conversation, keeping each user
// message's ID and resetting each message before its chunks, so a host that
// still holds it does not append the replay to it, and then reports the
// current state before answering; an unknown replay cursor is invalid params.
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
    let (reply, live) = server.prompt_v2(&session_id, "read the shared notes");
    let message_id = reply.unwrap()["messageId"].clone();
    let mut held = BTreeMap::new();
    hold_messages(&mut held, &live);
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
    assert_eq!(
        kinds[..2],
        ["user_message", "user_message_chunk"],
        "{replay:#?}"
    );
    // A replayed user message keeps the ID its prompt was answered with.
    assert_eq!(replay[0]["messageId"], message_id);
    hold_messages(&mut held, &replay);
    assert_eq!(held[message_id.as_str().unwrap()], "read the shared notes");
    assert!(kinds.contains(&"tool_call_update"), "{kinds:?}");
    assert_eq!(
        replay.last().unwrap(),
        &json!({"sessionUpdate": "state_update", "state": "idle", "background": []})
    );
}

// Covers: a v2 host must see the background process a turn leaves running
// (T3 Code's turns do not wait for one): announced as a terminal before the
// idle state lists it, again on resume before its snapshot, then, once it
// exits, its exit status before the repeated idle state that drops it, with
// no new stop reason.
// Owner: ACP v2 boundary and daemon process reporting, over the real stdio
// server
#[test]
fn v2_lists_a_background_process_until_it_exits() {
    let workspace = Workspace::new();
    let config = workspace.home.config_path.clone();
    let settings = std::fs::read_to_string(&config)
        .unwrap()
        .replace("permission_mode = \"plan\"", "permission_mode = \"bypass\"");
    std::fs::write(config, settings).unwrap();
    // The process waits for the test to release it, so it outlives the turn.
    let release = workspace.project.join("release");
    let command = format!(
        "while [ ! -e '{}' ]; do sleep 0.05; done; exit 3",
        release.display()
    );
    let mut server = AcpServer::from_command(
        AcpServer::command(&workspace.home, "process-then-reply", &workspace.project)
            .env("RHO_AUTOMATION_TEST_COMMAND", &command)
            .arg("--t3code"),
    );
    server
        .request("initialize", negotiating_initialize())
        .0
        .unwrap();
    let created = server
        .request("session/new", json!({"cwd": workspace.project}))
        .0
        .unwrap();
    let session_id = created["sessionId"].as_str().unwrap().to_owned();

    let (reply, updates) = server.prompt_v2(&session_id, "start the server");
    reply.unwrap();
    let announced = updates
        .iter()
        .position(|update| update["sessionUpdate"] == "terminal_update")
        .unwrap_or_else(|| panic!("no terminal announced: {updates:#?}"));
    let terminal = updates[announced]["terminalId"].clone();
    let first_listed = updates
        .iter()
        .position(|update| {
            update["sessionUpdate"] == "state_update"
                && update["background"]
                    .as_array()
                    .is_some_and(|work| !work.is_empty())
        })
        .unwrap();
    assert!(announced < first_listed, "{updates:#?}");
    assert_eq!(
        (&updates[announced], updates.last().unwrap()),
        (
            &json!({"sessionUpdate": "terminal_update", "terminalId": terminal, "command": command}),
            &json!({
                "sessionUpdate": "state_update",
                "state": "idle",
                "stopReason": "end_turn",
                "background": [{"type": "terminal", "terminalId": terminal}],
            }),
        )
    );

    // A host that resumes the session, as T3 Code does on start, hears of
    // the terminal again before the snapshot that lists it.
    let (resumed, snapshot) = server.request(
        "session/resume",
        json!({"sessionId": session_id, "cwd": workspace.project}),
    );
    resumed.unwrap();
    assert_eq!(
        background_traffic(&snapshot),
        [
            json!({"sessionUpdate": "terminal_update", "terminalId": terminal, "command": command}),
            json!({
                "sessionUpdate": "state_update",
                "state": "idle",
                "background": [{"type": "terminal", "terminalId": terminal}],
            }),
        ]
    );

    std::fs::write(&release, "").unwrap();
    let mut finished = Vec::new();
    while background_traffic(&finished)
        .last()
        .is_none_or(|update| update["sessionUpdate"] != "state_update")
    {
        let message = server.recv("the background process's exit");
        finished.push(message["params"]["update"].clone());
    }
    assert_eq!(
        background_traffic(&finished),
        [
            json!({"sessionUpdate": "terminal_update", "terminalId": terminal, "exitStatus": {"exitCode": 3}}),
            json!({"sessionUpdate": "state_update", "state": "idle", "background": []}),
        ]
    );
}
