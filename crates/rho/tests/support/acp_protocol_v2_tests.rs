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
        let exchange = self.prompt_v2_answering(session_id, text, |request| {
            panic!("the prompt asked the host: {request}")
        });
        (exchange.reply, exchange.updates)
    }

    /// [`Self::prompt_v2`], answering each request the agent sends the host
    /// with `answer`'s result.
    pub(super) fn prompt_v2_answering(
        &mut self,
        session_id: &str,
        text: &str,
        answer: impl Fn(&Value) -> Value,
    ) -> V2Exchange {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({
            "jsonrpc": "2.0", "id": id, "method": "session/prompt",
            "params": {"sessionId": session_id, "prompt": [{"type": "text", "text": text}]},
        }));
        let mut reply = None;
        let mut updates = Vec::new();
        let mut asked = Vec::new();
        loop {
            let message = self.recv("v2 prompt traffic");
            match message["method"].as_str() {
                Some("session/update") => updates.push(message["params"]["update"].clone()),
                Some(_) => {
                    self.send(&json!({
                        "jsonrpc": "2.0", "id": message["id"], "result": answer(&message),
                    }));
                    asked.push(message);
                }
                None => {
                    assert_eq!(message["id"], id, "{message}");
                    let answer = match message.get("error") {
                        Some(error) => Err(error.clone()),
                        None => Ok(message["result"].clone()),
                    };
                    // A refused prompt is never inserted, so no `idle` follows.
                    if answer.is_err() {
                        return V2Exchange {
                            reply: answer,
                            updates,
                            asked,
                        };
                    }
                    reply = Some(answer);
                }
            }
            let idle = updates.last().is_some_and(|update| {
                update["sessionUpdate"] == "state_update" && update["state"] == "idle"
            });
            if let (Some(reply), true) = (&reply, idle) {
                return V2Exchange {
                    reply: reply.clone(),
                    updates,
                    asked,
                };
            }
        }
    }
}

/// A v2 prompt's traffic: the reply, the updates until `idle`, and the
/// requests the agent sent the host.
pub(super) struct V2Exchange {
    pub(super) reply: Result<Value, Value>,
    pub(super) updates: Vec<Value>,
    pub(super) asked: Vec<Value>,
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
                Some("terminal_update" | "tool_call_update" | "subagent_update" | "state_update")
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

/// Puts the workspace's sessions in bypass mode, so tools run unasked.
fn bypass(workspace: &Workspace) {
    let config = workspace.home.config_path.clone();
    let settings = std::fs::read_to_string(&config)
        .unwrap()
        .replace("permission_mode = \"plan\"", "permission_mode = \"bypass\"");
    std::fs::write(config, settings).unwrap();
}

/// `rho acp --t3code` with one v2 session whose prompts start `command` as
/// a background process, which T3 Code's turns do not wait for.
fn t3code_process_session(workspace: &Workspace, command: &str) -> (AcpServer, String) {
    bypass(workspace);
    let mut server = AcpServer::from_command(
        AcpServer::command(&workspace.home, "process-then-reply", &workspace.project)
            .env("RHO_AUTOMATION_TEST_COMMAND", command)
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
    (server, session_id)
}

/// Which `rho acp` a test drives.
#[derive(Clone, Copy, Debug)]
enum Host {
    Plain,
    /// `rho acp --t3code`, whose children run as T3 threads.
    T3Code,
}

/// `rho acp` as `host` in bypass mode on fixture `mode`, with one v2
/// session.
fn v2_session(workspace: &Workspace, mode: &str, host: Host) -> (AcpServer, String) {
    bypass(workspace);
    let mut command = AcpServer::command(&workspace.home, mode, &workspace.project);
    match host {
        Host::Plain => {}
        Host::T3Code => {
            command.arg("--t3code");
        }
    }
    let mut server = AcpServer::from_command(&mut command);
    server
        .request("initialize", negotiating_initialize())
        .0
        .unwrap();
    let created = server
        .request("session/new", json!({"cwd": workspace.project}))
        .0
        .unwrap();
    let session_id = created["sessionId"].as_str().unwrap().to_owned();
    (server, session_id)
}

/// A workspace workflow, `wait-run`, whose one agent node, on the fixture
/// model, runs a command that waits until the test creates the returned
/// file. An agent node, not a `shell` one, so it runs where Rho cannot
/// launch frozen workflow processes (macOS).
fn waiting_workflow(workspace: &Workspace) -> std::path::PathBuf {
    let release = workspace.project.join("release");
    let wait = json!({
        "id": "wait", "name": "bash",
        "arguments": {"command": format!("while [ ! -e '{}' ]; do sleep 0.05; done", release.display())},
    });
    let workflow = workspace.project.join(".rho/workflows/wait-run");
    std::fs::create_dir_all(workflow.join("agents")).unwrap();
    std::fs::write(
        workflow.join("agents/waiter.md"),
        "---\ndescription: Fixture agent that waits\n---\n\nDo the task.\n",
    )
    .unwrap();
    // A Starlark string literal holding the call's JSON.
    let prompt = serde_json::to_string(&wait.to_string()).unwrap();
    std::fs::write(
        workflow.join("workflow.star"),
        format!(
            r#"def build(inputs):
    return workflow(name = "wait-run", nodes = [
        agent(
            name = "wait",
            agent = "waiter",
            prompt = {prompt},
            access = "mutating",
            timeout_seconds = 600,
            max_output_bytes = 100000,
        ),
    ])

WORKFLOW = define(inputs = {{}}, build = build)
"#
        ),
    )
    .unwrap();
    release
}

/// Starts `wait-run` with `/workflow:`, returning its tool call's id once
/// the prompt's `idle` lists it, after asserting the announcement before.
fn start_waiting_workflow(server: &mut AcpServer, session_id: &str) -> Value {
    let (reply, updates) = server.prompt_v2(session_id, "/workflow:wait-run");
    reply.unwrap();
    let announced = updates
        .iter()
        .find(|update| update["sessionUpdate"] == "tool_call_update")
        .unwrap_or_else(|| panic!("no workflow announced: {updates:#?}"));
    let call = announced["toolCallId"].clone();
    assert_eq!(
        (announced, updates.last().unwrap()),
        (
            &json!({
                "sessionUpdate": "tool_call_update", "toolCallId": call, "title": "wait-run",
                "kind": "other", "status": "in_progress", "capabilities": {"cancel": {}},
            }),
            &json!({
                "sessionUpdate": "state_update", "state": "idle", "stopReason": "end_turn",
                "background": [{"type": "tool_call", "toolCallId": call}],
            }),
        ),
        "{updates:#?}"
    );
    call
}

/// The work updates and state a cancel answered with `early` brought,
/// through the first state: those that came with the answer, then, until
/// the state arrives, those read after it. The answer only accepts the
/// cancel, so it may come before, between, or after them, and a turn the
/// daemon starts to deliver the result may follow the state.
fn stopped(server: &AcpServer, early: &[Value], kinds: &[&str], what: &str) -> Vec<Value> {
    let mut traffic = background_traffic(early);
    match traffic
        .iter()
        .position(|update| update["sessionUpdate"] == "state_update")
    {
        Some(state) => traffic.truncate(state + 1),
        None => traffic.extend(until_state(server, kinds, what)),
    }
    traffic
}

/// Reads updates until the next `state_update`, returning those of `kinds`
/// and the state.
fn until_state(server: &AcpServer, kinds: &[&str], what: &str) -> Vec<Value> {
    let mut seen = Vec::new();
    loop {
        let message = server.recv(what);
        assert_eq!(message["method"], "session/update", "{message}");
        let update = message["params"]["update"].clone();
        let kind = update["sessionUpdate"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        if kind == "state_update" {
            seen.push(update);
            return seen;
        }
        if kinds.contains(&kind.as_str()) {
            seen.push(update);
        }
    }
}

// Covers: a v2 host must see the background process a turn leaves running
// (T3 Code's turns do not wait for one): announced as a cancellable
// terminal before the idle state lists it, again on resume before its
// snapshot, then, once it exits, its exit status before the repeated idle
// state that drops it, with no new stop reason.
// Owner: ACP v2 boundary and daemon background reporting, over the real
// stdio server
#[test]
fn v2_lists_a_background_process_until_it_exits() {
    let workspace = Workspace::new();
    // The process waits for the test to release it, so it outlives the turn.
    let release = workspace.project.join("release");
    let command = format!(
        "while [ ! -e '{}' ]; do sleep 0.05; done; exit 3",
        release.display()
    );
    let (mut server, session_id) = t3code_process_session(&workspace, &command);

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
    let announcement = json!({
        "sessionUpdate": "terminal_update", "terminalId": terminal, "command": command,
        "capabilities": {"cancel": {}},
    });
    let listed = json!([{"type": "terminal", "terminalId": terminal}]);
    assert_eq!(
        (&updates[announced], updates.last().unwrap()),
        (
            &announcement,
            &json!({
                "sessionUpdate": "state_update", "state": "idle", "stopReason": "end_turn",
                "background": listed,
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
            announcement,
            json!({"sessionUpdate": "state_update", "state": "idle", "background": listed}),
        ]
    );

    std::fs::write(&release, "").unwrap();
    assert_eq!(
        until_state(
            &server,
            &["terminal_update"],
            "the background process's exit"
        ),
        [
            json!({
                "sessionUpdate": "terminal_update", "terminalId": terminal,
                "exitStatus": {"exitCode": 3}, "capabilities": null,
            }),
            json!({"sessionUpdate": "state_update", "state": "idle", "background": []}),
        ]
    );
}

// Covers: a v2 host that cancels a running background terminal gets an
// accepting answer, then the terminal's exit status before the state that
// drops it; cancelling it again, once it exited, is invalid params.
// Owner: ACP v2 boundary and daemon background stop, over the real stdio
// server
#[test]
fn v2_cancels_a_background_terminal() {
    let workspace = Workspace::new();
    let (mut server, session_id) = t3code_process_session(&workspace, "sleep 600");
    let (reply, updates) = server.prompt_v2(&session_id, "start the server");
    reply.unwrap();
    let terminal = updates.last().unwrap()["background"][0]["terminalId"].clone();
    assert!(terminal.is_string(), "{updates:#?}");
    let cancel = json!({"sessionId": session_id, "terminalId": terminal});

    let (accepted, early) = server.request("terminal/cancel", cancel.clone());
    let stopped = stopped(
        &server,
        &early,
        &["terminal_update"],
        "the cancelled terminal's exit",
    );
    let (again, _) = server.request("terminal/cancel", cancel);

    assert_eq!(
        (
            accepted,
            stopped,
            again.map_err(|error| error["code"].clone())
        ),
        (
            Ok(json!({})),
            vec![
                json!({
                    "sessionUpdate": "terminal_update", "terminalId": terminal,
                    "exitStatus": {}, "capabilities": null,
                }),
                json!({"sessionUpdate": "state_update", "state": "idle", "background": []}),
            ],
            Err(json!(-32602)),
        )
    );
}

// Covers: a v2 host, plain or T3 Code, must see a workflow run
// `/workflow:` starts (whose prompt ends at once): announced as a
// cancellable tool call before the idle state lists it, then, once the run
// ends, `completed` before the state that drops it.
// Owner: ACP v2 boundary and daemon background reporting, over the real
// stdio server
#[test]
fn v2_lists_a_running_workflow_until_it_ends() {
    for host in [Host::Plain, Host::T3Code] {
        let workspace = Workspace::new();
        let release = waiting_workflow(&workspace);
        let (mut server, session_id) = v2_session(&workspace, "tool-call", host);
        let call = start_waiting_workflow(&mut server, &session_id);

        std::fs::write(&release, "").unwrap();
        assert_eq!(
            until_state(&server, &["tool_call_update"], "the workflow run's end"),
            [
                json!({
                    "sessionUpdate": "tool_call_update", "toolCallId": call,
                    "status": "completed", "capabilities": null,
                }),
                json!({"sessionUpdate": "state_update", "state": "idle", "background": []}),
            ],
            "{host:?}"
        );
    }
}

// Covers: a v2 host that cancels a running workflow's tool call gets an
// accepting answer, then the call `cancelled` before the state that drops
// it; cancelling it again, once the run ended, is invalid params.
// Owner: ACP v2 boundary and daemon background stop, over the real stdio
// server
#[test]
fn v2_cancels_a_background_workflow() {
    let workspace = Workspace::new();
    waiting_workflow(&workspace);
    let (mut server, session_id) = v2_session(&workspace, "tool-call", Host::Plain);
    let call = start_waiting_workflow(&mut server, &session_id);
    let cancel = json!({"sessionId": session_id, "toolCallId": call});

    let (accepted, early) = server.request("tool_call/cancel", cancel.clone());
    let stopped = stopped(
        &server,
        &early,
        &["tool_call_update"],
        "the cancelled run's end",
    );
    let (again, _) = server.request("tool_call/cancel", cancel);

    assert_eq!(
        (
            accepted,
            stopped,
            again.map_err(|error| error["code"].clone())
        ),
        (
            Ok(json!({})),
            vec![
                json!({
                    "sessionUpdate": "tool_call_update", "toolCallId": call,
                    "status": "cancelled", "capabilities": null,
                }),
                json!({"sessionUpdate": "state_update", "state": "idle", "background": []}),
            ],
            Err(json!(-32602)),
        )
    );
}

/// Delegates to a worker subagent that, on the same fixture model, runs a
/// command waiting until the test creates the returned file. Returns the
/// subagent's ACP session id once the prompt's `idle` lists it, after
/// asserting its announcement before.
fn start_waiting_subagent(
    workspace: &Workspace,
    server: &mut AcpServer,
    session_id: &str,
) -> (Value, std::path::PathBuf) {
    let release = workspace.project.join("release");
    let wait = json!({
        "id": "wait", "name": "bash",
        "arguments": {"command": format!("while [ ! -e '{}' ]; do sleep 0.05; done", release.display())},
    });
    let delegate = json!({
        "id": "delegate", "name": "agent",
        "arguments": {"agent_id": "worker", "prompt": wait.to_string()},
    });
    let (reply, updates) = server.prompt_v2(session_id, &delegate.to_string());
    reply.unwrap();
    let announced = updates
        .iter()
        .find(|update| update["sessionUpdate"] == "subagent_update")
        .unwrap_or_else(|| panic!("no subagent announced: {updates:#?}"));
    let child = announced["sessionId"].clone();
    assert_eq!(
        (announced, updates.last().unwrap()),
        (
            &json!({
                "sessionUpdate": "subagent_update", "sessionId": child,
                "title": wait.to_string(), "state": {"state": "running"},
                "capabilities": {"cancel": {}},
            }),
            &json!({
                "sessionUpdate": "state_update", "state": "idle", "stopReason": "end_turn",
                "background": [{"type": "subagent", "sessionId": child}],
            }),
        ),
        "{updates:#?}"
    );
    (child, release)
}

// Covers: `session/cancel` on a running subagent's session id stops that
// subagent, not the parent: it ends `idle` with `cancelled` before the
// state that drops it, and its cancelled result still reaches the parent
// in a delivery turn.
// Owner: ACP v2 boundary and daemon background stop, over the real stdio
// server
#[test]
fn v2_cancels_a_background_subagent() {
    let workspace = Workspace::new();
    let (mut server, session_id) = v2_session(&workspace, "tool-call", Host::Plain);
    let (child, _release) = start_waiting_subagent(&workspace, &mut server, &session_id);

    server.send(
        &json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": child}}),
    );
    let stopped = until_state(
        &server,
        &["subagent_update"],
        "the cancelled subagent's end",
    );
    let delivery = until_state(&server, &[], "the delivery turn's start");

    assert_eq!(
        (stopped, delivery),
        (
            vec![
                json!({
                    "sessionUpdate": "subagent_update", "sessionId": child,
                    "state": {"state": "idle", "stopReason": "cancelled"}, "capabilities": null,
                }),
                json!({"sessionUpdate": "state_update", "state": "idle", "background": []}),
            ],
            vec![json!({"sessionUpdate": "state_update", "state": "running", "background": []})],
        )
    );
}

// Covers: a v2 host must see a subagent its turn leaves running (a v2
// prompt does not wait for one, as the daemon delivers its result in a turn
// of its own): announced as a cancellable child session before the idle
// state lists it, then, once it finished, its idle state before the state
// that drops it, and the daemon's delivery turn as `running`, then `idle`.
// Owner: ACP v2 boundary and daemon background reporting, over the real
// stdio server
#[test]
fn v2_lists_a_running_subagent_until_it_ends() {
    let workspace = Workspace::new();
    let (mut server, session_id) = v2_session(&workspace, "tool-call", Host::Plain);
    let (child, release) = start_waiting_subagent(&workspace, &mut server, &session_id);

    std::fs::write(&release, "").unwrap();
    let ended = until_state(&server, &["subagent_update"], "the subagent's end");
    let delivery = until_state(&server, &[], "the delivery turn's start");
    let mut delivered = until_state(&server, &[], "the delivery turn's end");
    // The turn's stop reason is the fixture's, not this test's.
    delivered[0].as_object_mut().unwrap().remove("stopReason");
    assert_eq!(
        (ended, delivery, delivered),
        (
            vec![
                json!({
                    "sessionUpdate": "subagent_update", "sessionId": child,
                    "state": {"state": "idle", "stopReason": "end_turn"}, "capabilities": null,
                }),
                json!({"sessionUpdate": "state_update", "state": "idle", "background": []}),
            ],
            vec![json!({"sessionUpdate": "state_update", "state": "running", "background": []})],
            vec![json!({"sessionUpdate": "state_update", "state": "idle", "background": []})],
        )
    );
}

// Covers: a v2 host must see `requires_action` while the turn waits on its
// permission answer, and `running` once the turn goes on, not a turn that
// looks busy throughout.
// Owner: ACP v2 boundary, over the real stdio server
#[test]
fn v2_reports_requires_action_while_a_permission_waits() {
    let workspace = Workspace::new();
    let mut server = AcpServer::from_command(&mut AcpServer::command(
        &workspace.home,
        "tool-call",
        &workspace.project,
    ));
    server
        .request("initialize", negotiating_initialize())
        .0
        .unwrap();
    let created = server
        .request("session/new", json!({"cwd": workspace.project}))
        .0
        .unwrap();
    let session_id = created["sessionId"].as_str().unwrap().to_owned();
    // The session opens in plan mode, where leaving it asks the host.
    let call = json!({
        "id": "plan", "name": "exit_plan_mode",
        "arguments": {"plan": "1. Change the thing."},
    });

    let exchange = server.prompt_v2_answering(
        &session_id,
        &call.to_string(),
        |_| json!({"outcome": {"outcome": "selected", "optionId": "keep_planning"}}),
    );

    exchange.reply.unwrap();
    let methods: Vec<_> = exchange
        .asked
        .iter()
        .map(|request| request["method"].clone())
        .collect();
    let states: Vec<_> = exchange
        .updates
        .iter()
        .filter(|update| update["sessionUpdate"] == "state_update")
        .map(|update| update["state"].clone())
        .collect();
    assert_eq!(
        (methods, states),
        (
            vec![json!("session/request_permission")],
            vec![
                json!("running"),
                json!("requires_action"),
                json!("running"),
                json!("idle")
            ],
        )
    );
}
