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
/// blocking `bash` call runs unasked, initialized for ACP v2.
fn server(workspace: &Workspace) -> AcpServer {
    let config = workspace.home.config_path.clone();
    let settings = std::fs::read_to_string(&config)
        .unwrap()
        .replace("permission_mode = \"plan\"", "permission_mode = \"bypass\"");
    std::fs::write(config, settings).unwrap();
    let mut server = AcpServer::spawn(&workspace.home, "tool-call", &workspace.project);
    let params = json!({
        "protocolVersion": 2,
        "info": {"name": "idempotency-tests", "version": "0"},
        "capabilities": {},
    });
    server.request("initialize", params).0.unwrap();
    server
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

/// Sends `session/prompt` `id` with `text`, naming `key`, without waiting.
fn send_keyed(server: &mut AcpServer, id: &str, session_id: &str, text: &str, key: Option<&str>) {
    server.send(&json!({
        "jsonrpc": "2.0", "id": id, "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": text}],
            "idempotencyKey": key,
        },
    }));
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

/// Answers once the agent read everything sent before it: the connection
/// handles messages in order. Returns the traffic that came first.
fn barrier(server: &mut AcpServer, log: &mut Vec<Value>) {
    server.send(&json!({
        "jsonrpc": "2.0", "id": "barrier", "method": "session/list", "params": {},
    }));
    read_until(server, log, |message| message["id"] == "barrier");
}

/// Sends prompt `id` naming `key` and reads until it is answered and, when
/// it started a turn, until that turn is idle, then until the agent read
/// everything sent before. Returns the traffic.
fn prompt(server: &mut AcpServer, id: &str, session_id: &str, text: &str, key: &str) -> Vec<Value> {
    send_keyed(server, id, session_id, text, Some(key));
    let mut log = Vec::new();
    read_until(server, &mut log, |message| message["id"] == id);
    if log
        .iter()
        .any(|message| message["params"]["update"]["state"] == "running")
    {
        read_until(server, &mut log, idle);
    }
    barrier(server, &mut log);
    log
}

/// The session's prompt lifecycle in `log`: user messages and states, and
/// each prompt's answer, with its message ID or error code.
fn lifecycle(log: &[Value]) -> Vec<String> {
    log.iter()
        .filter_map(|message| {
            let update = &message["params"]["update"];
            match (message["method"].as_str(), update["sessionUpdate"].as_str()) {
                (Some("session/update"), Some("user_message")) => {
                    Some(format!("user_message {}", update["messageId"].as_str()?))
                }
                (Some("session/update"), Some("state_update")) => {
                    Some(update["state"].as_str()?.to_owned())
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

/// The message ID prompt `id` was answered with.
fn message_id(log: &[Value], id: &str) -> String {
    log.iter()
        .find(|message| message["id"] == id && message.get("method").is_none())
        .and_then(|reply| reply["result"]["messageId"].as_str())
        .unwrap_or_else(|| panic!("prompt {id} was not accepted: {log:#?}"))
        .to_owned()
}

// Covers: a prompt sent again with its `idempotencyKey` inserts nothing and
// is answered with the original `messageId`, with no `user_message` or
// state: for an earlier key after another turn, for a `/compact` the
// session saves no message for, and from another connection, which only
// the daemon can answer.
// Owner: ACP v2 prompt retries, over two real stdio servers and the daemon
#[test]
fn v2_answers_a_retried_prompt_with_its_first_message_id() {
    let workspace = Workspace::new();
    let mut first = server(&workspace);
    let session_id = new_session(&mut first, &workspace);

    let m1 = message_id(&prompt(&mut first, "k1", &session_id, "first", "K1"), "k1");
    let m2 = message_id(&prompt(&mut first, "k2", &session_id, "second", "K2"), "k2");
    let c = message_id(
        &prompt(&mut first, "k3", &session_id, "/compact", "K3"),
        "k3",
    );
    // A later turn, so the `/compact` is not the session's latest.
    prompt(&mut first, "k4", &session_id, "after", "K4");
    let retries = [("r1", "first", "K1"), ("r3", "/compact", "K3")]
        .map(|(id, text, key)| lifecycle(&prompt(&mut first, id, &session_id, text, key)));

    let mut second = server(&workspace);
    second
        .request(
            "session/resume",
            json!({"sessionId": session_id, "cwd": workspace.project}),
        )
        .0
        .unwrap();
    let elsewhere = [
        ("s1", "first", "K1"),
        ("s2", "second", "K2"),
        ("s3", "/compact", "K3"),
    ]
    .map(|(id, text, key)| lifecycle(&prompt(&mut second, id, &session_id, text, key)));

    assert_eq!(
        (retries, elsewhere),
        (
            [
                vec![format!("reply r1 {m1}")],
                vec![format!("reply r3 {c}")]
            ],
            [
                vec![format!("reply s1 {m1}")],
                vec![format!("reply s2 {m2}")],
                vec![format!("reply s3 {c}")],
            ],
        )
    );
}

// Covers: two prompts with one key, held while a turn runs, hold one place:
// both are answered with the one `messageId` the inserted prompt has, which
// a single `user_message` carries.
// Owner: ACP v2 held prompts, over the real stdio server
#[test]
fn v2_held_prompts_with_one_key_insert_one_message() {
    let workspace = Workspace::new();
    let release = workspace.project.join("release");
    let mut server = server(&workspace);
    let session_id = new_session(&mut server, &workspace);

    let mut log = Vec::new();
    send_keyed(
        &mut server,
        "p1",
        &session_id,
        &blocking_prompt(&release),
        None,
    );
    read_until(&server, &mut log, |message| message["id"] == "p1");
    send_keyed(&mut server, "a", &session_id, "held", Some("K"));
    send_keyed(&mut server, "b", &session_id, "held", Some("K"));
    barrier(&mut server, &mut log);
    std::fs::write(&release, "").unwrap();
    let mut answered = 0;
    let mut idles = 0;
    read_until(&server, &mut log, |message| {
        answered += usize::from(["a", "b"].contains(&message["id"].as_str().unwrap_or("")));
        idles += usize::from(idle(message));
        answered == 2 && idles == 2
    });

    let [m1, m] = ["p1", "a"].map(|id| message_id(&log, id));
    let mut lifecycle = lifecycle(&log);
    // The retry is answered once the prompt it follows is, in no fixed
    // place among that prompt's updates.
    let b = lifecycle
        .iter()
        .position(|step| step.starts_with("reply b"))
        .unwrap_or_else(|| panic!("b unanswered: {lifecycle:#?}"));
    assert_eq!(lifecycle.remove(b), format!("reply b {m}"));
    assert_eq!(
        lifecycle,
        [
            format!("user_message {m1}"),
            "running".into(),
            format!("reply p1 {m1}"),
            "idle".into(),
            format!("user_message {m}"),
            "running".into(),
            format!("reply a {m}"),
            "idle".into(),
        ]
    );
}

// Covers: a `$/cancel_request` for a retry withdraws the held prompt it
// follows, so both are answered `-32800` and neither is inserted, and the
// key, unused, inserts the next prompt that names it.
// Owner: ACP v2 held prompts, over the real stdio server
#[test]
fn v2_cancelling_a_retry_withdraws_the_held_prompt_and_frees_its_key() {
    let workspace = Workspace::new();
    let release = workspace.project.join("release");
    let mut server = server(&workspace);
    let session_id = new_session(&mut server, &workspace);

    let mut log = Vec::new();
    send_keyed(
        &mut server,
        "p1",
        &session_id,
        &blocking_prompt(&release),
        None,
    );
    read_until(&server, &mut log, |message| message["id"] == "p1");
    send_keyed(&mut server, "a", &session_id, "held", Some("K"));
    send_keyed(&mut server, "b", &session_id, "held", Some("K"));
    server.send(&json!({
        "jsonrpc": "2.0", "method": "$/cancel_request", "params": {"requestId": "b"},
    }));
    let mut answered = 0;
    read_until(&server, &mut log, |message| {
        answered += usize::from(["a", "b"].contains(&message["id"].as_str().unwrap_or("")));
        answered == 2
    });
    std::fs::write(&release, "").unwrap();
    read_until(&server, &mut log, idle);
    let after = prompt(&mut server, "c", &session_id, "again", "K");
    let m = message_id(&after, "c");
    log.extend(after);

    let [m1] = ["p1"].map(|id| message_id(&log, id));
    let mut lifecycle = lifecycle(&log);
    // Both answers come while p1's turn runs, in no fixed order.
    lifecycle[3..5].sort();
    assert_eq!(
        lifecycle,
        [
            format!("user_message {m1}"),
            "running".into(),
            format!("reply p1 {m1}"),
            "reply a -32800".into(),
            "reply b -32800".into(),
            "idle".into(),
            format!("user_message {m}"),
            "running".into(),
            format!("reply c {m}"),
            "idle".into(),
        ]
    );
}
