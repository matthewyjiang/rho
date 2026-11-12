//! `rho attach` through the real binary, against a daemon hosting a
//! session: it reports how the session's last turn ended, follows a turn
//! under way to its end, detaches on Ctrl+C while the turn runs on, and
//! answers and cancels from lines typed on stdin.

use std::process::{ExitStatus, Stdio};

use pretty_assertions::assert_eq;
use rho_protocol::{methods::JobList, JobListParams, JobState, SessionEvent, SessionId};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStderr, ChildStdin, ChildStdout},
};

use super::sessions::{
    answer, blue, first_request, prompt_a_supervised_write, settled_job, Follower, Sandbox,
    DEADLINE, REPLY,
};

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
            .stdin(Stdio::null())
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

/// The job of `session`'s turns.
async fn job_of(client: &rho_protocol::Client, session: &SessionId) -> rho_protocol::JobSummary {
    client
        .request::<rho_protocol::methods::JobList>(&rho_protocol::JobListParams {})
        .await
        .unwrap()
        .jobs
        .into_iter()
        .find(|job| job.session.as_ref() == Some(session))
        .unwrap()
}

// Covers: after a turn ended, `rho attach` by job id replays the session
// and reports how the turn ended as `rho run` would, exit 0 with the
// answer, and so it does again through the next daemon, which reopens the
// session (#1661); by session id in text mode it shows the conversation; an
// id the daemon does not know fails.
// Owner: rho attach CLI (process contract).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attach_reports_how_the_last_turn_ended() {
    let sandbox = Sandbox::new();
    let (mut daemon, client) = sandbox.spawn_daemon(sandbox.rho("fixed"), &[]).await;
    let session = sandbox.new_session(&client).await;
    assert_eq!(sandbox.prompt(&client, &session).await, Ok(()));
    let job = settled_job(&sandbox, &client, &session).await;

    let (status, lines) = Attach::spawn(&sandbox, "fixed", &[job.id.as_str(), "--output", "jsonl"])
        .finish()
        .await;
    drop(client);
    let stop = sandbox
        .rho("fixed")
        .args(["daemon", "stop"])
        .status()
        .unwrap();
    assert!(stop.success(), "log:\n{}", sandbox.isolated.daemon_log());
    daemon.wait().unwrap();
    let _client = sandbox.start_daemon("fixed").await;
    let (reopened_status, reopened_lines) =
        Attach::spawn(&sandbox, "fixed", &[session.as_str(), "--output", "jsonl"])
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

    // Both attaches, the second to the reopened session, report the turn's
    // end under the id the turn ran with.
    let turn = events(&lines)
        .iter()
        .rev()
        .nth(1)
        .map(|ended| &ended["turn"]);
    assert!(
        turn.is_some_and(Value::is_string),
        "the turn's end names its turn: {lines:#?}"
    );
    let turn_ended = json!({
        "type": "turn_ended",
        "turn": turn,
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
        (
            reopened_status.code(),
            ends(&reopened_lines).1,
            events(&reopened_lines).iter().rev().nth(1).copied(),
        ),
        (
            Some(0),
            json!({"type": "run_ended", "reason": "completed", "exit_code": 0}),
            Some(&turn_ended),
        ),
        "{reopened_lines:#?}\nlog:\n{}",
        sandbox.isolated.daemon_log()
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
            "Error: could not attach to session nope: rho daemon rejected rho/session/attach: no session is saved under nope (code -32003)\n".to_owned()
        )
    );
}

// Covers: `rho attach` follows a turn under way; Ctrl+C detaches with exit
// 130 and the turn runs on; attached again, it ends with the turn when a
// client cancels it, as a cancelled turn (exit 1).
// Owner: rho attach CLI (process contract).
// Unix: the test interrupts `rho attach` with SIGINT, which nothing sends a
// process without a console on Windows.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attach_follows_a_turn_and_detaches_on_ctrl_c() {
    use rho_protocol::{methods::SessionCancel, SessionCancelParams};

    let sandbox = Sandbox::new();
    // The fixture holds the turn until it is cancelled.
    let client = sandbox.start_daemon("delay").await;
    let session = sandbox.new_session(&client).await;
    // The end the attachment reports is this turn's, by its id.
    let started = sandbox
        .prompt_keyed(&client, &session, "hello", None)
        .await
        .unwrap();

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
            Some(&json!({
                "type": "turn_ended",
                "turn": started.as_str(),
                "outcome": {"type": "cancelled"},
            })),
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

/// A text-mode `rho attach` process typed at through its stdin.
struct Typed {
    child: Child,
    stdin: ChildStdin,
    stderr: Lines<BufReader<ChildStderr>>,
    told: Vec<String>,
}

impl Typed {
    fn spawn(sandbox: &Sandbox, mode: &str, session: &SessionId) -> Self {
        let mut command = tokio::process::Command::from(sandbox.rho(mode));
        command
            .args(["attach", session.as_str(), "--input", "stdin"])
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let stderr = BufReader::new(child.stderr.take().unwrap()).lines();
        Self {
            child,
            stdin,
            stderr,
            told: Vec::new(),
        }
    }

    /// Reads stderr until a line is `line`.
    async fn told(&mut self, line: &str) {
        loop {
            let next = tokio::time::timeout(DEADLINE, self.stderr.next_line())
                .await
                .expect("rho attach tells its next line")
                .unwrap()
                .unwrap_or_else(|| {
                    panic!("rho attach ended before telling {line:?}: {:#?}", self.told)
                });
            let found = next == line;
            self.told.push(next);
            if found {
                return;
            }
        }
    }

    async fn type_line(&mut self, line: &str) {
        self.stdin
            .write_all(format!("{line}\n").as_bytes())
            .await
            .unwrap();
    }

    async fn finish(mut self) -> (Option<i32>, Vec<String>) {
        while let Some(line) = tokio::time::timeout(DEADLINE, self.stderr.next_line())
            .await
            .expect("rho attach ends")
            .unwrap()
        {
            self.told.push(line);
        }
        let status = tokio::time::timeout(DEADLINE, self.child.wait())
            .await
            .expect("rho attach exits")
            .unwrap();
        (status.code(), self.told)
    }
}

// Covers: in text mode with `--input stdin`, `rho attach` tells a
// supervised turn's approval with its choices and answers it from a line
// written to its stdin, and the turn carries on to its end.
// Owner: rho attach CLI (process contract).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attach_answers_an_approval_written_to_stdin() {
    let sandbox = Sandbox::new();
    let (client, session) = prompt_a_supervised_write(&sandbox).await;
    let mut approver = Typed::spawn(&sandbox, "tool-call", &session);
    approver
        .told("  1 allow once · 2 allow for session · 3 deny")
        .await;
    approver.type_line("1").await;
    let (status, told) = approver.finish().await;
    let written = std::fs::read_to_string(sandbox.workspace.join("note.txt")).ok();
    let job = settled_job(&sandbox, &client, &session).await;

    assert_eq!(
        (status, written, job.state),
        (Some(0), Some("approved".to_owned()), JobState::Completed),
        "{told:#?}"
    );
}

// Covers: a /cancel line written to `rho attach`'s stdin cancels the turn
// under way, which ends the attachment as a cancelled turn.
// Owner: rho attach CLI (process contract).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attach_cancels_the_turn_on_a_cancel_line() {
    let sandbox = Sandbox::new();
    let client = sandbox.start_daemon("delay").await;
    let session = sandbox.new_session(&client).await;
    assert_eq!(sandbox.prompt(&client, &session).await, Ok(()));
    let mut canceller = Typed::spawn(&sandbox, "delay", &session);
    // Lines are read once it is attached; one written earlier waits for it.
    canceller.type_line("/cancel").await;
    let (status, told) = canceller.finish().await;

    assert_eq!(
        (status, told.last().cloned()),
        (Some(1), Some("Error: the turn was cancelled".to_owned())),
        "{told:#?}\nlog:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: `rho attach` follows a subagent run by its job's id: it shows the
// run's events while the run is under way, here the question the run asks
// its parent, and ends with the run, exit 0, once the parent's client
// answered; attached again after the run ended, it replays the session the
// run saved and exits 0 (#1668).
// Owner: rho attach CLI (process contract).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attach_follows_a_subagent_run() {
    let sandbox = Sandbox::new();
    // The fixture makes the call the prompt spells out as JSON.
    let client = sandbox.start_daemon("tool-call").await;
    let session = sandbox.new_session(&client).await;
    let (mut parent, _) = Follower::attach(&sandbox, &session, Vec::new(), None).await;
    // The run's prompt is a call too: it asks its parent and waits.
    let ask = json!({
        "id": "ask",
        "name": "questionnaire",
        "arguments": {
            "questions": [{
                "id": "color",
                "question": "Which color?",
                "choices": ["Red", "Blue"],
            }],
        },
    });
    let delegate = json!({
        "id": "delegate",
        "name": "agent",
        "arguments": {"agent_id": "default", "prompt": ask.to_string()},
    });
    sandbox.prompt_tool_call(&client, &session, &delegate).await;
    parent
        .read_until(|event| matches!(event, SessionEvent::InputRequested { .. }))
        .await;
    let request = first_request(&parent.events).id;
    let parent_job = job_of(&client, &session).await.id;
    let run = client
        .request::<JobList>(&JobListParams {})
        .await
        .unwrap()
        .jobs
        .into_iter()
        .find(|job| job.parent.as_ref() == Some(&parent_job))
        .expect("the run's job");
    let run_session = run
        .session
        .clone()
        .expect("a Rho subagent writes a session");

    let mut live = Attach::spawn(
        &sandbox,
        "tool-call",
        &[run.id.as_str(), "--output", "jsonl"],
    );
    live.read_until(|line| line["event"]["type"] == "tool_call")
        .await;
    answer(&client, &session, request, blue()).await.unwrap();
    let (status, lines) = live.finish().await;
    let (after_status, after_lines) = Attach::spawn(
        &sandbox,
        "tool-call",
        &[run.id.as_str(), "--output", "jsonl"],
    )
    .finish()
    .await;

    let run_started = json!({
        "type": "run_started",
        "session": run_session.as_str(),
        "workspace": sandbox.resolved_workspace(),
    });
    let completed = json!({"type": "run_ended", "reason": "completed", "exit_code": 0});
    let ended: Vec<_> = events(&lines)
        .into_iter()
        .filter(|event| event["type"] == "turn_ended")
        .map(|event| event["outcome"]["type"].clone())
        .collect();
    assert_eq!(
        (status.code(), ends(&lines), ended),
        (
            Some(0),
            (run_started, completed.clone()),
            vec![json!("answered")],
        ),
        "{lines:#?}\nlog:\n{}",
        sandbox.isolated.daemon_log()
    );
    // The replay names the workspace as the saved session does, which is
    // not this test's to pin.
    let (after_started, after_ended) = ends(&after_lines);
    assert_eq!(
        (
            after_status.code(),
            after_started["type"].clone(),
            after_started["session"].clone(),
            after_ended,
        ),
        (
            Some(0),
            json!("run_started"),
            json!(run_session.as_str()),
            completed,
        ),
        "{after_lines:#?}\nlog:\n{}",
        sandbox.isolated.daemon_log()
    );
}
