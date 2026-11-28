use std::{
    path::Path,
    time::{Duration, Instant},
};

use pretty_assertions::assert_eq;
use serde_json::{json, Value};

use super::{AcpServer, Workspace};

/// Failure bound for the stopped process to exit once close answered.
const EXIT_TIMEOUT: Duration = Duration::from_secs(30);

/// `rho acp` on the fixture's tool-call mode in bypass mode, so the tools
/// run unasked, initialized for ACP v2 by a host with form elicitation,
/// with one session open.
fn v2_session(workspace: &Workspace) -> (AcpServer, String) {
    let config = workspace.home.config_path.clone();
    let settings = std::fs::read_to_string(&config)
        .unwrap()
        .replace("permission_mode = \"plan\"", "permission_mode = \"bypass\"");
    std::fs::write(config, settings).unwrap();
    let mut server = AcpServer::spawn(&workspace.home, "tool-call", &workspace.project);
    let initialized = server
        .request(
            "initialize",
            json!({
                "protocolVersion": 2,
                "info": {"name": "close-tests", "version": "0"},
                "capabilities": {"elicitation": {"form": {}}},
            }),
        )
        .0
        .unwrap();
    assert_eq!(initialized["protocolVersion"], 2);
    let created = server
        .request(
            "session/new",
            json!({"cwd": workspace.project, "mcpServers": []}),
        )
        .0
        .unwrap();
    let session_id = created["sessionId"].as_str().unwrap().to_owned();
    (server, session_id)
}

/// Sends request `id` without waiting for its answer.
fn send(server: &mut AcpServer, id: &str, method: &str, params: Value) {
    server.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
}

fn send_prompt(server: &mut AcpServer, id: &str, session_id: &str, text: &str) {
    let prompt = json!({"sessionId": session_id, "prompt": [{"type": "text", "text": text}]});
    send(server, id, "session/prompt", prompt);
}

fn tool_call(name: &str, arguments: Value) -> String {
    json!({"id": name, "name": name, "arguments": arguments}).to_string()
}

/// Appends the agent's messages to `log` until `done` holds for one.
fn read_until(server: &AcpServer, log: &mut Vec<Value>, mut done: impl FnMut(&Value) -> bool) {
    loop {
        let message = server.recv("close traffic");
        let finished = done(&message);
        log.push(message);
        if finished {
            return;
        }
    }
}

fn reply_to(id: &str) -> impl Fn(&Value) -> bool + '_ {
    move |message| message["id"] == id && message.get("method").is_none()
}

/// The session's states, the answers and the withdrawn requests in `log`,
/// in order: each state with its stop reason and the background it lists,
/// each answer by request id with its error code, or `ok`, and each
/// `$/cancel_request` by the request it withdraws.
fn lifecycle(log: &[Value]) -> Vec<String> {
    log.iter()
        .filter_map(|message| {
            let update = &message["params"]["update"];
            if message["method"] == "$/cancel_request" {
                return Some(format!("withdraw {}", message["params"]["requestId"]));
            }
            if message["method"] == "session/update" {
                if update["sessionUpdate"] != "state_update" {
                    return None;
                }
                let listed = update["background"].as_array().map_or(0, Vec::len);
                return Some(match update["stopReason"].as_str() {
                    Some(stop) => format!("{} {stop} [{listed}]", update["state"].as_str()?),
                    None => format!("{} [{listed}]", update["state"].as_str()?),
                });
            }
            let code = match message.get("error") {
                Some(error) => error["code"].to_string(),
                None => "ok".to_owned(),
            };
            Some(format!("reply {} {code}", message["id"].as_str()?))
        })
        .collect()
}

/// Whether process `pid` still runs; on Linux, a zombie the daemon has yet
/// to reap counts as ended.
fn running(pid: libc::pid_t) -> bool {
    // SAFETY: `kill` takes plain integers; signal 0 checks that `pid`
    // exists without sending anything.
    if unsafe { libc::kill(pid, 0) } != 0 {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            stat.rsplit_once(") ")
                .is_none_or(|(_, rest)| !rest.starts_with("Z "))
        })
    }
    #[cfg(not(target_os = "linux"))]
    true
}

/// Kills the sleeper still running when dropped, so a failed test leaves
/// none behind.
struct Sleeper(libc::pid_t);

impl Drop for Sleeper {
    fn drop(&mut self) {
        if running(self.0) {
            // SAFETY: plain integer call; a pid that ended meanwhile is
            // signalled in vain.
            unsafe { libc::kill(self.0, libc::SIGKILL) };
        }
    }
}

fn wait_for_pid(path: &Path) -> libc::pid_t {
    let deadline = Instant::now() + EXIT_TIMEOUT;
    loop {
        if let Some(pid) = std::fs::read_to_string(path)
            .ok()
            .and_then(|pid| pid.trim().parse().ok())
        {
            return pid;
        }
        assert!(Instant::now() < deadline, "the process never wrote its pid");
        std::thread::sleep(Duration::from_millis(20));
    }
}

// Covers: ACP v2 `session/close` stops the session's work before it
// answers: the held prompt answers -32800, the turn under way ends idle
// `cancelled` and its open elicitation is withdrawn (`$/cancel_request`)
// before the close's answer, the background process the last state listed
// is stopped, and the agent sends nothing for the session after the
// answer. ACP v1 only detaches (`session_lifecycle`).
// Owner: ACP v2 session close, over the real stdio server
#[test]
fn v2_close_cancels_the_turn_and_stops_background_work_before_answering() {
    let workspace = Workspace::new();
    let pid_file = workspace.project.join("background.pid");
    let (mut server, session_id) = v2_session(&workspace);
    let mut log = Vec::new();

    let background = format!("echo $$ > '{}' && exec sleep 600", pid_file.display());
    let start = tool_call("process", json!({"action": "start", "command": background}));
    send_prompt(&mut server, "background", &session_id, &start);
    read_until(&server, &mut log, |message| {
        message["params"]["update"]["stopReason"] == "end_turn"
    });
    let pid = wait_for_pid(&pid_file);
    let _leftover = Sleeper(pid);
    let ask = json!({"questions": [{
        "id": "color",
        "question": "Which color?",
        "choices": ["Red", "Blue"],
    }]});
    send_prompt(
        &mut server,
        "foreground",
        &session_id,
        &tool_call("questionnaire", ask),
    );
    // The question and the state it puts the turn in, in either order.
    let (mut asked, mut waiting) = (false, false);
    read_until(&server, &mut log, |message| {
        asked |= message["method"] == "elicitation/create";
        waiting |= message["params"]["update"]["state"] == "requires_action";
        asked && waiting
    });
    let elicitation = log
        .iter()
        .find(|message| message["method"] == "elicitation/create")
        .unwrap()["id"]
        .clone();
    send_prompt(&mut server, "held", &session_id, "held behind the turn");
    send(
        &mut server,
        "close",
        "session/close",
        json!({"sessionId": session_id}),
    );
    let mut close = Vec::new();
    let mut answered = 0;
    read_until(&server, &mut close, |message| {
        answered += usize::from(message["id"] == "held" || message["id"] == "close");
        answered == 2
    });
    // Answers once the agent read everything sent before it.
    send(&mut server, "barrier", "session/list", json!({}));
    let mut after = Vec::new();
    read_until(&server, &mut after, reply_to("barrier"));
    let until_close = close
        .iter()
        .position(reply_to("close"))
        .map_or(close.len(), |at| at + 1);
    let deadline = Instant::now() + EXIT_TIMEOUT;
    while running(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }

    let mut held = lifecycle(&close);
    held.retain(|entry| entry.starts_with("reply held"));
    let mut closed = lifecycle(&close[..until_close]);
    // The stopped process's final update and the state that drops it may
    // come before the answer, or not, as the process ends first or not.
    // The turn reports `running` again once its question is withdrawn.
    closed.retain(|entry| {
        !entry.starts_with("reply held") && entry != "idle [0]" && entry != "running [1]"
    });
    // The withdrawal and the turn's end race; both precede the answer.
    closed.sort();
    let updates_after: Vec<&Value> = close[until_close..]
        .iter()
        .chain(&after)
        .filter(|message| message["method"] == "session/update")
        .collect();
    assert_eq!(
        (held, closed, updates_after, running(pid)),
        (
            vec!["reply held -32800".to_owned()],
            vec![
                "idle cancelled [1]".to_owned(),
                "reply close ok".to_owned(),
                format!("withdraw {elicitation}"),
            ],
            Vec::<&Value>::new(),
            false,
        ),
        "close traffic: {close:#?}"
    );
}
