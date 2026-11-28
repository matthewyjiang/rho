use std::path::Path;

use pretty_assertions::assert_eq;
use serde_json::{json, Value};

use super::{AcpServer, Workspace};

/// A fixture prompt whose `bash` call runs until `release` exists, so its
/// turn stays busy until the test lets it end.
fn blocking_prompt(release: &Path) -> String {
    let command = format!(
        "while [ ! -e '{}' ]; do sleep 0.05; done",
        release.display()
    );
    json!({"id": "block", "name": "bash", "arguments": {"command": command}}).to_string()
}

/// `rho acp` on the fixture's tool-call mode in bypass mode, so the
/// blocking `bash` call runs unasked.
fn server(workspace: &Workspace) -> AcpServer {
    let config = workspace.home.config_path.clone();
    let settings = std::fs::read_to_string(&config)
        .unwrap()
        .replace("permission_mode = \"plan\"", "permission_mode = \"bypass\"");
    std::fs::write(config, settings).unwrap();
    AcpServer::spawn(&workspace.home, "tool-call", &workspace.project)
}

fn initialize(server: &mut AcpServer, version: u16) {
    let params = match version {
        1 => json!({"protocolVersion": 1, "clientCapabilities": {}}),
        _ => json!({
            "protocolVersion": 2,
            "info": {"name": "prompt-tests", "version": "0"},
            "capabilities": {},
        }),
    };
    let initialized = server.request("initialize", params).0.unwrap();
    assert_eq!(initialized["protocolVersion"], version);
}

fn new_session(server: &mut AcpServer, workspace: &Workspace) -> String {
    let created = server
        .request(
            "session/new",
            json!({"cwd": workspace.project, "mcpServers": []}),
        )
        .0
        .unwrap();
    created["sessionId"].as_str().unwrap().to_owned()
}

/// Sends `session/prompt` with request id `id` without waiting for it.
fn send_prompt(server: &mut AcpServer, id: &str, session_id: &str, text: &str) {
    server.send(&json!({
        "jsonrpc": "2.0", "id": id, "method": "session/prompt",
        "params": {"sessionId": session_id, "prompt": [{"type": "text", "text": text}]},
    }));
}

/// Answers once the agent read everything sent before it: the connection
/// handles messages in order. Returns the traffic that came first.
fn barrier(server: &mut AcpServer, log: &mut Vec<Value>) {
    server.send(&json!({
        "jsonrpc": "2.0", "id": "barrier", "method": "session/list", "params": {},
    }));
    read_until(server, log, |message| message["id"] == "barrier");
}

/// Appends the agent's messages to `log` until `done` holds for one.
fn read_until(server: &AcpServer, log: &mut Vec<Value>, mut done: impl FnMut(&Value) -> bool) {
    loop {
        let message = server.recv("prompt traffic");
        let finished = done(&message);
        log.push(message);
        if finished {
            return;
        }
    }
}

fn idle(message: &Value) -> bool {
    message["params"]["update"]["state"] == "idle"
}

/// The session's prompt lifecycle in `log`, in order: each user message
/// with its ID, each state, and each prompt's answer by request id, with
/// its message ID or error code.
fn lifecycle(log: &[Value]) -> Vec<String> {
    log.iter()
        .filter_map(|message| {
            let update = &message["params"]["update"];
            match (message["method"].as_str(), update["sessionUpdate"].as_str()) {
                (Some("session/update"), Some("user_message")) => {
                    Some(format!("user_message {}", update["messageId"].as_str()?))
                }
                (Some("session/update"), Some("state_update")) => {
                    Some(match update["stopReason"].as_str() {
                        Some(stop) => format!("{} {stop}", update["state"].as_str()?),
                        None => update["state"].as_str()?.to_owned(),
                    })
                }
                (None, _) if message["id"] != "barrier" => {
                    let answer = match message.get("error") {
                        Some(error) => error["code"].to_string(),
                        None => message["result"]["messageId"].as_str()?.to_owned(),
                    };
                    Some(format!("reply {} {answer}", message["id"].as_str()?))
                }
                _ => None,
            }
        })
        .collect()
}

/// The message ID `id`'s prompt was answered with.
fn message_id(log: &[Value], id: &str) -> String {
    log.iter()
        .find(|message| message["id"] == id && message.get("method").is_none())
        .and_then(|reply| reply["result"]["messageId"].as_str())
        .unwrap_or_else(|| panic!("prompt {id} was not accepted: {log:#?}"))
        .to_owned()
}

// Covers: ACP v2 must not reject a prompt sent while the session's turn
// runs: Rho holds each one, its answer pending, and inserts them one by one
// in the order they arrived, each as a new turn after the last one's idle.
// Owner: ACP v2 prompt lifecycle, over the real stdio server
#[test]
fn v2_holds_prompts_sent_during_a_turn_and_inserts_them_in_order() {
    let workspace = Workspace::new();
    let release = workspace.project.join("release");
    let mut server = server(&workspace);
    initialize(&mut server, 2);
    let session_id = new_session(&mut server, &workspace);

    let mut log = Vec::new();
    send_prompt(&mut server, "p1", &session_id, &blocking_prompt(&release));
    read_until(&server, &mut log, |message| message["id"] == "p1");
    send_prompt(&mut server, "p2", &session_id, "second");
    send_prompt(&mut server, "p3", &session_id, "third");
    barrier(&mut server, &mut log);
    std::fs::write(&release, "").unwrap();
    let mut answered = 0;
    let mut idles = 0;
    read_until(&server, &mut log, |message| {
        answered += usize::from(["p2", "p3"].contains(&message["id"].as_str().unwrap_or("")));
        idles += usize::from(idle(message));
        answered == 2 && idles == 3
    });

    let [m1, m2, m3] = ["p1", "p2", "p3"].map(|id| message_id(&log, id));
    assert_eq!(
        lifecycle(&log),
        [
            format!("user_message {m1}"),
            "running".into(),
            format!("reply p1 {m1}"),
            "idle end_turn".into(),
            format!("user_message {m2}"),
            "running".into(),
            format!("reply p2 {m2}"),
            "idle end_turn".into(),
            format!("user_message {m3}"),
            "running".into(),
            format!("reply p3 {m3}"),
            "idle end_turn".into(),
        ]
    );
}

// Covers: `session/cancel` covers the prompts held when it arrives, and
// `$/cancel_request` the one it names: each is answered `-32800` and never
// inserted, while a prompt sent after the cancel runs once the cancelled
// turn is idle.
// Owner: ACP v2 prompt lifecycle, over the real stdio server
#[test]
fn v2_cancels_answer_held_prompts_cancelled_and_never_insert_them() {
    let workspace = Workspace::new();
    let release = workspace.project.join("release");
    let mut server = server(&workspace);
    initialize(&mut server, 2);
    let session_id = new_session(&mut server, &workspace);

    let mut log = Vec::new();
    send_prompt(&mut server, "p1", &session_id, &blocking_prompt(&release));
    read_until(&server, &mut log, |message| message["id"] == "p1");
    send_prompt(&mut server, "p2", &session_id, "dropped by session/cancel");
    send_prompt(
        &mut server,
        "p3",
        &session_id,
        "dropped by $/cancel_request",
    );
    server.send(&json!({
        "jsonrpc": "2.0", "method": "$/cancel_request", "params": {"requestId": "p3"},
    }));
    read_until(&server, &mut log, |message| message["id"] == "p3");
    server.send(&json!({
        "jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": session_id},
    }));
    send_prompt(&mut server, "p4", &session_id, "after the cancel");
    let mut idles = 0;
    read_until(&server, &mut log, |message| {
        idles += usize::from(idle(message));
        idles == 2
    });

    let [m1, m4] = ["p1", "p4"].map(|id| message_id(&log, id));
    let mut lifecycle = lifecycle(&log);
    // The cancel answers p2 as it arrives, while the cancelled turn winds
    // down, so its answer has no fixed place before p4 starts.
    let p2 = lifecycle
        .iter()
        .position(|step| step.starts_with("reply p2"))
        .unwrap_or_else(|| panic!("p2 unanswered: {lifecycle:#?}"));
    assert_eq!(lifecycle.remove(p2), "reply p2 -32800");
    assert_eq!(
        lifecycle,
        [
            format!("user_message {m1}"),
            "running".into(),
            format!("reply p1 {m1}"),
            "reply p3 -32800".into(),
            "idle cancelled".into(),
            format!("user_message {m4}"),
            "running".into(),
            format!("reply p4 {m4}"),
            "idle end_turn".into(),
        ]
    );
}

// Covers: a prompt sent while another client's turn runs in the session,
// which the daemon refuses as busy, is held until that turn ends and then
// inserted, instead of failing.
// Owner: ACP v2 prompt lifecycle with the daemon, over two real stdio servers
#[test]
fn v2_holds_a_prompt_behind_another_clients_turn() {
    let workspace = Workspace::new();
    let release = workspace.project.join("release");
    let mut first = server(&workspace);
    initialize(&mut first, 2);
    let session_id = new_session(&mut first, &workspace);
    let mut first_log = Vec::new();
    send_prompt(&mut first, "p1", &session_id, &blocking_prompt(&release));
    read_until(&first, &mut first_log, |message| message["id"] == "p1");

    let mut second = AcpServer::spawn(&workspace.home, "tool-call", &workspace.project);
    initialize(&mut second, 2);
    let (resumed, snapshot) = second.request(
        "session/resume",
        json!({"sessionId": session_id, "cwd": workspace.project}),
    );
    resumed.unwrap();
    assert_eq!(
        snapshot.last().unwrap(),
        &json!({"sessionUpdate": "state_update", "state": "running", "background": []})
    );
    let mut log = Vec::new();
    send_prompt(
        &mut second,
        "p2",
        &session_id,
        "after the other client's turn",
    );
    barrier(&mut second, &mut log);
    std::fs::write(&release, "").unwrap();
    let mut idles = 0;
    read_until(&second, &mut log, |message| {
        idles += usize::from(idle(message));
        idles == 2
    });

    let m2 = message_id(&log, "p2");
    assert_eq!(
        lifecycle(&log),
        [
            "idle end_turn".to_owned(),
            format!("user_message {m2}"),
            "running".into(),
            format!("reply p2 {m2}"),
            "idle end_turn".into(),
        ]
    );
}

// Covers: ACP v1 keeps refusing a prompt sent while the session's turn
// runs, as v1 hosts expect: it fails as an invalid request at once.
// Owner: ACP v1 prompt, over the real stdio server
#[test]
fn v1_refuses_a_prompt_sent_during_a_turn() {
    let workspace = Workspace::new();
    let release = workspace.project.join("release");
    let mut server = server(&workspace);
    initialize(&mut server, 1);
    let session_id = new_session(&mut server, &workspace);

    let mut log = Vec::new();
    send_prompt(&mut server, "p1", &session_id, &blocking_prompt(&release));
    read_until(&server, &mut log, |message| {
        message["params"]["update"]["sessionUpdate"] == "tool_call"
    });
    send_prompt(&mut server, "p2", &session_id, "second");
    read_until(&server, &mut log, |message| message["id"] == "p2");
    std::fs::write(&release, "").unwrap();
    read_until(&server, &mut log, |message| message["id"] == "p1");

    let replies: Vec<_> = log
        .iter()
        .filter(|message| message.get("method").is_none())
        .map(|reply| {
            (
                reply["id"].clone(),
                reply["error"]["code"].clone(),
                reply["result"].clone(),
            )
        })
        .collect();
    assert_eq!(
        replies,
        [
            (json!("p2"), json!(-32600), Value::Null),
            (json!("p1"), Value::Null, json!({"stopReason": "end_turn"})),
        ]
    );
}
