use std::{path::Path, sync::Mutex, time::Duration};

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::ToolSpec,
    tool::{ToolError, ToolFuture, ToolOutput},
};

use super::*;
use crate::tools::{
    process::ProcessLimits,
    sdk_registry::{AppToolSet, StaticToolBundle},
};

#[cfg(unix)]
const LONG_RUNNING_COMMAND: &str = "sleep 300";
#[cfg(windows)]
const LONG_RUNNING_COMMAND: &str = "Start-Sleep -Seconds 300";

/// Bounds each wait on real process exits and signal polls; a hang fails.
const TEST_WAIT: Duration = Duration::from_secs(10);

type Calls = Arc<Mutex<Vec<(String, Value)>>>;

/// A `t3-code` task tool that records its calls and returns `result`.
struct T3Tool {
    name: &'static str,
    calls: Calls,
    result: Result<ToolOutput, ToolError>,
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
        Box::pin(std::future::ready(self.result.clone()))
    }
}

struct Fixture {
    signals: tempfile::TempDir,
    tools: AppToolSet,
    calls: Calls,
    background: Background,
    wake: T3Wake,
}

impl Fixture {
    fn new() -> Self {
        let calls = Calls::default();
        let mut tools = AppToolSet::disabled();
        tools.add_bundle(StaticToolBundle::new(
            [
                (
                    "delegate_task",
                    Ok(ToolOutput::text("delegated")
                        .with_structured_content(json!({"taskId": "task-1"}))),
                ),
                ("task_cancel", Ok(ToolOutput::text("cancel requested"))),
            ]
            .into_iter()
            .map(|(name, result)| {
                Arc::new(T3Tool {
                    name,
                    calls: calls.clone(),
                    result,
                }) as Arc<dyn Tool>
            })
            .collect(),
        ));
        let signals = tempfile::tempdir().unwrap();
        let wake = T3Wake::new(signals.path().to_path_buf());
        Self {
            signals,
            tools,
            calls,
            background: Background {
                session_id: "parent".into(),
                processes: Some(ProcessManager::new(ProcessLimits::default())),
                workflows: WorkflowRunTracker::new(),
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
        self.wake
            .prompt_ended(&self.tools, self.background.clone())
            .await
            .unwrap();
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
// it twice. Owner: ACP T3 wake watcher.
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
    let signal = WakeSignal::parse_in(fixture.signals.path(), &token).unwrap();
    let summary = tokio::time::timeout(TEST_WAIT, signal.wait(&CancellationToken::new()))
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
// delivered there, that prompt's end cancels the watcher, so it neither
// lingers in T3's panel nor wakes the thread for nothing.
// Owner: ACP T3 wake watcher.
#[tokio::test]
async fn work_delivered_in_prompt_retires_the_watcher() {
    let mut fixture = Fixture::new();
    let process = fixture.start_process().await;
    fixture.prompt_ended().await;
    fixture.take_calls();

    assert_eq!(fixture.wake.prompt_started(None), PromptOrigin::Other);
    fixture.stop_process(&process).await;
    fixture.processes().take_notifications();
    fixture.prompt_ended().await;

    assert_eq!(
        fixture.take_calls(),
        vec![("task_cancel".to_owned(), json!({"taskId": "task-1"}))]
    );
}

// Covers: a workflow whose runtime reported completion but whose result is
// not yet published still counts as outstanding, so the prompt ending in
// that window arms a watcher instead of leaving the result unannounced.
// Owner: ACP T3 wake watcher.
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
// thread. Owner: ACP T3 wake watcher.
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

    let signal = WakeSignal::parse_in(fixture.signals.path(), &second).unwrap();
    let Fixture { wake, .. } = fixture;
    drop(wake);
    let result = tokio::time::timeout(TEST_WAIT, signal.wait(&CancellationToken::new()))
        .await
        .unwrap();
    assert_eq!(result.as_deref(), Some(PARENT_ENDED));
}
