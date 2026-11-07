//! `rho attach` through the real binary, against a daemon hosting a
//! session: it reports how the session's last turn ended, follows a turn
//! under way to its end, and detaches on Ctrl+C while the turn runs on.

use std::process::{ExitStatus, Stdio};

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{JobList, SessionCancel},
    JobListParams, JobState, SessionCancelParams, SessionId,
};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, BufReader, Lines},
    process::{Child, ChildStdout},
};

use super::sessions::{new_session, prompt, settled_job, Sandbox, DEADLINE, REPLY};

/// A `rho attach` process and its JSONL stream.
struct Attach {
    child: Child,
    lines: Lines<BufReader<ChildStdout>>,
    read: Vec<Value>,
}

impl Attach {
    fn spawn(sandbox: &Sandbox, mode: &str, args: &[&str]) -> Self {
        let mut command = tokio::process::Command::from(sandbox.rho(mode));
        command
            .arg("attach")
            .args(args)
            .stdout(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().unwrap();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        Self {
            child,
            lines,
            read: Vec::new(),
        }
    }

    /// Reads lines until one satisfies `done`.
    async fn read_until(&mut self, done: impl Fn(&Value) -> bool) {
        loop {
            let line = tokio::time::timeout(DEADLINE, self.lines.next_line())
                .await
                .expect("rho attach writes its next line")
                .unwrap()
                .expect("rho attach writes until the line it is waited for");
            let line: Value = serde_json::from_str(&line).unwrap();
            let finished = done(&line);
            self.read.push(line);
            if finished {
                return;
            }
        }
    }

    /// Waits for the process to exit, and returns its exit and every line
    /// it wrote.
    async fn finish(mut self) -> (ExitStatus, Vec<Value>) {
        while let Some(line) = tokio::time::timeout(DEADLINE, self.lines.next_line())
            .await
            .expect("rho attach ends its stream")
            .unwrap()
        {
            self.read.push(serde_json::from_str(&line).unwrap());
        }
        let status = tokio::time::timeout(DEADLINE, self.child.wait())
            .await
            .expect("rho attach exits")
            .unwrap();
        (status, self.read)
    }
}

fn is_event(line: &Value) -> bool {
    line["type"] == "event"
}

/// The lines' session events.
fn events(lines: &[Value]) -> Vec<&Value> {
    lines
        .iter()
        .filter(|line| is_event(line))
        .map(|line| &line["event"])
        .collect()
}

/// The first and last lines, with `seq` and the schema version left out.
fn ends(lines: &[Value]) -> (Value, Value) {
    let bare = |line: &Value| {
        let mut line = line.clone();
        let object = line.as_object_mut().unwrap();
        assert_eq!(object.remove("schema_version"), Some(json!(2)));
        object.remove("seq");
        line
    };
    (bare(lines.first().unwrap()), bare(lines.last().unwrap()))
}

async fn job_of(client: &rho_protocol::Client, session: &SessionId) -> rho_protocol::JobSummary {
    client
        .request::<JobList>(&JobListParams {})
        .await
        .unwrap()
        .jobs
        .into_iter()
        .find(|job| job.session.as_ref() == Some(session))
        .unwrap()
}

// Covers: after a turn ended, `rho attach` by job id replays the session
// and reports how the turn ended as `rho run` would, exit 0 with the
// answer; by session id in text mode it shows the conversation; an id the
// daemon does not know fails.
// Owner: rho attach CLI (process contract).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attach_reports_how_the_last_turn_ended() {
    let sandbox = Sandbox::new();
    let (_daemon, client) = sandbox.start_daemon("fixed").await;
    let session = new_session(&client, &sandbox.workspace).await;
    assert_eq!(prompt(&client, &session).await, Ok(()));
    let job = settled_job(&sandbox, &client, &session).await;

    let (status, lines) = Attach::spawn(&sandbox, "fixed", &[job.id.as_str(), "--output", "jsonl"])
        .finish()
        .await;
    let text = sandbox
        .rho("fixed")
        .args(["attach", session.as_str()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    let unknown = sandbox
        .rho("fixed")
        .args(["attach", "nope"])
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    let turn_ended = json!({
        "type": "turn_ended",
        "outcome": {"type": "answered", "text": REPLY, "stop": "end_turn"},
    });
    assert_eq!(
        (
            status.code(),
            ends(&lines),
            events(&lines).iter().rev().nth(1).copied(),
        ),
        (
            Some(0),
            (
                json!({
                    "type": "run_started",
                    "session": session.as_str(),
                    "workspace": sandbox.workspace,
                }),
                json!({"type": "run_ended", "reason": "completed", "exit_code": 0}),
            ),
            Some(&turn_ended),
        ),
        "{lines:#?}"
    );
    assert_eq!(
        (text.status.code(), String::from_utf8(text.stdout).unwrap(),),
        (Some(0), format!("> hello\n{REPLY}\n")),
        "{}",
        String::from_utf8_lossy(&text.stderr)
    );
    assert_eq!(
        (
            unknown.status.code(),
            String::from_utf8(unknown.stderr).unwrap()
        ),
        (
            Some(1),
            "Error: the rho daemon has no job or session nope\n".to_owned()
        )
    );
}

// Covers: `rho attach` follows a turn under way; Ctrl+C detaches with exit
// 130 and the turn runs on; attached again, it ends with the turn when a
// client cancels it, as a cancelled turn (exit 1).
// Owner: rho attach CLI (process contract).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attach_follows_a_turn_and_detaches_on_ctrl_c() {
    let sandbox = Sandbox::new();
    // The fixture holds the turn until it is cancelled.
    let (_daemon, client) = sandbox.start_daemon("delay").await;
    let session = new_session(&client, &sandbox.workspace).await;
    assert_eq!(prompt(&client, &session).await, Ok(()));

    let mut detached = Attach::spawn(&sandbox, "delay", &[session.as_str(), "--output", "jsonl"]);
    // Events flow only once it is attached.
    detached.read_until(is_event).await;
    let pid = detached.child.id().unwrap().to_string();
    let interrupt = std::process::Command::new("kill")
        .args(["-INT", &pid])
        .status()
        .unwrap();
    assert!(interrupt.success());
    let (detached_status, detached_lines) = detached.finish().await;
    let running = job_of(&client, &session).await.state;

    let mut follower = Attach::spawn(&sandbox, "delay", &[session.as_str(), "--output", "jsonl"]);
    follower.read_until(is_event).await;
    client
        .request::<SessionCancel>(&SessionCancelParams {
            session: session.clone(),
        })
        .await
        .unwrap();
    let (status, lines) = follower.finish().await;

    assert_eq!(
        (
            detached_status.code(),
            ends(&detached_lines).1,
            running,
            status.code(),
            ends(&lines).1,
            events(&lines).last().copied(),
        ),
        (
            Some(130),
            json!({
                "type": "run_ended",
                "reason": "detached",
                "exit_code": 130,
                "message": "detached on SIGINT; the turn runs on in the daemon",
            }),
            JobState::Running,
            Some(1),
            json!({
                "type": "run_ended",
                "reason": "cancelled",
                "exit_code": 1,
                "message": "the turn was cancelled",
            }),
            Some(&json!({"type": "turn_ended", "outcome": {"type": "cancelled"}})),
        ),
        "log:\n{}",
        sandbox.daemon_log()
    );
}
