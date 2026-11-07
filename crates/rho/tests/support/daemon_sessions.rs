//! Sessions the daemon hosts, through the real binary and its socket: a
//! turn outlives the client that started it, every way a turn can end early
//! is recorded on its job, and clients that attach, leave, and come back see
//! the same events.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{
        JobList, SessionAnswer, SessionAttach, SessionCancel, SessionDetach, SessionHold,
        SessionNew, SessionPrompt,
    },
    AnswerStop, ApprovalAsk, ApprovalSource, Ask, Client, ClientError, ClientInfo, Cursor,
    DaemonNotification, DetachReason, ErrorCode, InputAnswer, InputRequest, InputRequestId,
    JobKind, JobListParams, JobState, JobSummary, Notifications, Operation, PathScope, Replay,
    Resolution, Seq, SessionAnswerParams, SessionAttachParams, SessionAttachResponse,
    SessionCancelParams, SessionDetachParams, SessionDetachedParams, SessionEvent,
    SessionHoldParams, SessionId, SessionNewParams, SessionPromptParams, StreamId, ToolCallId,
    ToolContent, ToolStatus, TurnError, TurnErrorKind, TurnOutcome,
};
use tokio::net::UnixStream;

use super::harness::{DaemonProcess, DaemonSandbox};

/// Failure bound for each wait on the daemon. Every turn here answers from
/// the fixture provider without network; this only stops a hung test.
const DEADLINE: Duration = Duration::from_secs(60);

/// The reply the fixture provider gives, which the test finds in the
/// session file.
const REPLY: &str = "reply saved by the daemon session";

const CONFIG: &str = r#"provider = "xai"
model = "grok-fixture"
auth = "xai-oauth"

[web_search]
hosted = false
provider = "disabled"
"#;

/// A one-node workflow whose agent replies from the fixture provider.
const WORKFLOW: &str = r#"def build(inputs):
    return workflow(name = "daemon-run", nodes = [
        agent(
            name = "inspect",
            agent = "fixture",
            prompt = "inspect the workspace",
            access = "mutating",
            timeout_seconds = 600,
            max_output_bytes = 100000,
        ),
    ])

WORKFLOW = define(inputs = {}, build = build)
"#;

const WORKFLOW_AGENT: &str = "---
description: Fixture agent for a daemon workflow run
---

Do the task.
";

struct Sandbox {
    daemon: DaemonSandbox,
    /// The session's workspace, canonicalized as the daemon records it.
    workspace: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let daemon = DaemonSandbox::new();
        std::fs::write(daemon.home().join("config.toml"), CONFIG).unwrap();
        let workspace = daemon.root().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let workspace = workspace.canonicalize().unwrap();
        Self { daemon, workspace }
    }

    /// `rho` in the workspace with its credential store inside the sandbox
    /// and the fixture provider in `mode`.
    fn rho(&self, mode: &str) -> Command {
        let mut command = self.daemon.rho();
        command
            .current_dir(&self.workspace)
            // Fixture sessions must not query the developer's OS keyring.
            .env("RHO_CREDENTIAL_STORE", "file")
            .env("RHO_AUTOMATION_TEST_MODE", mode)
            .env("RHO_AUTOMATION_TEST_RESPONSE", REPLY)
            .env_remove("RHO_AUTOMATION_TEST_PATH")
            .env_remove("HERDR_ENV")
            .env_remove("HERDR_SOCKET_PATH")
            .env_remove("HERDR_PANE_ID")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    /// Starts `rho daemon run` with the fixture in `mode` and connects to
    /// it once it answers.
    async fn start_daemon(&self, mode: &str) -> (DaemonProcess, Client) {
        // Outside every session's workspace, as a service manager starts it,
        // so nothing a session runs can lean on the daemon's own directory.
        let daemon = DaemonProcess(
            self.rho(mode)
                .current_dir(self.daemon.home())
                .args(["daemon", "run"])
                .spawn()
                .unwrap(),
        );
        // A daemon killed earlier left its socket file behind, so wait for
        // one that answers rather than one that exists.
        let client = self
            .daemon
            .poll_until("the daemon to answer", DEADLINE, async || {
                self.try_connect().await
            })
            .await
            .0;
        (daemon, client)
    }

    async fn connect(&self) -> Client {
        self.connect_following().await.0
    }

    /// A connection and the session events the daemon sends on it.
    async fn connect_following(&self) -> (Client, Notifications) {
        self.try_connect().await.expect("the daemon answers")
    }

    async fn try_connect(&self) -> Option<(Client, Notifications)> {
        let stream = UnixStream::connect(self.daemon.socket()).await.ok()?;
        let client = ClientInfo {
            name: "daemon-sessions-test".into(),
            version: "0".into(),
        };
        Client::handshake(stream, client)
            .await
            .ok()
            .map(|(client, notifications, _)| (client, notifications))
    }

    fn daemon_log(&self) -> String {
        self.daemon.daemon_log()
    }

    /// The records saved in `session`'s file.
    fn session_records(&self, session: &SessionId) -> Vec<serde_json::Value> {
        // Each session is a directory named for its id, below its
        // workspace's.
        let suffix = format!("_{session}");
        let session = std::fs::read_dir(self.daemon.home().join("sessions"))
            .unwrap()
            .flatten()
            .filter_map(|workspace| std::fs::read_dir(workspace.path()).ok())
            .flatten()
            .flatten()
            .find(|entry| entry.file_name().to_string_lossy().ends_with(&suffix))
            .unwrap_or_else(|| panic!("no session directory for {session}"))
            .path();
        std::fs::read_to_string(session.join("session.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

async fn new_session(client: &Client, workspace: &Path) -> SessionId {
    client
        .request::<SessionNew>(&SessionNewParams {
            cwd: workspace.to_path_buf(),
        })
        .await
        .unwrap()
        .session
}

async fn prompt(client: &Client, session: &SessionId) -> Result<(), Option<ErrorCode>> {
    client
        .request::<SessionPrompt>(&SessionPromptParams {
            session: session.clone(),
            text: "hello".into(),
        })
        .await
        .map(|_| ())
        .map_err(|error| match error {
            ClientError::Rpc { error, .. } => error.error_code(),
            error => panic!("prompt failed outside the protocol: {error}"),
        })
}

/// Polls `rho/job/list` until the job of `session`'s turns is in a state
/// other than running or waiting for input, and returns the job then.
async fn settled_job(sandbox: &Sandbox, client: &Client, session: &SessionId) -> JobSummary {
    settled(sandbox, client, |job| job.session.as_ref() == Some(session)).await
}

/// Polls `rho/job/list` until the job `pick` picks is in a state other than
/// queued, running, or waiting for input, and returns the job then.
async fn settled(
    sandbox: &Sandbox,
    client: &Client,
    pick: impl Fn(&JobSummary) -> bool,
) -> JobSummary {
    sandbox
        .daemon
        .poll_until("the job to end", DEADLINE, async || {
            let jobs = client.request::<JobList>(&JobListParams {}).await.unwrap();
            let job = jobs
                .jobs
                .into_iter()
                .find(&pick)
                .expect("the daemon lists the job");
            match job.state {
                JobState::Queued | JobState::Running | JobState::WaitingForInput => None,
                JobState::Idle
                | JobState::Completed
                | JobState::Failed
                | JobState::Cancelled
                | JobState::Interrupted => Some(job),
                state => panic!("the daemon sent a job state this test does not know: {state:?}"),
            }
        })
        .await
}

// Covers: a prompt returns once its turn saved the prompt, the turn runs on in the
// daemon after the client that sent it disconnects, a new client sees its
// job completed with the session's workspace and id, and the session file
// holds the model's reply. A session id no session has is not found.
// Owner: daemon session hosting.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_turn_outlives_the_client_that_prompted_it() {
    let sandbox = Sandbox::new();
    let (_daemon, client) = sandbox.start_daemon("fixed").await;
    let session = new_session(&client, &sandbox.workspace).await;
    assert_eq!(prompt(&client, &session).await, Ok(()));
    drop(client);

    let client = sandbox.connect().await;
    let job = settled_job(&sandbox, &client, &session).await;
    let unknown = prompt(&client, &SessionId::new("no-such-session")).await;

    assert_eq!(
        (
            job.state,
            job.kind,
            job.workspace,
            job.parent,
            job.session,
            job.error,
            unknown
        ),
        (
            JobState::Completed,
            JobKind::Session,
            sandbox.workspace.clone(),
            None,
            Some(session.clone()),
            None,
            Err(Some(ErrorCode::SessionNotLoaded))
        ),
        "log:\n{}",
        sandbox.daemon_log()
    );
    let records = sandbox.session_records(&session);
    assert!(
        records
            .iter()
            .any(|record| record.to_string().contains(REPLY)),
        "the session saved the reply: {records:#?}"
    );
}

// Covers: a turn's end reaches attached clients as how it ended, before
// the job state that records it: the model's answer whole, or why it
// failed, with an authentication failure naming only the provider, auth,
// and how to log in, never the provider's text (#1669).
// Owner: daemon session event feed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_turn_tells_clients_how_it_ended() {
    let cases = [
        (
            "fixed",
            TurnOutcome::Answered {
                text: REPLY.into(),
                stop: AnswerStop::EndTurn,
            },
            JobState::Completed,
        ),
        (
            "fail",
            TurnOutcome::Failed {
                error: TurnError {
                    kind: TurnErrorKind::Provider,
                    message: "provider failed: deterministic provider failure".into(),
                },
            },
            JobState::Failed,
        ),
        (
            "auth-failure",
            TurnOutcome::Failed {
                error: TurnError {
                    kind: TurnErrorKind::Authentication,
                    message: "authentication failed for xai (auth: xai-oauth); run /login xai or pass --auth xai-api-key".into(),
                },
            },
            JobState::Failed,
        ),
    ];
    for (mode, outcome, state) in cases {
        let sandbox = Sandbox::new();
        let (_daemon, client) = sandbox.start_daemon(mode).await;
        let session = new_session(&client, &sandbox.workspace).await;
        let (mut watcher, _) = Follower::attach(&sandbox, &session, Vec::new(), None).await;
        // A turn that fails before it saves the prompt answers the prompt
        // with the failure; the clients learn it all the same.
        let _ = prompt(&client, &session).await;
        watcher.read_until(is_state(state)).await;

        let ending: Vec<SessionEvent> = watcher
            .events
            .iter()
            .rev()
            .take(2)
            .rev()
            .map(|(_, event)| event.clone())
            .collect();
        assert_eq!(
            ending,
            vec![
                SessionEvent::TurnEnded { outcome },
                SessionEvent::State { state }
            ],
            "{mode}: log:\n{}",
            sandbox.daemon_log()
        );
    }
}

/// How a test ends a turn the fixture holds open.
#[derive(Clone, Copy, Debug)]
enum Ending {
    /// `rho/session/cancel`.
    Cancel,
    /// `rho daemon stop`, then a new daemon.
    Stop,
    /// The daemon is killed, then a new daemon starts.
    Kill,
}

// Covers: a subagent a daemon session starts runs in the daemon and is
// listed as a child job of the session's job, in the session's workspace,
// completed once it finished, with the session it saved its reply in.
// Owner: daemon subagent jobs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_subagent_is_a_child_job_of_its_session() {
    let sandbox = Sandbox::new();
    // The fixture makes the call the prompt spells out as JSON.
    let (_daemon, client) = sandbox.start_daemon("tool-call").await;
    let session = new_session(&client, &sandbox.workspace).await;
    let call = serde_json::json!({
        "id": "delegate",
        "name": "agent",
        "arguments": {"agent_id": "explorer", "prompt": "Look around."},
    });
    client
        .request::<SessionPrompt>(&SessionPromptParams {
            session: session.clone(),
            text: call.to_string(),
        })
        .await
        .unwrap();

    let parent = settled_job(&sandbox, &client, &session).await;
    let child = settled(&sandbox, &client, |job| {
        job.parent.as_ref() == Some(&parent.id)
    })
    .await;
    assert_eq!(
        (
            parent.state,
            child.kind,
            child.state,
            child.workspace,
            child.error,
            child.queue,
        ),
        (
            JobState::Completed,
            JobKind::Subagent,
            JobState::Completed,
            sandbox.workspace.clone(),
            None,
            None,
        ),
        "log:\n{}",
        sandbox.daemon_log()
    );
    let child_session = child.session.expect("a Rho subagent writes a session");
    let records = sandbox.session_records(&child_session);
    assert!(
        records
            .iter()
            .any(|record| record.to_string().contains(REPLY)),
        "the subagent saved its reply: {records:#?}"
    );
}

// Covers: a workflow run a daemon session starts is driven in the daemon in
// the session's workspace, not the daemon's directory, and is listed as a
// child job of the session's job, completed once the run succeeded; each of
// its agent nodes is a subagent job whose parent is the workflow job.
// Owner: daemon workflow jobs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_workflow_run_is_a_child_job_of_its_session() {
    let sandbox = Sandbox::new();
    let workflows = sandbox.workspace.join("wf");
    std::fs::create_dir_all(workflows.join("agents")).unwrap();
    std::fs::write(workflows.join("main.star"), WORKFLOW).unwrap();
    std::fs::write(workflows.join("agents/fixture.md"), WORKFLOW_AGENT).unwrap();
    let (_daemon, client) = sandbox.start_daemon("tool-call").await;
    let session = new_session(&client, &sandbox.workspace).await;
    let call = serde_json::json!({
        "id": "start",
        "name": "workflow",
        "arguments": {"action": "start", "file": "wf/main.star"},
    });
    client
        .request::<SessionPrompt>(&SessionPromptParams {
            session: session.clone(),
            text: call.to_string(),
        })
        .await
        .unwrap();

    let parent = settled_job(&sandbox, &client, &session).await;
    let workflow = settled(&sandbox, &client, |job| {
        job.parent.as_ref() == Some(&parent.id)
    })
    .await;
    let node = settled(&sandbox, &client, |job| {
        job.parent.as_ref() == Some(&workflow.id)
    })
    .await;
    let log = sandbox.daemon_log();
    assert_eq!(
        (
            (parent.state, workflow.kind, workflow.state, workflow.error),
            (node.kind, node.state, node.error),
            (workflow.workspace, node.workspace),
        ),
        (
            (
                JobState::Completed,
                JobKind::Workflow,
                JobState::Completed,
                None
            ),
            (JobKind::Subagent, JobState::Completed, None),
            (sandbox.workspace.clone(), sandbox.workspace.clone()),
        ),
        "log:\n{log}"
    );
    // The run the tool started is the only one under the daemon's home.
    let runs: Vec<String> = std::fs::read_dir(sandbox.daemon.home().join("workflows/runs"))
        .unwrap()
        .map(|run| run.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(workflow.workflow_run.map(|run| vec![run]), Some(runs));
}

// Covers: a turn cancelled through `rho/session/cancel` is recorded
// cancelled and its session takes the next prompt; a turn the daemon's stop
// interrupts is recorded interrupted, within `rho daemon stop`'s wait; a
// turn whose daemon died is recorded interrupted by the next daemon. A
// session an earlier daemon saved is listed with its job but refuses
// prompts as not loaded.
// Owner: daemon session hosting and job records.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_turn_ended_early_is_recorded_on_its_job() {
    let cases = [
        (Ending::Cancel, JobState::Cancelled, Ok(())),
        (
            Ending::Stop,
            JobState::Interrupted,
            Err(Some(ErrorCode::SessionNotLoaded)),
        ),
        (
            Ending::Kill,
            JobState::Interrupted,
            Err(Some(ErrorCode::SessionNotLoaded)),
        ),
    ];
    for (ending, expected_state, expected_next_prompt) in cases {
        let sandbox = Sandbox::new();
        // The fixture holds each turn until it is cancelled.
        let (mut daemon, client) = sandbox.start_daemon("delay").await;
        let session = new_session(&client, &sandbox.workspace).await;
        assert_eq!(prompt(&client, &session).await, Ok(()), "{ending:?}");

        let (_daemon, client) = match ending {
            Ending::Cancel => {
                client
                    .request::<SessionCancel>(&SessionCancelParams {
                        session: session.clone(),
                    })
                    .await
                    .unwrap();
                (daemon, client)
            }
            Ending::Stop => {
                drop(client);
                let stop = sandbox
                    .rho("delay")
                    .args(["daemon", "stop"])
                    .status()
                    .unwrap();
                assert!(stop.success(), "{ending:?}: log:\n{}", sandbox.daemon_log());
                drop(daemon);
                sandbox.start_daemon("delay").await
            }
            Ending::Kill => {
                drop(client);
                daemon.kill();
                sandbox.start_daemon("delay").await
            }
        };
        let state = settled_job(&sandbox, &client, &session).await.state;
        let next_prompt = prompt(&client, &session).await;

        assert_eq!(
            (state, next_prompt),
            (expected_state, expected_next_prompt),
            "{ending:?}: log:\n{}",
            sandbox.daemon_log()
        );
    }
}

/// A client following one session's events on its own connection, as a
/// frontend does: it keeps the events it has, by seq, and skips any it
/// already has, so a resumed stream continues its view without repeats.
struct Follower {
    client: Client,
    notifications: Notifications,
    session: SessionId,
    stream: StreamId,
    events: Vec<(Seq, SessionEvent)>,
}

impl Follower {
    /// Connects and attaches to `session`, resuming after `after` when
    /// given; returns the follower and how the daemon answered the attach.
    async fn attach(
        sandbox: &Sandbox,
        session: &SessionId,
        events: Vec<(Seq, SessionEvent)>,
        after: Option<Cursor>,
    ) -> (Self, SessionAttachResponse) {
        let (client, notifications) = sandbox.connect_following().await;
        let attached = client
            .request::<SessionAttach>(&SessionAttachParams {
                session: session.clone(),
                after,
            })
            .await
            .unwrap();
        let follower = Self {
            client,
            notifications,
            session: session.clone(),
            stream: attached.stream.clone(),
            events,
        };
        (follower, attached)
    }

    /// Where the follower is: the last event it has.
    fn cursor(&self) -> Cursor {
        let (seq, _) = self.events.last().expect("the follower has an event");
        Cursor {
            stream: self.stream.clone(),
            seq: *seq,
        }
    }

    /// The next notification, or `None` once the daemon closed the
    /// connection.
    async fn next_notification(&mut self) -> Option<DaemonNotification> {
        tokio::time::timeout(DEADLINE, self.notifications.recv())
            .await
            .expect("the daemon sends a notification or closes")
    }

    /// Reads events until one satisfies `done`.
    async fn read_until(&mut self, done: impl Fn(&SessionEvent) -> bool) {
        loop {
            let notification = tokio::time::timeout(DEADLINE, self.notifications.recv())
                .await
                .expect("the daemon sends the session's next event")
                .expect("the connection stays open");
            let DaemonNotification::SessionEvent(params) = notification else {
                panic!("expected a session event, got {notification:?}");
            };
            assert_eq!(params.session, self.session);
            let seen = self.events.last().map(|(seq, _)| *seq);
            // A reset's transcript is at seq 0 but its last event, so a cold
            // follower takes equal seqs as they come.
            if seen.is_some_and(|seen| params.seq < seen) {
                continue;
            }
            let finished = done(&params.event);
            self.events.push((params.seq, params.event));
            if finished {
                return;
            }
        }
    }

    /// Detaches from the session, then leaves, as a closing frontend does.
    async fn leave(self) -> Vec<(Seq, SessionEvent)> {
        self.client
            .request::<SessionDetach>(&SessionDetachParams {
                session: self.session.clone(),
            })
            .await
            .unwrap();
        self.events
    }
}

fn is_state(state: JobState) -> impl Fn(&SessionEvent) -> bool {
    move |event| matches!(event, SessionEvent::State { state: got, .. } if *got == state)
}

/// What a conversation shows: its events without job states and context
/// usage, which describe the session as it was when they were sent.
fn conversation(events: &[(Seq, SessionEvent)]) -> Vec<SessionEvent> {
    events
        .iter()
        .map(|(_, event)| event.clone())
        .filter(|event| {
            !matches!(
                event,
                SessionEvent::State { .. } | SessionEvent::Usage { .. }
            )
        })
        .collect()
}

// Covers: two clients follow one turn; one detaches and leaves mid-turn,
// the turn goes on, and it comes back with its cursor to the events it
// missed and no repeats, so both hold the same events under the same seqs.
// A client that first attaches after the turn saved resets to the same
// conversation, replayed from the saved session, under the same message
// ids. When the daemon stops, an attached client is told why it was
// detached (#1657).
// Owner: daemon session event feed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clients_that_leave_and_come_back_see_the_same_turn() {
    let sandbox = Sandbox::new();
    // The fixture holds the turn until it is cancelled.
    let (_daemon, client) = sandbox.start_daemon("delay").await;
    let session = new_session(&client, &sandbox.workspace).await;
    let (mut watcher, watched) = Follower::attach(&sandbox, &session, Vec::new(), None).await;
    let (mut leaver, ..) = Follower::attach(&sandbox, &session, Vec::new(), None).await;

    assert_eq!(prompt(&client, &session).await, Ok(()));
    let saved_prompt = |event: &SessionEvent| matches!(event, SessionEvent::UserMessage { .. });
    leaver.read_until(saved_prompt).await;
    let cursor = leaver.cursor();
    let kept = leaver.leave().await;
    client
        .request::<SessionCancel>(&SessionCancelParams {
            session: session.clone(),
        })
        .await
        .unwrap();
    watcher.read_until(is_state(JobState::Cancelled)).await;

    let (mut leaver, resumed) = Follower::attach(&sandbox, &session, kept, Some(cursor)).await;
    leaver.read_until(is_state(JobState::Cancelled)).await;
    let (mut latecomer, late) = Follower::attach(&sandbox, &session, Vec::new(), None).await;
    latecomer.read_until(is_state(JobState::Cancelled)).await;

    let seqs: Vec<u64> = watcher.events.iter().map(|(seq, _)| seq.0).collect();
    assert_eq!(
        seqs,
        (1..=seqs.len() as u64).collect::<Vec<_>>(),
        "the stream numbers every event once, in order"
    );
    assert_eq!(
        (
            (watched.replay, watched.state),
            (resumed.replay, resumed.state),
            late.replay
        ),
        (
            (Replay::Reset, JobState::Idle),
            (Replay::Tail, JobState::Cancelled),
            Replay::Reset
        )
    );
    assert_eq!(
        leaver.events,
        watcher.events,
        "log:\n{}",
        sandbox.daemon_log()
    );
    assert_eq!(
        conversation(&latecomer.events),
        conversation(&watcher.events)
    );
    assert!(
        conversation(&watcher.events)
            .iter()
            .any(|event| matches!(event, SessionEvent::UserMessage { .. })),
        "the turn showed its prompt: {:#?}",
        watcher.events
    );
    assert_eq!(
        conversation(&watcher.events).last(),
        Some(&SessionEvent::TurnEnded {
            outcome: TurnOutcome::Cancelled
        }),
        "the turn ended cancelled"
    );

    let stop = sandbox
        .rho("delay")
        .args(["daemon", "stop"])
        .status()
        .unwrap();
    assert!(stop.success(), "log:\n{}", sandbox.daemon_log());
    assert_eq!(
        watcher.next_notification().await,
        Some(DaemonNotification::SessionDetached(SessionDetachedParams {
            session: session.clone(),
            reason: DetachReason::DaemonStopping,
        })),
        "log:\n{}",
        sandbox.daemon_log()
    );
}

/// Answers `request` of `session` with `answer` on `client`.
async fn answer(
    client: &Client,
    session: &SessionId,
    request: InputRequestId,
    answer: InputAnswer,
) -> Result<(), Option<ErrorCode>> {
    client
        .request::<SessionAnswer>(&SessionAnswerParams {
            session: session.clone(),
            request,
            answer,
        })
        .await
        .map(|_| ())
        .map_err(|error| match error {
            ClientError::Rpc { error, .. } => error.error_code(),
            error => panic!("answer failed outside the protocol: {error}"),
        })
}

/// Starts a supervised daemon and prompts a new session's turn to `write`
/// `note.txt`, which waits for approval with nobody attached.
async fn prompt_a_supervised_write(sandbox: &Sandbox) -> (DaemonProcess, Client, SessionId) {
    std::fs::write(
        sandbox.daemon.home().join("config.toml"),
        format!("permission_mode = \"supervised\"\n{CONFIG}"),
    )
    .unwrap();
    // The fixture makes the call the prompt spells out as JSON.
    let (daemon, client) = sandbox.start_daemon("tool-call").await;
    let session = new_session(&client, &sandbox.workspace).await;
    let call = serde_json::json!({
        "id": "edit",
        "name": "write",
        "arguments": {"path": "note.txt", "content": "approved"},
    });
    client
        .request::<SessionPrompt>(&SessionPromptParams {
            session: session.clone(),
            text: call.to_string(),
        })
        .await
        .unwrap();
    (daemon, client, session)
}

// Covers: a supervised turn's approval, asked while no client is attached,
// waits in the daemon with the job waiting for input; a client that
// attaches later is told the open request in its attach response and
// answers it, every attached client learns it was answered, a second answer
// finds it no longer open, and the turn goes on with the answer (#1660).
// Owner: daemon input requests.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_approval_waits_for_any_client_to_answer() {
    let sandbox = Sandbox::new();
    let (_daemon, client, session) = prompt_a_supervised_write(&sandbox).await;
    // Nobody is attached while the turn asks.
    sandbox
        .daemon
        .poll_until("the turn to ask", DEADLINE, async || {
            let jobs = client.request::<JobList>(&JobListParams {}).await.unwrap();
            jobs.jobs.into_iter().find(|job| {
                job.session.as_ref() == Some(&session) && job.state == JobState::WaitingForInput
            })
        })
        .await;

    let (mut answerer, attached) = Follower::attach(&sandbox, &session, Vec::new(), None).await;
    let request = attached
        .inputs
        .first()
        .expect("the request is open")
        .clone();
    let first = answer(
        &answerer.client,
        &session,
        request.id,
        InputAnswer::AllowOnce,
    )
    .await;
    answerer
        .read_until(|event| matches!(event, SessionEvent::InputResolved { .. }))
        .await;
    let second = answer(
        &client,
        &session,
        request.id,
        InputAnswer::Deny { reason: None },
    )
    .await;
    let job = settled_job(&sandbox, &client, &session).await;

    // The policy words its reason; the rest is what the call asked for.
    let reason = match &request.ask {
        Ask::Approval(approval) => approval.reason.clone(),
        ask => panic!("a write asks for approval: {ask:?}"),
    };
    assert_eq!(
        (
            attached.inputs.clone(),
            attached.state,
            first,
            answerer.events.last().map(|(_, event)| event.clone()),
            second,
            job.state,
            std::fs::read_to_string(sandbox.workspace.join("note.txt")).ok(),
        ),
        (
            vec![InputRequest {
                id: request.id,
                ask: Ask::Approval(ApprovalAsk {
                    source: ApprovalSource::BuiltInTool {
                        name: "write".into(),
                    },
                    operation: Operation::WritePath {
                        path: sandbox.workspace.join("note.txt"),
                        scope: PathScope::Workspace,
                    },
                    reason,
                }),
                tool_call: Some(ToolCallId::new("edit")),
                timeout: None,
            }],
            JobState::WaitingForInput,
            Ok(()),
            Some(SessionEvent::InputResolved {
                id: request.id,
                resolution: Resolution::Answered,
            }),
            Err(Some(ErrorCode::InputNotOpen)),
            JobState::Completed,
            Some("approved".to_owned()),
        ),
        "log:\n{}",
        sandbox.daemon_log()
    );
}

// Covers: a daemon session's turn that waits for a person with nobody
// attached runs the user's `input_requested` hooks, which name the session
// and what it asks, so a user away from every client can be told (#1660).
// Owner: daemon input requests.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_turn_that_asks_tells_the_hooks() {
    let sandbox = Sandbox::new();
    let home = sandbox.daemon.home();
    let events = home.join("asked.jsonl");
    let hook = home.join("on-input-requested");
    std::fs::write(&hook, format!("#!/bin/sh\ncat >> '{}'\n", events.display())).unwrap();
    std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    std::fs::write(
        home.join("hooks.toml"),
        format!(
            "version = 1\n\n[[hook]]\nid = \"notify\"\non = \"input_requested\"\ncommand = [\"{}\"]\ntimeout = \"5s\"\n",
            hook.display()
        ),
    )
    .unwrap();
    let (_daemon, _client, session) = prompt_a_supervised_write(&sandbox).await;

    let event: serde_json::Value = sandbox
        .daemon
        .poll_until("the hook to run", DEADLINE, async || {
            let written = std::fs::read_to_string(&events).ok()?;
            serde_json::from_str(written.lines().next()?).ok()
        })
        .await;

    assert_eq!(
        (
            &event["event"],
            &event["identity"]["session_id"],
            &event["payload"]["kind"],
            event["payload"]["summary"]
                .as_str()
                .is_some_and(|summary| !summary.is_empty()),
        ),
        (
            &serde_json::json!("input_requested"),
            &serde_json::json!(session.as_str()),
            &serde_json::json!("approval"),
            true,
        ),
        "event: {event:#}\nlog:\n{}",
        sandbox.daemon_log()
    );
}

/// A sandbox whose config sets `[questionnaire] timeout_seconds`.
fn sandbox_with_question_timeout(seconds: u64) -> Sandbox {
    let sandbox = Sandbox::new();
    std::fs::write(
        sandbox.daemon.home().join("config.toml"),
        format!("{CONFIG}\n[questionnaire]\ntimeout_seconds = {seconds}\n"),
    )
    .unwrap();
    sandbox
}

/// Prompts `session` to ask which color, with `Blue` as the fallback
/// answers, through the `tool-call` fixture.
async fn ask_color_with_fallback(client: &Client, session: &SessionId) {
    let call = serde_json::json!({
        "id": "ask",
        "name": "questionnaire",
        "arguments": {
            "questions": [{
                "id": "color",
                "question": "Which color?",
                "choices": ["Red", "Blue"],
            }],
            "on_timeout": {"answers": {"color": "Blue"}, "reason": "either works"},
        },
    });
    client
        .request::<SessionPrompt>(&SessionPromptParams {
            session: session.clone(),
            text: call.to_string(),
        })
        .await
        .unwrap();
}

fn blue() -> InputAnswer {
    InputAnswer::Answers {
        answers: [("color".to_owned(), vec!["Blue".to_owned()])].into(),
    }
}

/// Reads until the color question's tool call completes, and returns its
/// result as JSON.
async fn color_result(follower: &mut Follower) -> serde_json::Value {
    let ask = ToolCallId::new("ask");
    follower
        .read_until(|event| {
            matches!(event, SessionEvent::ToolCall(update)
                if update.id == ask && update.status == Some(ToolStatus::Completed))
        })
        .await;
    let text: String = follower
        .events
        .iter()
        .filter_map(|(_, event)| match event {
            SessionEvent::ToolCall(update) if update.id == ask => update.content.clone(),
            _ => None,
        })
        .flatten()
        .filter_map(|content| match content {
            ToolContent::Text { text } => Some(text),
            _ => None,
        })
        .collect();
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{error}: {text}"))
}

/// The first input request `events` open.
fn first_request(events: &[(Seq, SessionEvent)]) -> InputRequest {
    events
        .iter()
        .find_map(|(_, event)| match event {
            SessionEvent::InputRequested { request } => Some((**request).clone()),
            _ => None,
        })
        .expect("a request opened")
}

fn resolutions(events: &[(Seq, SessionEvent)]) -> Vec<Resolution> {
    events
        .iter()
        .filter_map(|(_, event)| match event {
            SessionEvent::InputResolved { resolution, .. } => Some(*resolution),
            _ => None,
        })
        .collect()
}

// Covers: with `[questionnaire] timeout_seconds` set, a question whose
// agent offered fallback answers tells clients with what and why it will
// be answered, and, with nobody answering, the daemon answers it with the
// fallback when the timeout passes: clients see it time out and the tool
// gets the fallback answers, marked as the fallback (#1660).
// Owner: daemon input requests.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_question_nobody_answers_times_out_with_its_fallback() {
    // The shortest timeout the config takes.
    let sandbox = sandbox_with_question_timeout(1);
    let (_daemon, client) = sandbox.start_daemon("tool-call").await;
    let session = new_session(&client, &sandbox.workspace).await;
    let (mut watcher, _) = Follower::attach(&sandbox, &session, Vec::new(), None).await;
    ask_color_with_fallback(&client, &session).await;

    let result = color_result(&mut watcher).await;
    let job = settled(&sandbox, &client, |job| {
        job.session.as_ref() == Some(&session)
    })
    .await;
    let timeout = first_request(&watcher.events)
        .timeout
        .map(|timeout| (timeout.answer, timeout.reason));

    assert_eq!(
        (timeout, resolutions(&watcher.events), result, job.state),
        (
            Some((blue(), "either works".to_owned())),
            vec![Resolution::TimedOut],
            serde_json::json!({
                "answers": [{"id": "color", "answer": "Blue"}],
                "source": "timeout_fallback",
            }),
            JobState::Completed,
        ),
        "log:\n{}",
        sandbox.daemon_log()
    );
}

// Covers: `rho/session/hold` stops a question's timeout: attached clients
// see `input_held`, a client that attaches later finds the request without
// a timeout, and the user's answer then reaches the tool as their own
// (#1660).
// Owner: daemon input requests.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_held_question_waits_for_the_users_answer() {
    // Far longer than the test, so the hold always comes first.
    let sandbox = sandbox_with_question_timeout(3600);
    let (_daemon, client) = sandbox.start_daemon("tool-call").await;
    let session = new_session(&client, &sandbox.workspace).await;
    let (mut watcher, _) = Follower::attach(&sandbox, &session, Vec::new(), None).await;
    ask_color_with_fallback(&client, &session).await;
    watcher
        .read_until(|event| matches!(event, SessionEvent::InputRequested { .. }))
        .await;
    let request = first_request(&watcher.events).id;

    client
        .request::<SessionHold>(&SessionHoldParams {
            session: session.clone(),
            request,
        })
        .await
        .unwrap();
    watcher
        .read_until(|event| matches!(event, SessionEvent::InputHeld { .. }))
        .await;
    let (_late, attached) = Follower::attach(&sandbox, &session, Vec::new(), None).await;
    let answered = answer(&client, &session, request, blue()).await;
    let result = color_result(&mut watcher).await;

    assert_eq!(
        (
            attached
                .inputs
                .iter()
                .map(|request| (request.id, request.timeout.clone()))
                .collect::<Vec<_>>(),
            answered,
            resolutions(&watcher.events),
            result,
        ),
        (
            vec![(request, None)],
            Ok(()),
            vec![Resolution::Answered],
            serde_json::json!({
                "answers": [{"id": "color", "answer": "Blue"}],
                "source": "user",
            }),
        ),
        "log:\n{}",
        sandbox.daemon_log()
    );
}
