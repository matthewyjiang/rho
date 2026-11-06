//! Threads the daemon hosts, through the real binary and its socket: a
//! turn outlives the client that started it, and every way a turn can end
//! early is recorded on its job.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{JobsList, ThreadCancel, ThreadCreate, ThreadPrompt},
    Client, ClientError, ClientInfo, ErrorCode, JobId, JobKind, JobState, JobSummary,
    JobsListParams, ThreadCancelParams, ThreadCreateParams, ThreadPromptParams,
};
use tokio::net::UnixStream;

use super::harness::{DaemonProcess, DaemonSandbox};

/// Failure bound for each wait on the daemon. Every turn here answers from
/// the fixture provider without network; this only stops a hung test.
const DEADLINE: Duration = Duration::from_secs(60);

/// The reply the fixture provider gives, which the test finds in the
/// session file.
const REPLY: &str = "reply saved by the daemon thread";

const CONFIG: &str = r#"provider = "xai"
model = "grok-fixture"
auth = "xai-oauth"

[web_search]
hosted = false
provider = "disabled"
"#;

struct Sandbox {
    daemon: DaemonSandbox,
    /// The thread's workspace, canonicalized as the daemon records it.
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
    async fn start_daemon(&self, mode: &str) -> (DaemonProcess, Client<UnixStream>) {
        let daemon = DaemonProcess(self.rho(mode).args(["daemon", "run"]).spawn().unwrap());
        // A daemon killed earlier left its socket file behind, so wait for
        // one that answers rather than one that exists.
        let client = self
            .daemon
            .poll_until("the daemon to answer", DEADLINE, async || {
                self.try_connect().await
            })
            .await;
        (daemon, client)
    }

    async fn connect(&self) -> Client<UnixStream> {
        self.try_connect().await.expect("the daemon answers")
    }

    async fn try_connect(&self) -> Option<Client<UnixStream>> {
        let stream = UnixStream::connect(self.daemon.socket()).await.ok()?;
        let client = ClientInfo {
            name: "daemon-threads-test".into(),
            version: "0".into(),
        };
        Client::handshake(stream, client)
            .await
            .ok()
            .map(|(client, _)| client)
    }

    fn daemon_log(&self) -> String {
        self.daemon.daemon_log()
    }

    /// The records saved in session `session_id`'s file.
    fn session_records(&self, session_id: &str) -> Vec<serde_json::Value> {
        // Each session is a directory named for its id, below its
        // workspace's.
        let suffix = format!("_{session_id}");
        let session = std::fs::read_dir(self.daemon.home().join("sessions"))
            .unwrap()
            .flatten()
            .filter_map(|workspace| std::fs::read_dir(workspace.path()).ok())
            .flatten()
            .flatten()
            .find(|entry| entry.file_name().to_string_lossy().ends_with(&suffix))
            .unwrap_or_else(|| panic!("no session directory for {session_id}"))
            .path();
        std::fs::read_to_string(session.join("session.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

async fn create_thread(client: &mut Client<UnixStream>, workspace: &Path) -> (JobId, String) {
    let created = client
        .request::<ThreadCreate>(&ThreadCreateParams {
            cwd: workspace.to_path_buf(),
        })
        .await
        .unwrap();
    (created.thread, created.session_id)
}

async fn prompt(client: &mut Client<UnixStream>, thread: &JobId) -> Result<(), Option<ErrorCode>> {
    client
        .request::<ThreadPrompt>(&ThreadPromptParams {
            thread: thread.clone(),
            text: "hello".into(),
        })
        .await
        .map(|_| ())
        .map_err(|error| match error {
            ClientError::Rpc { error, .. } => error.error_code(),
            error => panic!("prompt failed outside the protocol: {error}"),
        })
}

/// Polls `rho/jobs/list` until `thread` is in a state other than running
/// or waiting for input, and returns the job then.
async fn settled_job(
    sandbox: &Sandbox,
    client: &mut Client<UnixStream>,
    thread: &JobId,
) -> JobSummary {
    sandbox
        .daemon
        .poll_until("the turn to end", DEADLINE, async || {
            let jobs = client
                .request::<JobsList>(&JobsListParams {})
                .await
                .unwrap();
            let job = jobs
                .jobs
                .into_iter()
                .find(|job| &job.id == thread)
                .expect("the daemon lists the thread");
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
// job completed with the thread's workspace and session, and the session
// file holds the model's reply.
// Owner: daemon thread hosting.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_turn_outlives_the_client_that_prompted_it() {
    let sandbox = Sandbox::new();
    let (_daemon, mut client) = sandbox.start_daemon("fixed").await;
    let (thread, session_id) = create_thread(&mut client, &sandbox.workspace).await;
    assert_eq!(prompt(&mut client, &thread).await, Ok(()));
    drop(client);

    let mut client = sandbox.connect().await;
    let job = settled_job(&sandbox, &mut client, &thread).await;

    assert_eq!(
        (
            job.state,
            job.kind,
            job.project_root,
            job.parent,
            job.session_id.as_str(),
            job.error
        ),
        (
            JobState::Completed,
            JobKind::Thread,
            sandbox.workspace.clone(),
            None,
            session_id.as_str(),
            None
        ),
        "log:\n{}",
        sandbox.daemon_log()
    );
    let records = sandbox.session_records(&session_id);
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
    /// `rho/thread/cancel`.
    Cancel,
    /// `rho daemon stop`, then a new daemon.
    Stop,
    /// The daemon is killed, then a new daemon starts.
    Kill,
}

// Covers: a turn cancelled through `rho/thread/cancel` is recorded
// cancelled and its thread takes the next prompt; a turn the daemon's stop
// interrupts is recorded interrupted, within `rho daemon stop`'s wait; a
// turn whose daemon died is recorded interrupted by the next daemon. A
// thread from an earlier daemon is listed but refuses prompts as not
// loaded.
// Owner: daemon thread hosting and job records.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_turn_ended_early_is_recorded_on_its_job() {
    let cases = [
        (Ending::Cancel, JobState::Cancelled, Ok(())),
        (
            Ending::Stop,
            JobState::Interrupted,
            Err(Some(ErrorCode::ThreadNotLoaded)),
        ),
        (
            Ending::Kill,
            JobState::Interrupted,
            Err(Some(ErrorCode::ThreadNotLoaded)),
        ),
    ];
    for (ending, expected_state, expected_next_prompt) in cases {
        let sandbox = Sandbox::new();
        // The fixture holds each turn until it is cancelled.
        let (mut daemon, mut client) = sandbox.start_daemon("delay").await;
        let (thread, _) = create_thread(&mut client, &sandbox.workspace).await;
        assert_eq!(prompt(&mut client, &thread).await, Ok(()), "{ending:?}");

        let (_daemon, mut client) = match ending {
            Ending::Cancel => {
                client
                    .request::<ThreadCancel>(&ThreadCancelParams {
                        thread: thread.clone(),
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
        let state = settled_job(&sandbox, &mut client, &thread).await.state;
        let next_prompt = prompt(&mut client, &thread).await;

        assert_eq!(
            (state, next_prompt),
            (expected_state, expected_next_prompt),
            "{ending:?}: log:\n{}",
            sandbox.daemon_log()
        );
    }
}
