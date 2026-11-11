use std::{path::Path, sync::Mutex, time::Duration};

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::ToolSpec,
    tool::{Tool, ToolFuture, ToolInvocation, ToolOutput},
};
use serde_json::{json, Value};

use super::{signal::PARENT_ENDED, *};
use crate::tools::{
    mcp::exported_name::namespaced_tool_name,
    process::ProcessLimits,
    sdk_registry::{AppToolSet, StaticToolBundle},
};

/// The host's MCP server, whose task tools the watchers call.
const SERVER: &str = "t3-code";

/// The daemon, and so this module, is unix-only so far (#1665).
const LONG_RUNNING_COMMAND: &str = "sleep 300";

/// Bounds each wait on real process exits and signal polls; a hang fails.
const TEST_WAIT: Duration = Duration::from_secs(10);

/// The daemon session id of the watcher reading a signal.
const WATCHER_SESSION: &str = "5a7c9e1b-3d4f-4a6b-8c0d-2e4f6a8b0c1d";

type Calls = Arc<Mutex<Vec<(String, Value)>>>;

/// How a fixture task tool answers.
#[derive(Clone)]
enum Reply {
    /// At once, with this.
    Now(Box<Result<ToolOutput, ToolError>>),
    /// Never, as a host that hangs.
    Never,
}

impl Reply {
    fn now(result: Result<ToolOutput, ToolError>) -> Self {
        Self::Now(Box::new(result))
    }
}

/// A `t3-code` task tool that records its calls and answers as `reply`.
struct T3Tool {
    name: &'static str,
    calls: Calls,
    reply: Reply,
}

impl Tool for T3Tool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: namespaced_tool_name(SERVER, self.name),
            description: String::new(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        self.calls
            .lock()
            .unwrap()
            .push((self.name.to_owned(), invocation.into_arguments()));
        let reply = self.reply.clone();
        Box::pin(async move {
            match reply {
                Reply::Now(result) => *result,
                Reply::Never => std::future::pending().await,
            }
        })
    }
}

struct Fixture {
    _home: tempfile::TempDir,
    dir: WakeDir,
    tools: AppToolSet,
    calls: Calls,
    background: Background,
    wake: T3Wake,
}

impl Fixture {
    fn new() -> Self {
        Self::with_delegate(Reply::now(Ok(
            ToolOutput::text("delegated").with_structured_content(json!({"taskId": "task-1"}))
        )))
    }

    /// A fixture whose host answers `delegate_task` as `delegate` says.
    fn with_delegate(delegate: Reply) -> Self {
        let calls = Calls::default();
        let mut tools = AppToolSet::disabled();
        tools.add_bundle(StaticToolBundle::new(
            [
                ("delegate_task", delegate),
                ("task_status", Reply::now(Ok(ToolOutput::text("running")))),
                (
                    "task_cancel",
                    Reply::now(Ok(ToolOutput::text("cancel requested"))),
                ),
            ]
            .into_iter()
            .map(|(name, reply)| {
                Arc::new(T3Tool {
                    name,
                    calls: calls.clone(),
                    reply,
                }) as Arc<dyn Tool>
            })
            .collect(),
        ));
        let home = tempfile::tempdir().unwrap();
        let dir = WakeDir::in_home(home.path());
        let wake = T3Wake::new(dir.clone());
        Self {
            _home: home,
            dir,
            tools,
            calls,
            background: Background {
                session_id: "parent".into(),
                processes: Some(ProcessManager::new(ProcessLimits::default())),
                workflows: WorkflowRunTracker::new(),
                subagents: None,
            },
            wake,
        }
    }

    fn processes(&self) -> &ProcessManager {
        self.background.processes.as_ref().unwrap()
    }

    async fn start_process(&self) -> String {
        self.processes()
            .start(LONG_RUNNING_COMMAND.into(), Path::new("."), None)
            .await
            .unwrap()
            .process_id
    }

    async fn stop_process(&self, id: &str) {
        let exited = self.processes().notified_owned();
        self.processes().stop(id, Duration::ZERO).await.unwrap();
        tokio::time::timeout(TEST_WAIT, exited).await.unwrap();
    }

    async fn prompt_ended(&mut self) {
        self.prompt_ended_interrupted_by(&CancellationToken::new())
            .await
            .unwrap();
    }

    async fn prompt_ended_interrupted_by(
        &mut self,
        interrupt: &CancellationToken,
    ) -> anyhow::Result<()> {
        self.wake
            .prompt_ended(
                SessionTools::of_tool_set_for_tests(&self.tools),
                self.background.clone(),
                interrupt,
            )
            .await
    }

    /// Waits until the armed watcher's notifier has finished publishing.
    async fn notifier_settled(&mut self) {
        let notifier = self.wake.armed.as_mut().unwrap().notifier.as_mut().unwrap();
        tokio::time::timeout(TEST_WAIT, &mut notifier.task)
            .await
            .unwrap()
            .unwrap();
    }

    fn take_calls(&self) -> Vec<(String, Value)> {
        std::mem::take(&mut *self.calls.lock().unwrap())
    }

    /// The signal the delegated `/rho-wake <token>` task waits on.
    fn delegated_signal(task: &Value) -> String {
        task.as_str()
            .unwrap()
            .strip_prefix("/rho-wake ")
            .unwrap()
            .to_owned()
    }
}

// Covers: a prompt that ends with a process running arms exactly one watcher
// titled after it, kept across idle prompts; the exit fires the watcher's
// signal while the thread is idle, and the prompt T3 then sends is known as
// the wake. A broken arm, fire, or match leaves the thread asleep or wakes
// it twice. Owner: daemon T3 wake watcher.
#[tokio::test]
async fn running_process_arms_one_watcher_that_wakes_on_exit() {
    let mut fixture = Fixture::new();
    let process = fixture.start_process().await;

    fixture.prompt_ended().await;
    let calls = fixture.take_calls();
    let token = Fixture::delegated_signal(&calls[0].1["task"]);
    assert_eq!(
        calls,
        vec![(
            "delegate_task".to_owned(),
            json!({
                "task": format!("/rho-wake {token}"),
                "title": format!("waiting: {LONG_RUNNING_COMMAND}"),
                "mode": "async",
            }),
        )]
    );

    // A message while the process still runs keeps the same watcher.
    assert_eq!(
        fixture.wake.prompt_started(Some("how is it going?")),
        PromptOrigin::Other
    );
    fixture.prompt_ended().await;
    assert_eq!(fixture.take_calls(), Vec::new());

    fixture.stop_process(&process).await;
    fixture.notifier_settled().await;
    // A message that lands after the signal but before the watcher reads it
    // must leave the signal and the parent's lock in place.
    assert_eq!(
        fixture.wake.prompt_started(Some("still there?")),
        PromptOrigin::Other
    );
    let signal = WakeSignal::parse(&fixture.dir, &token).unwrap();
    let summary = tokio::time::timeout(
        TEST_WAIT,
        signal.wait(WATCHER_SESSION, &CancellationToken::new()),
    )
    .await
    .unwrap();
    let summary = summary.expect("the watcher receives the exit");
    assert!(
        summary.starts_with(LONG_RUNNING_COMMAND),
        "the watcher reports the process, not that its parent ended: {summary}"
    );

    assert_eq!(
        fixture.wake.prompt_started(Some(
            "Delegated task task-1 reached a terminal state. Use task_status with taskId \
             task-1 to read the result."
        )),
        PromptOrigin::WatcherNotice
    );
    fixture.processes().take_notifications();
    fixture.prompt_ended().await;
    assert_eq!(fixture.take_calls(), Vec::new(), "nothing left to watch");
}

// Covers: when the watched work finishes during a later prompt and is
// delivered there, that prompt's end cancels the watcher and keeps its lock,
// so it exits by cancel instead of reporting its parent ended, and neither
// lingers in T3's panel nor wakes the thread for nothing.
// Owner: daemon T3 wake watcher.
#[tokio::test]
async fn work_delivered_in_prompt_retires_the_watcher() {
    let mut fixture = Fixture::new();
    let process = fixture.start_process().await;
    fixture.prompt_ended().await;
    let token = Fixture::delegated_signal(&fixture.take_calls()[0].1["task"]);
    let signal = WakeSignal::parse(&fixture.dir, &token).unwrap();

    assert_eq!(fixture.wake.prompt_started(None), PromptOrigin::Other);
    fixture.stop_process(&process).await;
    fixture.processes().take_notifications();
    fixture.prompt_ended().await;

    assert_eq!(
        fixture.take_calls(),
        vec![("task_cancel".to_owned(), json!({"taskId": "task-1"}))]
    );

    // Later prompts end before T3 delivers the cancel; the watcher must
    // still not see its parent let go.
    assert_eq!(fixture.wake.prompt_started(None), PromptOrigin::Other);
    fixture.prompt_ended().await;
    assert!(
        !signal.parent_ended(),
        "the cancelled watcher's lock is held"
    );
}

// Covers: a workflow whose runtime reported completion but whose result is
// not yet published still counts as outstanding, so the prompt ending in
// that window arms a watcher instead of leaving the result unannounced.
// Owner: daemon T3 wake watcher.
#[tokio::test]
async fn workflow_completing_but_unpublished_arms_a_watcher() {
    let mut fixture = Fixture::new();
    let workflows = &fixture.background.workflows;
    assert!(workflows.register_start("wf-run", "review", "digest", Some("parent".into())));
    workflows.record_event(
        "wf-run",
        &crate::app::workflow_runtime::RuntimeEvent::Completed,
    );

    fixture.prompt_ended().await;

    assert_eq!(
        fixture
            .take_calls()
            .into_iter()
            .map(|(tool, _)| tool)
            .collect::<Vec<_>>(),
        vec!["delegate_task"]
    );
}

// Covers: a watcher that ends without this parent signalling it, because
// the user cancelled it in T3 or the parent session closed, is never
// reused: the next prompt end arms a fresh one, and a closed parent's
// watcher stops waiting. A reused or orphaned watcher never wakes the
// thread. Owner: daemon T3 wake watcher.
#[tokio::test]
async fn watcher_that_ended_unsignalled_is_replaced_or_released() {
    let mut fixture = Fixture::new();
    fixture.start_process().await;
    fixture.prompt_ended().await;
    let first = Fixture::delegated_signal(&fixture.take_calls()[0].1["task"]);

    assert_eq!(
        fixture.wake.prompt_started(Some(
            "Delegated tasks task-1, task-9 reached terminal states."
        )),
        PromptOrigin::WatcherNotice
    );
    fixture.prompt_ended().await;
    let second = Fixture::delegated_signal(&fixture.take_calls()[0].1["task"]);
    assert_ne!(first, second, "a fresh watcher replaces the ended one");

    let signal = WakeSignal::parse(&fixture.dir, &second).unwrap();
    let Fixture { wake, .. } = fixture;
    drop(wake);
    let result = tokio::time::timeout(
        TEST_WAIT,
        signal.wait(WATCHER_SESSION, &CancellationToken::new()),
    )
    .await
    .unwrap();
    assert_eq!(result.as_deref(), Some(PARENT_ENDED));
}

// Covers: a daemon stop or a cancel that interrupts a prompt's end gives up
// on a host that never answers delegate_task, instead of holding the turn,
// and keeps holding the lock of the watcher T3 may have started anyway. A
// hung host call blocks the turn's end, and so the session's next prompt and
// the daemon's stop; a released lock makes a started watcher report that
// its parent ended. Owner: daemon T3 wake watcher.
#[tokio::test]
async fn interrupted_arming_gives_up_on_a_hung_host() {
    let mut fixture = Fixture::with_delegate(Reply::Never);
    fixture.start_process().await;
    let interrupt = CancellationToken::new();
    interrupt.cancel();

    let armed = tokio::time::timeout(TEST_WAIT, fixture.prompt_ended_interrupted_by(&interrupt))
        .await
        .expect("an interrupted host call gives up");

    let locks: Vec<_> = std::fs::read_dir(fixture.dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    let held: Vec<_> = locks
        .iter()
        .map(|path| {
            (
                path.extension() == Some("lock".as_ref()),
                !signal::released(path),
            )
        })
        .collect();
    let ended: Vec<_> = fixture
        .wake
        .ended
        .iter()
        .map(|ended| ended.task_id.clone())
        .collect();
    assert_eq!(
        (armed.is_err(), fixture.wake.armed.is_none(), held, ended),
        (true, true, vec![(true, true)], vec![None])
    );
}

// Covers: T3's finish notice drops only the watcher it names, so `task-1`
// does not match inside `task-12`. A loose match drops a live watcher and
// releases its lock, and the watcher then reports that its parent ended.
// Owner: daemon T3 wake watcher.
#[test]
fn finish_notice_names_its_task_whole() {
    for (text, expected) in [
        ("task task-1 finished", true),
        ("Task \"task-1\" completed.", true),
        ("task-1", true),
        ("task task-12 finished", false),
        ("task subtask-1 finished", false),
        ("no task here", false),
    ] {
        assert_eq!(names_task(text, "task-1"), expected, "{text}");
    }
}

/// What a prompt's leading text asks of the wake watchers.
#[derive(Debug, PartialEq)]
enum Command {
    Wake(String),
    Rejected,
    NotWake,
}

// Covers: `/rho-wake <token>` starts a watcher's wait with the token
// normalised, a token that is not a UUID, which could name a path outside
// the wake directory, is rejected, and a longer word is left to the shared
// commands. The token is a path component: a loose match lets a prompt read
// or write any file. Owner: daemon T3 wake command.
#[test]
fn wake_command_matches_only_a_uuid_token() {
    let home = tempfile::tempdir().unwrap();
    let dir = WakeDir::in_home(home.path());
    let cases = [
        (
            "wake watcher",
            Some("/rho-wake 0F4E2C3A-9B1D-4E5F-8A7B-6C5D4E3F2A1B"),
            Command::Wake("0f4e2c3a-9b1d-4e5f-8a7b-6c5d4e3f2a1b".into()),
        ),
        (
            "wake token that could name a path",
            Some("/rho-wake ../config.toml"),
            Command::Rejected,
        ),
        (
            "wake prefix of a longer word",
            Some("/rho-wakeup now"),
            Command::NotWake,
        ),
        ("prompt without leading text", None, Command::NotWake),
    ];

    for (label, text, expected) in cases {
        let command = match wake_command(&dir, text) {
            Some(Ok(signal)) => Command::Wake(signal.token().to_owned()),
            Some(Err(_)) => Command::Rejected,
            None => Command::NotWake,
        };
        assert_eq!(command, expected, "{label}");
    }
}
