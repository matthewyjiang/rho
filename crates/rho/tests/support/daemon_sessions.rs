//! Sessions the daemon hosts, through the real binary and its socket: a
//! turn outlives the client that started it, every way a turn can end early
//! is recorded on its job, and clients that attach, leave, and come back see
//! the same events.

use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{
        JobList, SessionAnswer, SessionAttach, SessionCancel, SessionDetach, SessionHold,
        SessionNew, SessionPrompt,
    },
    AnswerStop, ApprovalAsk, ApprovalSource, Ask, Client, ClientEnvironment, ClientError,
    ClientInfo, ContentBlock, Cursor, DaemonNotification, DetachReason, ErrorCode, InputAnswer,
    InputRequest, InputRequestId, JobId, JobKind, JobListParams, JobState, JobSummary,
    Notifications, Operation, PathScope, Replay, Resolution, Seq, SessionAnswerParams,
    SessionAttachParams, SessionAttachResponse, SessionCancelParams, SessionDetachParams,
    SessionDetachedParams, SessionEvent, SessionHoldParams, SessionId, SessionNewParams,
    SessionPromptParams, StreamId, ToolCallId, ToolContent, ToolStatus, TurnError, TurnErrorKind,
    TurnOutcome,
};
use rho_tui_pty::IsolatedHome;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    net::UnixStream,
};

/// Failure bound for each wait on the daemon. Every turn here answers from
/// the fixture provider without network; this only stops a hung test.
pub(super) const DEADLINE: Duration = Duration::from_secs(60);

/// How often a wait polls the daemon's socket, process, or job list. The
/// daemon pushes nothing to clients yet (#1657), so polling is the only way
/// to see it change.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The reply the fixture provider gives, which the test finds in the
/// session file.
pub(super) const REPLY: &str = "reply saved by the daemon session";

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

/// An agent a session is saved as, other than the default.
const ARCHIVIST_AGENT: &str = "---
description: Fixture agent a session is saved as
---

Archive.
";

const WORKFLOW_AGENT: &str = "---
description: Fixture agent for a daemon workflow run
---

Do the task.
";

/// An [`IsolatedHome`] with the fixture provider's config, whose daemon a
/// test starts itself or a client in it starts. Dropping it stops that
/// daemon, also while a test panics. Its clients should share one fixture
/// mode (see [`IsolatedHome`]).
pub(super) struct Sandbox {
    pub(super) isolated: IsolatedHome,
    /// The session's workspace, canonicalized as the daemon records it.
    pub(super) workspace: PathBuf,
}

impl Sandbox {
    pub(super) fn new() -> Self {
        let isolated = IsolatedHome::new().unwrap();
        std::fs::write(&isolated.config_path, CONFIG).unwrap();
        let workspace = isolated.workspace.canonicalize().unwrap();
        Self {
            isolated,
            workspace,
        }
    }

    /// `rho` in the workspace, in the sandbox, with the fixture provider in
    /// `mode`.
    pub(super) fn rho(&self, mode: &str) -> Command {
        let mut command = self.isolated.command(env!("CARGO_BIN_EXE_rho"));
        command
            .current_dir(&self.workspace)
            .env("RHO_AUTOMATION_TEST_MODE", mode)
            .env("RHO_AUTOMATION_TEST_RESPONSE", REPLY)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    /// Starts `rho daemon run` with the fixture in `mode` and connects to
    /// it once it answers.
    pub(super) async fn start_daemon(&self, mode: &str) -> (Child, Client) {
        self.start_daemon_with(mode, &[]).await
    }

    /// Starts `rho daemon run` with `args` and the fixture in `mode`, and
    /// connects to it once it answers.
    pub(super) async fn start_daemon_with(&self, mode: &str, args: &[&str]) -> (Child, Client) {
        self.spawn_daemon(self.rho(mode), args).await
    }

    /// [`Self::start_daemon`], with the TUI fixture answering the title
    /// agent, whose one-shot provider skips the automation fixture: it
    /// names a session "Fixture session title".
    async fn start_titling_daemon(&self, mode: &str) -> (Child, Client) {
        let mut command = self.rho(mode);
        command.env("RHO_TUI_TEST_MODE", "matrix");
        self.spawn_daemon(command, &[]).await
    }

    /// Runs `command daemon run args`, and connects once the daemon answers.
    ///
    /// The sandbox stops the daemon when it drops; the returned process is
    /// for a test that waits for the daemon's exit or kills it. Dropping it
    /// does neither.
    pub(super) async fn spawn_daemon(
        &self,
        mut command: Command,
        args: &[&str],
    ) -> (Child, Client) {
        // Outside every session's workspace, as a service manager starts it,
        // so nothing a session runs can lean on the daemon's own directory.
        let daemon = command
            .current_dir(self.rho_home())
            .args(["daemon", "run"])
            .args(args)
            .spawn()
            .unwrap();
        // A daemon killed earlier left its socket file behind, so wait for
        // one that answers rather than one that exists.
        let client = self
            .poll_until("the daemon to answer", async || self.try_connect().await)
            .await
            .0;
        (daemon, client)
    }

    pub(super) async fn connect(&self) -> Client {
        self.connect_following().await.0
    }

    /// A connection and the session events the daemon sends on it.
    pub(super) async fn connect_following(&self) -> (Client, Notifications) {
        self.try_connect().await.expect("the daemon answers")
    }

    async fn try_connect(&self) -> Option<(Client, Notifications)> {
        let stream = UnixStream::connect(self.isolated.daemon_socket()?)
            .await
            .ok()?;
        let client = ClientInfo {
            name: "daemon-sessions-test".into(),
            version: "0".into(),
        };
        Client::handshake(stream, client)
            .await
            .ok()
            .map(|(client, notifications, _)| (client, notifications))
    }

    pub(super) fn daemon_log(&self) -> String {
        self.isolated.daemon_log()
    }

    /// Writes the fixture config with `permission_mode = "<mode>"`, which a
    /// daemon started after reads.
    pub(super) fn set_permission_mode(&self, mode: &str) {
        self.set_config(mode, "");
    }

    /// Writes the fixture config with `permission_mode = "<mode>"` and the
    /// TOML `tables` after it, which a daemon started after reads.
    pub(super) fn set_config(&self, mode: &str, tables: &str) {
        std::fs::write(
            &self.isolated.config_path,
            format!("permission_mode = \"{mode}\"\n{CONFIG}\n{tables}"),
        )
        .unwrap();
    }

    /// The daemon's `RHO_HOME`, where user `hooks.toml` lives.
    pub(super) fn rho_home(&self) -> PathBuf {
        self.isolated.rho_home()
    }

    /// Calls `attempt` every [`POLL_INTERVAL`] until it returns `Some`, and
    /// fails with the daemon log once [`DEADLINE`] passes.
    pub(super) async fn poll_until<T>(
        &self,
        what: &str,
        mut attempt: impl AsyncFnMut() -> Option<T>,
    ) -> T {
        let started = Instant::now();
        loop {
            if let Some(value) = attempt().await {
                return value;
            }
            assert!(
                started.elapsed() < DEADLINE,
                "timed out after {DEADLINE:?} waiting for {what}; daemon log:\n{}",
                self.daemon_log()
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Opens a new session in the workspace.
    pub(super) async fn new_session(&self, client: &Client) -> SessionId {
        self.new_session_with_environment(client, self.client_environment())
            .await
    }

    /// Opens a new session in the workspace for a client with `environment`.
    pub(super) async fn new_session_with_environment(
        &self,
        client: &Client,
        environment: ClientEnvironment,
    ) -> SessionId {
        client
            .request::<SessionNew>(&SessionNewParams {
                cwd: self.workspace.clone(),
                environment,
            })
            .await
            .unwrap()
            .session
    }

    /// Prompts `session` with "hello"; the error code of a refused prompt.
    pub(super) async fn prompt(
        &self,
        client: &Client,
        session: &SessionId,
    ) -> Result<(), Option<ErrorCode>> {
        client
            .request::<SessionPrompt>(&SessionPromptParams {
                session: session.clone(),
                content: vec![ContentBlock::Text {
                    text: "hello".into(),
                }],
                timeout_ms: None,
                environment: self.client_environment(),
            })
            .await
            .map(|_| ())
            .map_err(|error| match error {
                ClientError::Rpc { error, .. } => error.error_code(),
                error => panic!("prompt failed outside the protocol: {error}"),
            })
    }

    /// Prompts `session` with the tool call `call`, which the `tool-call`
    /// fixture makes; returns once the turn saved the prompt.
    pub(super) async fn prompt_tool_call(
        &self,
        client: &Client,
        session: &SessionId,
        call: &serde_json::Value,
    ) {
        let environment = self.client_environment();
        self.prompt_tool_call_with_environment(client, session, call, environment)
            .await;
    }

    /// [`Self::prompt_tool_call`] from a client whose environment is
    /// `environment`.
    pub(super) async fn prompt_tool_call_with_environment(
        &self,
        client: &Client,
        session: &SessionId,
        call: &serde_json::Value,
        environment: ClientEnvironment,
    ) {
        client
            .request::<SessionPrompt>(&SessionPromptParams {
                session: session.clone(),
                content: vec![ContentBlock::Text {
                    text: call.to_string(),
                }],
                timeout_ms: None,
                environment,
            })
            .await
            .unwrap();
    }

    /// The environment a client in the sandbox sends, which its sessions'
    /// processes run with (#1659): the whole environment
    /// [`IsolatedHome::command`] gives a child, so nothing a session runs
    /// reaches the developer's rho home or daemon. It has no fixture mode;
    /// only the daemon does.
    pub(super) fn client_environment(&self) -> ClientEnvironment {
        // `command` clears the environment, so the variables it sets are
        // all a child gets.
        self.isolated
            .command(env!("CARGO_BIN_EXE_rho"))
            .get_envs()
            .filter_map(|(name, value)| {
                Some((name.to_str()?.to_owned(), value?.to_str()?.to_owned()))
            })
            .collect()
    }

    /// The directory `session` is saved in, once it is: each session is a
    /// directory named for its id, below its workspace's.
    fn session_dir(&self, session: &SessionId) -> Option<PathBuf> {
        let suffix = format!("_{session}");
        std::fs::read_dir(self.rho_home().join("sessions"))
            .ok()?
            .flatten()
            .filter_map(|workspace| std::fs::read_dir(workspace.path()).ok())
            .flatten()
            .flatten()
            .find(|entry| entry.file_name().to_string_lossy().ends_with(&suffix))
            .map(|entry| entry.path())
    }

    /// Whether `session`'s file has saved a user message yet. A record still
    /// being written is skipped.
    fn saved_a_user_message(&self, session: &SessionId) -> bool {
        self.session_dir(session)
            .and_then(|dir| std::fs::read_to_string(dir.join("session.jsonl")).ok())
            .is_some_and(|file| {
                file.lines()
                    .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                    .flat_map(|record| {
                        record["transition"]["delta"]["appended_history"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default()
                    })
                    .any(|message| message.get("User").is_some())
            })
    }

    /// The records saved in `session`'s file.
    pub(super) fn session_records(&self, session: &SessionId) -> Vec<serde_json::Value> {
        let session = self
            .session_dir(session)
            .unwrap_or_else(|| panic!("no session directory for {session}"));
        std::fs::read_to_string(session.join("session.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

/// Polls `rho/job/list` until the job of `session`'s turns is in a state
/// other than running or waiting for input, and returns the job then.
pub(super) async fn settled_job(
    sandbox: &Sandbox,
    client: &Client,
    session: &SessionId,
) -> JobSummary {
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
        .poll_until("the job to end", async || {
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
                | JobState::TimedOut
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
    let session = sandbox.new_session(&client).await;
    assert_eq!(sandbox.prompt(&client, &session).await, Ok(()));
    drop(client);

    let client = sandbox.connect().await;
    let job = settled_job(&sandbox, &client, &session).await;
    let unknown = sandbox
        .prompt(&client, &SessionId::new("no-such-session"))
        .await;

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
            Err(Some(ErrorCode::SessionNotFound))
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
// and how to log in, never the provider's text, here and in the job list
// (#1669).
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
        let session = sandbox.new_session(&client).await;
        let (mut watcher, _) = Follower::attach(&sandbox, &session, Vec::new(), None).await;
        // A turn that fails before it saves the prompt answers the prompt
        // with the failure; the clients learn it all the same.
        let _ = sandbox.prompt(&client, &session).await;
        watcher.read_until(is_state(state)).await;

        let ending: Vec<SessionEvent> = watcher.events[watcher.events.len() - 2..]
            .iter()
            .map(|(_, event)| event.clone())
            .collect();
        let listed = match &outcome {
            TurnOutcome::Failed { error } => Some(error.message.clone()),
            _ => None,
        };
        assert_eq!(
            ending,
            vec![
                SessionEvent::TurnEnded { outcome },
                SessionEvent::State { state }
            ],
            "{mode}: log:\n{}",
            sandbox.daemon_log()
        );
        let job = settled_job(&sandbox, &client, &session).await;
        assert_eq!((job.state, job.error), (state, listed), "{mode}: job list");
    }
}

// Covers: a prompt's image and resource blocks reach the model, the image
// as an image and the resource as its text fenced under its URI, and the
// prompt's user message shows clients what was saved; a prompt with no
// blocks, or an image that is not what it declares, is refused as invalid
// params before a turn starts (#1670).
// Owner: daemon session prompt content.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_prompts_image_and_resource_reach_the_model() {
    // A 1x1 PNG.
    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
    let sandbox = Sandbox::new();
    // The fixture answers with the request it got.
    let (_daemon, client) = sandbox.start_daemon("inspect").await;
    let session = sandbox.new_session(&client).await;
    let (mut watcher, _) = Follower::attach(&sandbox, &session, Vec::new(), None).await;
    let refused = [
        Vec::new(),
        vec![ContentBlock::Image {
            mime_type: "image/jpeg".into(),
            data: PNG.into(),
        }],
    ];
    for content in refused {
        let refusal = client
            .request::<SessionPrompt>(&SessionPromptParams {
                session: session.clone(),
                content: content.clone(),
                timeout_ms: None,
                environment: sandbox.client_environment(),
            })
            .await
            .map(|_| ())
            .map_err(|error| match error {
                ClientError::Rpc { error, .. } => error.error_code(),
                error => panic!("prompt failed outside the protocol: {error}"),
            });
        assert_eq!(refusal, Err(Some(ErrorCode::InvalidParams)), "{content:?}");
    }
    client
        .request::<SessionPrompt>(&SessionPromptParams {
            session: session.clone(),
            content: vec![
                ContentBlock::Text {
                    text: "describe these".into(),
                },
                ContentBlock::Image {
                    mime_type: "image/png".into(),
                    data: PNG.into(),
                },
                ContentBlock::Resource {
                    uri: "file:///notes.md".into(),
                    text: "remember the milk".into(),
                },
            ],
            timeout_ms: None,
            environment: sandbox.client_environment(),
        })
        .await
        .unwrap();
    watcher.read_until(is_state(JobState::Completed)).await;

    let fenced = "```resource file:///notes.md\nremember the milk\n```";
    let shown = watcher.events.iter().find_map(|(_, event)| match event {
        SessionEvent::UserMessage { content, .. } => Some(content.clone()),
        _ => None,
    });
    assert_eq!(
        shown,
        Some(vec![
            ContentBlock::Text {
                text: "describe these".into(),
            },
            ContentBlock::Image {
                mime_type: "image/png".into(),
                data: PNG.into(),
            },
            ContentBlock::Text {
                text: fenced.into(),
            },
        ]),
        "log:\n{}",
        sandbox.daemon_log()
    );
    let answer = watcher.events.iter().find_map(|(_, event)| match event {
        SessionEvent::TurnEnded {
            outcome: TurnOutcome::Answered { text, .. },
        } => Some(text.clone()),
        _ => None,
    });
    let request: serde_json::Value =
        serde_json::from_str(&answer.expect("the turn answered")).unwrap();
    let prompt = request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|message| message.get("User").is_some())
        .cloned();
    assert_eq!(
        prompt,
        Some(serde_json::json!({"User": [
            {"Text": "describe these"},
            {"Image": {"data": PNG, "mime_type": "image/png"}},
            {"Text": fenced},
        ]}))
    );
}

// Covers: after a session's first answered turn, the daemon titles it in
// the background, saves the title where session lists read it, and tells
// attached clients with `title_changed`; a client that attaches later,
// whose replay need not hold that event, gets it in the attach response
// (#1670).
// Owner: daemon session titles.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_answered_session_is_titled_for_its_clients() {
    let sandbox = Sandbox::new();
    let (_daemon, client) = sandbox.start_titling_daemon("fixed").await;
    let session = sandbox.new_session(&client).await;
    let (mut watcher, before) = Follower::attach(&sandbox, &session, Vec::new(), None).await;
    assert_eq!(sandbox.prompt(&client, &session).await, Ok(()));
    watcher
        .read_until(|event| matches!(event, SessionEvent::TitleChanged { .. }))
        .await;
    let (_, after) = Follower::attach(&sandbox, &session, Vec::new(), None).await;

    let listed = sandbox
        .rho("fixed")
        .args(["sessions", "list", "--json"])
        .stdout(Stdio::piped())
        .output()
        .unwrap();
    let listing: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    let saved = listing["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|listed| listed["id"] == session.to_string())
        .map(|listed| listed["title"].clone());
    let title = "Fixture session title";
    assert_eq!(
        (
            before.title,
            watcher.events.last().map(|(_, event)| event.clone()),
            after.title,
            saved,
        ),
        (
            None,
            Some(SessionEvent::TitleChanged {
                title: title.into()
            }),
            Some(title.into()),
            Some(serde_json::Value::from(title)),
        ),
        "log:\n{}",
        sandbox.daemon_log()
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

// Covers: a subagent a daemon session starts runs in the daemon and is
// listed as a child job of the session's job, in the session's workspace,
// completed once it finished, with the session it saved its reply in, which
// a client cannot reopen: only its parent continues it (#1661).
// Owner: daemon subagent jobs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_subagent_is_a_child_job_of_its_session() {
    let sandbox = Sandbox::new();
    // The fixture makes the call the prompt spells out as JSON.
    let (_daemon, client) = sandbox.start_daemon("tool-call").await;
    let session = sandbox.new_session(&client).await;
    let call = serde_json::json!({
        "id": "delegate",
        "name": "agent",
        "arguments": {"agent_id": "explorer", "prompt": "Look around."},
    });
    sandbox.prompt_tool_call(&client, &session, &call).await;

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
    assert_eq!(
        sandbox.prompt(&client, &child_session).await,
        Err(Some(ErrorCode::SessionOpenFailed))
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
    let session = sandbox.new_session(&client).await;
    let call = serde_json::json!({
        "id": "start",
        "name": "workflow",
        "arguments": {"action": "start", "file": "wf/main.star"},
    });
    sandbox.prompt_tool_call(&client, &session, &call).await;

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
    let runs: Vec<String> = std::fs::read_dir(sandbox.rho_home().join("workflows/runs"))
        .unwrap()
        .map(|run| run.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(workflow.workflow_run.map(|run| vec![run]), Some(runs));
}

// Covers: a turn cancelled through `rho/session/cancel` is recorded
// cancelled; a turn the daemon's stop interrupts is recorded interrupted,
// within `rho daemon stop`'s wait; a turn whose daemon died (`kill -9`) is
// recorded interrupted by the next daemon. Either way the session comes back
// (#1661): an attach reopens it, in its workspace, replaying its saved
// conversation and its job's state, and the next prompt continues it on the
// same job.
// Owner: daemon session hosting and job records.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_turn_ended_early_is_recorded_and_its_session_continues() {
    let cases = [
        (Ending::Cancel, JobState::Cancelled),
        (Ending::Stop, JobState::Interrupted),
        (Ending::Kill, JobState::Interrupted),
    ];
    for (ending, expected_state) in cases {
        let sandbox = Sandbox::new();
        // The fixture holds each turn until it is cancelled.
        let (mut daemon, client) = sandbox.start_daemon("delay").await;
        let session = sandbox.new_session(&client).await;
        assert_eq!(
            sandbox.prompt(&client, &session).await,
            Ok(()),
            "{ending:?}"
        );
        let jobs_of_session = async |client: &Client| -> Vec<JobId> {
            let jobs = client.request::<JobList>(&JobListParams {}).await.unwrap();
            jobs.jobs
                .into_iter()
                .filter(|job| job.session.as_ref() == Some(&session))
                .map(|job| job.id)
                .collect()
        };
        let job = jobs_of_session(&client).await;

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
                daemon.wait().unwrap();
                sandbox.start_daemon("delay").await
            }
            Ending::Kill => {
                drop(client);
                // Reaped, so its lock is free for the next daemon.
                daemon.kill().unwrap();
                daemon.wait().unwrap();
                sandbox.start_daemon("delay").await
            }
        };
        let state = settled_job(&sandbox, &client, &session).await.state;
        let (mut follower, attached) = Follower::attach(&sandbox, &session, Vec::new(), None).await;
        follower
            .read_until(|event| matches!(event, SessionEvent::UserMessage { .. }))
            .await;
        let next_prompt = sandbox.prompt(&client, &session).await;

        assert_eq!(
            (
                state,
                attached.replay,
                attached.workspace,
                attached.state,
                next_prompt,
                jobs_of_session(&client).await,
            ),
            (
                expected_state,
                Replay::Reset,
                sandbox.workspace.clone(),
                expected_state,
                Ok(()),
                job,
            ),
            "{ending:?}: log:\n{}",
            sandbox.daemon_log()
        );
    }
}

// Covers: a session no daemon ran, saved by a foreground `rho run`, is
// refused as in use while that run has it open; once the run is gone, an
// attach reopens it (#1661) in its own workspace, on a new session job that
// has run no turn yet, replaying its saved conversation, and it takes the
// next prompt.
// Owner: daemon session reopening.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_another_process_saved_reopens_once_it_lets_go() {
    let sandbox = Sandbox::new();
    let (_daemon, client) = sandbox.start_daemon("delay").await;
    // The fixture's model call waits, so the run holds its session open.
    let mut run = tokio::process::Command::from(sandbox.rho("delay"));
    run.args(["run", "--output", "jsonl", "hello"])
        .stdout(Stdio::piped())
        .kill_on_drop(true);
    let mut run = run.spawn().unwrap();
    let mut lines = BufReader::new(run.stdout.take().unwrap()).lines();
    let started = tokio::time::timeout(DEADLINE, lines.next_line())
        .await
        .expect("the run starts")
        .unwrap()
        .expect("the run writes run_started");
    let started: serde_json::Value = serde_json::from_str(&started).unwrap();
    let session = SessionId::new(started["session"].as_str().unwrap());
    sandbox
        .poll_until("the run to save its prompt", async || {
            sandbox.saved_a_user_message(&session).then_some(())
        })
        .await;
    let attach = async |client: &Client| {
        client
            .request::<SessionAttach>(&SessionAttachParams {
                session: session.clone(),
                after: None,
                environment: sandbox.client_environment(),
            })
            .await
            .map(|_| ())
            .map_err(|error| error.error_code())
    };
    let in_use = attach(&client).await;

    run.kill().await.unwrap();
    let (mut follower, attached) = Follower::attach(&sandbox, &session, Vec::new(), None).await;
    follower
        .read_until(|event| matches!(event, SessionEvent::UserMessage { .. }))
        .await;
    let jobs: Vec<_> = client
        .request::<JobList>(&JobListParams {})
        .await
        .unwrap()
        .jobs
        .into_iter()
        .filter(|job| job.session.as_ref() == Some(&session))
        .map(|job| (job.kind, job.state))
        .collect();
    let next_prompt = sandbox.prompt(&client, &session).await;

    assert_eq!(
        (
            in_use,
            attached.workspace,
            attached.state,
            jobs,
            next_prompt
        ),
        (
            Err(Some(ErrorCode::SessionInUse)),
            sandbox.workspace.clone(),
            JobState::Idle,
            vec![(JobKind::Session, JobState::Idle)],
            Ok(())
        ),
        "log:\n{}",
        sandbox.daemon_log()
    );
}

// Covers: a session saved as an agent other than the default reopens as
// that agent while its definition is unchanged, and is refused once the
// definition changed, rather than reopened as the default agent (#1661).
// Owner: daemon session reopening.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_reopens_only_as_the_agent_it_was_saved_as() {
    let sandbox = Sandbox::new();
    let archivist = sandbox.isolated.home.join(".agents/agents/archivist.md");
    std::fs::create_dir_all(archivist.parent().unwrap()).unwrap();
    std::fs::write(&archivist, ARCHIVIST_AGENT).unwrap();
    let saved_as_archivist = || {
        let run = sandbox
            .rho("fixed")
            .args(["run", "--agent", "archivist", "--output", "jsonl", "hello"])
            .stdout(Stdio::piped())
            .output()
            .unwrap();
        assert!(run.status.success(), "{run:?}");
        let started = String::from_utf8(run.stdout).unwrap();
        let started: serde_json::Value =
            serde_json::from_str(started.lines().next().unwrap()).unwrap();
        SessionId::new(started["session"].as_str().unwrap())
    };
    let (kept, edited) = (saved_as_archivist(), saved_as_archivist());
    let (_daemon, client) = sandbox.start_daemon("fixed").await;

    let while_unchanged = sandbox.prompt(&client, &kept).await;
    std::fs::write(
        &archivist,
        ARCHIVIST_AGENT.replace("Archive.", "Archive everything."),
    )
    .unwrap();
    let once_changed = sandbox.prompt(&client, &edited).await;

    assert_eq!(
        (while_unchanged, once_changed),
        (Ok(()), Err(Some(ErrorCode::SessionOpenFailed))),
        "log:\n{}",
        sandbox.daemon_log()
    );
}

// Covers: a session that sits idle, with no turn under way and no client
// attached, past `--unload-idle-after` is unloaded, letting go of its file
// so another process can take it, here `rho sessions rm`; one a client
// follows, idle as long, stays loaded and held (#1661).
// Owner: daemon idle session unload.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_idle_session_is_unloaded_and_its_file_let_go() {
    let sandbox = Sandbox::new();
    let (_daemon, client) = sandbox
        .start_daemon_with("fixed", &["--unload-idle-after", "200ms"])
        .await;
    // Saved, so another process finds it.
    let saved_session = async || {
        let session = sandbox.new_session(&client).await;
        sandbox.prompt(&client, &session).await.unwrap();
        settled_job(&sandbox, &client, &session).await;
        session
    };
    // First, so without its follower it would unload no later than the other.
    let followed = saved_session().await;
    let (_follower, _) = Follower::attach(&sandbox, &followed, Vec::new(), None).await;
    let idle = saved_session().await;
    let delete = |session: &SessionId| {
        sandbox
            .rho("fixed")
            .args(["sessions", "rm", "--yes", session.as_str()])
            .status()
            .unwrap()
            .success()
    };

    sandbox
        .poll_until("the daemon to let the idle session go", async || {
            delete(&idle).then_some(())
        })
        .await;
    let followed_deleted = delete(&followed);

    assert!(!followed_deleted, "log:\n{}", sandbox.daemon_log());
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
                environment: sandbox.client_environment(),
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

/// What a conversation shows: its events without job states, context
/// usage, titles, and options, which describe the session as it was when they were
/// sent.
fn conversation(events: &[(Seq, SessionEvent)]) -> Vec<SessionEvent> {
    events
        .iter()
        .map(|(_, event)| event.clone())
        .filter(|event| {
            !matches!(
                event,
                SessionEvent::State { .. }
                    | SessionEvent::Usage { .. }
                    | SessionEvent::TitleChanged { .. }
                    | SessionEvent::OptionsChanged { .. }
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
    let session = sandbox.new_session(&client).await;
    let (mut watcher, watched) = Follower::attach(&sandbox, &session, Vec::new(), None).await;
    let (mut leaver, ..) = Follower::attach(&sandbox, &session, Vec::new(), None).await;

    assert_eq!(sandbox.prompt(&client, &session).await, Ok(()));
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
pub(super) async fn prompt_a_supervised_write(sandbox: &Sandbox) -> (Child, Client, SessionId) {
    sandbox.set_permission_mode("supervised");
    // The fixture makes the call the prompt spells out as JSON.
    let (daemon, client) = sandbox.start_daemon("tool-call").await;
    let session = sandbox.new_session(&client).await;
    let call = serde_json::json!({
        "id": "edit",
        "name": "write",
        "arguments": {"path": "note.txt", "content": "approved"},
    });
    sandbox.prompt_tool_call(&client, &session, &call).await;
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
        .poll_until("the turn to ask", async || {
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
    let home = sandbox.rho_home();
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
        .poll_until("the hook to run", async || {
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
        &sandbox.isolated.config_path,
        format!("{CONFIG}\n[questionnaire]\ntimeout_seconds = {seconds}\n"),
    )
    .unwrap();
    sandbox
}

/// Prompts `session` to ask which color, with `Blue` as the fallback
/// answers, through the `tool-call` fixture.
async fn ask_color_with_fallback(sandbox: &Sandbox, client: &Client, session: &SessionId) {
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
    sandbox.prompt_tool_call(client, session, &call).await;
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
    let session = sandbox.new_session(&client).await;
    let (mut watcher, _) = Follower::attach(&sandbox, &session, Vec::new(), None).await;
    ask_color_with_fallback(&sandbox, &client, &session).await;

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
    let session = sandbox.new_session(&client).await;
    let (mut watcher, _) = Follower::attach(&sandbox, &session, Vec::new(), None).await;
    ask_color_with_fallback(&sandbox, &client, &session).await;
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
