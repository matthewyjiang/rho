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
    methods::{JobList, SessionAttach, SessionCancel, SessionDetach, SessionNew, SessionPrompt},
    Client, ClientError, ClientInfo, Cursor, DaemonNotification, DetachReason, ErrorCode, JobKind,
    JobListParams, JobState, JobSummary, Notifications, Replay, Seq, SessionAttachParams,
    SessionCancelParams, SessionDetachParams, SessionDetachedParams, SessionEvent, SessionId,
    SessionNewParams, SessionPromptParams, StreamId,
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
        let daemon = DaemonProcess(self.rho(mode).args(["daemon", "run"]).spawn().unwrap());
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
    sandbox
        .daemon
        .poll_until("the turn to end", DEADLINE, async || {
            let jobs = client.request::<JobList>(&JobListParams {}).await.unwrap();
            let job = jobs
                .jobs
                .into_iter()
                .find(|job| &job.session == session)
                .expect("the daemon lists the session's job");
            match job.state {
                JobState::Running | JobState::WaitingForInput => None,
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
            session.clone(),
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
    ) -> (Self, Replay, JobState) {
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
            stream: attached.stream,
            events,
        };
        (follower, attached.replay, attached.state)
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
            // A reset numbers its replay at the point the turn in progress
            // began, so a cold follower takes equal seqs as they come.
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
    let (mut watcher, watcher_replay, watcher_state) =
        Follower::attach(&sandbox, &session, Vec::new(), None).await;
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

    let (mut leaver, resumed_replay, resumed_state) =
        Follower::attach(&sandbox, &session, kept, Some(cursor)).await;
    leaver.read_until(is_state(JobState::Cancelled)).await;
    let (mut latecomer, late_replay, _) =
        Follower::attach(&sandbox, &session, Vec::new(), None).await;
    latecomer.read_until(is_state(JobState::Cancelled)).await;

    let seqs: Vec<u64> = watcher.events.iter().map(|(seq, _)| seq.0).collect();
    assert_eq!(
        seqs,
        (1..=seqs.len() as u64).collect::<Vec<_>>(),
        "the stream numbers every event once, in order"
    );
    assert_eq!(
        (
            (watcher_replay, watcher_state),
            (resumed_replay, resumed_state),
            late_replay
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
