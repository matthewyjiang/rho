//! Wakes an idle T3 Code thread when background work finishes (`--t3code`).
//!
//! ACP gives an agent no way to start a turn, and T3 refuses a thread's own
//! writes, such as `t3_thread_send`, once its run ends. T3 does wake a parent
//! when a delegated task finishes, even after the parent's turn ended. So
//! when a prompt ends with background processes or workflows still running,
//! the host delegates one watcher task whose prompt is the unadvertised
//! `/rho-wake <token>` command. The watcher, a fresh Rho session, makes no
//! model call: it waits for the signal file this parent writes once a result
//! is ready, then ends its turn, and T3 wakes this thread. The result itself
//! stays here and reaches the model through the usual boundary delivery.
//!
//! Work that finishes during a prompt is delivered in that prompt, so a quick
//! `cargo check` never shows a watcher in T3's subagent panel.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use rho_sdk::{
    tool::{tool_progress_channel, Tool, ToolContext, ToolInvocation},
    CancellationToken, ToolCallId,
};
use serde_json::{json, Value};
use tokio::task::JoinHandle;

use crate::tools::{
    mcp::exported_name::namespaced_tool_name, process::ProcessManager, sdk_registry::AppToolSet,
    workflow_tracker::WorkflowRunTracker,
};

/// The watcher's prompt command. Not advertised, so hosts never offer it.
pub(super) const WAKE_COMMAND: &str = "/rho-wake";

const SERVER: &str = "t3-code";

/// How often a watcher checks for its signal. T3 takes seconds to start the
/// woken turn, so a half-second poll adds little latency and no real load.
const SIGNAL_POLL: Duration = Duration::from_millis(500);

/// Longest background label in a watcher title; T3's roster truncates anyway.
const TITLE_LABEL_CHARS: usize = 60;

/// The parent's watcher state, kept across prompts.
pub(super) struct T3Wake {
    /// Where this parent writes signals and its watchers look for them.
    signal_dir: PathBuf,
    armed: Option<Watcher>,
    /// Watchers that ended and were dropped: T3's notice that they finished
    /// may still be on its way as a later prompt.
    ended: Vec<String>,
    /// Locks of cancelled watchers, held until a later prompt ends so a
    /// watcher sees its cancel before it sees this parent let go.
    retired: Vec<std::fs::File>,
}

struct Watcher {
    task_id: String,
    signal: WakeSignal,
    /// Held while this parent lives; the watcher ends once it is released.
    lock: std::fs::File,
    notifier: Option<Notifier>,
}

/// Fires a watcher's signal once a result is ready, unless a prompt that
/// delivers results itself has started first.
struct Notifier {
    task: JoinHandle<()>,
    state: Arc<Mutex<NotifierState>>,
}

#[derive(Default)]
struct NotifierState {
    /// Set as a prompt starts. Publication checks it under the same lock, so
    /// once set no signal follows.
    quiet: bool,
    fired: bool,
}

impl Notifier {
    fn spawn(background: Background, signal: WakeSignal) -> Self {
        let state = Arc::new(Mutex::new(NotifierState::default()));
        let publish = Arc::clone(&state);
        let task = tokio::spawn(async move {
            background.clone().result_ready().await;
            let mut state = publish.lock().unwrap_or_else(|error| error.into_inner());
            if state.quiet {
                return;
            }
            match signal.fire(&finished_summary(&background)) {
                Ok(()) => state.fired = true,
                Err(error) => tracing::warn!(%error, "could not signal the T3 wake watcher"),
            }
        });
        Self { task, state }
    }

    /// Stops the notifier and reports whether it fired.
    fn quiet(self) -> bool {
        let fired = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.quiet = true;
            state.fired
        };
        self.task.abort();
        fired
    }
}

/// What a prompt was, as far as waking is concerned.
#[derive(Debug, PartialEq)]
pub(super) enum PromptOrigin {
    /// It mentions a watcher of this session, normally T3's notice that the
    /// watcher finished, so the model needs to know the watcher holds no
    /// result.
    WatcherNotice,
    Other,
}

/// The background work one session delivers.
#[derive(Clone)]
pub(super) struct Background {
    pub(super) session_id: String,
    pub(super) processes: Option<ProcessManager>,
    pub(super) workflows: WorkflowRunTracker,
}

impl Background {
    pub(super) fn of(tools: &AppToolSet, session_id: String) -> Self {
        Self {
            session_id,
            processes: tools.processes().cloned(),
            workflows: tools.workflow_tracker().clone(),
        }
    }

    /// Whether a result is waiting or could still arrive. A workflow counts
    /// until its terminal snapshot is published, which follows its last
    /// runtime event.
    fn outstanding(&self) -> bool {
        self.processes.as_ref().is_some_and(|processes| {
            processes.has_pending_notification()
                || processes
                    .live_summaries()
                    .iter()
                    .any(|summary| summary.state.is_live())
        }) || self
            .workflows
            .has_active_or_pending_notification(&self.session_id)
    }

    /// Labels for work still running, oldest first, for the watcher title.
    fn live_labels(&self) -> Vec<String> {
        let processes = self
            .processes
            .iter()
            .flat_map(ProcessManager::live_summaries)
            .filter(|summary| summary.state.is_live())
            .map(|summary| summary.command);
        let workflows = self
            .workflows
            .rail_summaries(&self.session_id)
            .into_iter()
            .filter(|summary| summary.is_live())
            .map(|summary| format!("workflow {}", summary.workflow_name));
        processes.chain(workflows).collect()
    }

    /// Whether a finished result is waiting for the model.
    fn has_result(&self) -> bool {
        self.processes
            .as_ref()
            .is_some_and(ProcessManager::has_pending_notification)
            || self.workflows.has_pending_notification(&self.session_id)
    }

    /// Resolves once a result is waiting.
    async fn result_ready(self) {
        loop {
            // Subscribe before checking so a finish in between still wakes.
            let process_exit = self.processes.as_ref().map(ProcessManager::notified_owned);
            let workflow_finish = self.workflows.notified_owned();
            if self.has_result() {
                return;
            }
            let process_exit = async move {
                match process_exit {
                    Some(exit) => exit.await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                () = process_exit => {}
                () = workflow_finish => {}
            }
        }
    }
}

impl T3Wake {
    pub(super) fn new(signal_dir: PathBuf) -> Self {
        Self {
            signal_dir,
            armed: None,
            ended: Vec::new(),
            retired: Vec::new(),
        }
    }

    /// Call as a prompt starts, before anything it awaits, with its leading
    /// text. The prompt delivers results itself, so the armed watcher's
    /// notifier stops until the prompt ends. A watcher that fired, or that
    /// this prompt reports as finished, is dropped: T3 already holds its
    /// wake.
    pub(super) fn prompt_started(&mut self, text: Option<&str>) -> PromptOrigin {
        let mentions = |id: &str| text.is_some_and(|text| text.contains(id));
        if let Some(watcher) = &mut self.armed {
            let fired = watcher.notifier.take().is_some_and(Notifier::quiet);
            if fired || mentions(&watcher.task_id) {
                let watcher = self.armed.take().expect("armed watcher");
                watcher.signal.remove();
                self.ended.push(watcher.task_id);
            }
        }
        let before = self.ended.len();
        self.ended.retain(|id| !mentions(id));
        if self.ended.len() < before {
            PromptOrigin::WatcherNotice
        } else {
            PromptOrigin::Other
        }
    }

    /// Call as a prompt ends. Arms a watcher while work is outstanding, and
    /// retires one that has nothing left to wait for. Fails when T3 refused
    /// to start the watcher, so the host can say that results wait for the
    /// next message.
    pub(super) async fn prompt_ended(
        &mut self,
        tools: &AppToolSet,
        background: Background,
    ) -> anyhow::Result<()> {
        self.retired.clear();
        if !background.outstanding() {
            if let Some(watcher) = self.armed.take() {
                self.retired.push(retire(tools, watcher).await);
            }
            return Ok(());
        }
        if self.armed.is_none() {
            let labels = background.live_labels();
            self.armed = Some(arm(tools, &self.signal_dir, &labels).await?);
        }
        let watcher = self.armed.as_mut().expect("armed watcher");
        watcher.notifier = Some(Notifier::spawn(background, watcher.signal.clone()));
        Ok(())
    }
}

/// Dropping the state, as closing or replacing the session does, releases
/// every watcher's lock, so they end instead of waiting forever.
impl Drop for T3Wake {
    fn drop(&mut self) {
        if let Some(watcher) = &mut self.armed {
            if let Some(notifier) = watcher.notifier.take() {
                notifier.quiet();
            }
        }
    }
}

/// Delegates a watcher titled after the running work.
async fn arm(tools: &AppToolSet, signal_dir: &Path, labels: &[String]) -> anyhow::Result<Watcher> {
    let delegate = t3_tool(tools, "delegate_task")?;
    let (signal, lock) = WakeSignal::create(signal_dir)?;
    let arguments = json!({
        "task": format!("{WAKE_COMMAND} {}", signal.token),
        "title": watcher_title(labels),
        "mode": "async",
    });
    let task_id = delegate
        .call(invocation(arguments), host_context())
        .await
        .map_err(|error| anyhow::anyhow!("delegate_task failed: {}", error.message()))
        .and_then(|output| {
            if output.is_failure() {
                anyhow::bail!("delegate_task failed: {}", output.content());
            }
            output
                .structured_content()
                .and_then(|data| data.get("taskId"))
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| anyhow::anyhow!("delegate_task returned no taskId"))
        });
    let task_id = match task_id {
        Ok(task_id) => task_id,
        Err(error) => {
            signal.cleanup();
            return Err(error);
        }
    };
    Ok(Watcher {
        task_id,
        signal,
        lock,
        notifier: None,
    })
}

/// Cancels a watcher whose work was delivered in-prompt, returning its lock
/// for the caller to hold a while longer. Best effort: a leftover watcher
/// only costs an idle child in T3's panel until this parent ends.
async fn retire(tools: &AppToolSet, mut watcher: Watcher) -> std::fs::File {
    if let Some(notifier) = watcher.notifier.take() {
        notifier.quiet();
    }
    watcher.signal.remove();
    let cancelled = match t3_tool(tools, "task_cancel") {
        Ok(cancel) => cancel
            .call(
                invocation(json!({"taskId": watcher.task_id})),
                host_context(),
            )
            .await
            .map_err(|error| error.message().to_owned())
            .and_then(|output| {
                if output.is_failure() {
                    Err(output.content().to_owned())
                } else {
                    Ok(())
                }
            }),
        Err(error) => Err(error.to_string()),
    };
    if let Err(error) = cancelled {
        tracing::warn!(%error, task_id = %watcher.task_id, "could not cancel T3 wake watcher");
    }
    watcher.lock
}

fn t3_tool(tools: &AppToolSet, name: &str) -> anyhow::Result<Arc<dyn Tool>> {
    tools
        .tool_named(&namespaced_tool_name(SERVER, name))
        .ok_or_else(|| anyhow::anyhow!("MCP server `{SERVER}` provides no `{name}` tool"))
}

fn invocation(arguments: Value) -> ToolInvocation {
    let id = ToolCallId::from_string(format!("t3-wake-{}", uuid::Uuid::new_v4()))
        .expect("uuid call ids are valid");
    ToolInvocation::new(id, arguments)
}

/// A host-originated context: these calls belong to no model turn.
fn host_context() -> ToolContext {
    let (progress, _receiver) = tool_progress_channel(std::num::NonZeroUsize::MIN);
    ToolContext::new(/*workspace*/ None, CancellationToken::new(), progress)
}

/// `waiting: <oldest label> (+N more)`.
fn watcher_title(labels: &[String]) -> String {
    let Some(first) = labels.first() else {
        return "waiting: background results".into();
    };
    let first = first.lines().next().unwrap_or_default().trim();
    let mut label: String = first.chars().take(TITLE_LABEL_CHARS).collect();
    if label.len() < first.len() {
        label.push('…');
    }
    match labels.len() {
        1 => format!("waiting: {label}"),
        count => format!("waiting: {label} (+{} more)", count - 1),
    }
}

/// The watcher's final message, shown as its result in T3's panel.
fn finished_summary(background: &Background) -> String {
    let process = background.processes.iter().find_map(|processes| {
        processes
            .live_summaries()
            .into_iter()
            .find(|summary| !summary.state.is_live())
    });
    match process {
        Some(summary) => match summary.exit_code {
            Some(code) => format!("{} exited {code}", summary.command),
            None => format!("{} finished", summary.command),
        },
        None => "background work finished".into(),
    }
}

/// Signals live under the Rho home, which a parent and the watcher T3
/// starts for it share.
pub(super) fn signal_dir() -> anyhow::Result<PathBuf> {
    Ok(crate::paths::rho_dir()?.join("acp-wakes"))
}

/// The watcher's result when its parent ended before any result was ready.
const PARENT_ENDED: &str = "the Rho session that started this background work ended before it \
                            finished";

/// The files one watcher shares with its parent: the signal the parent
/// writes when a result is ready, and a lock the parent holds while it
/// lives. The token is a UUID, so a watcher prompt cannot name a path
/// outside the signal directory.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct WakeSignal {
    token: String,
    path: PathBuf,
}

impl WakeSignal {
    /// A fresh signal, with the parent's lock on it held by the returned file.
    fn create(dir: &Path) -> anyhow::Result<(Self, std::fs::File)> {
        std::fs::create_dir_all(dir)?;
        let token = uuid::Uuid::new_v4().to_string();
        let signal = Self {
            path: dir.join(&token),
            token,
        };
        let lock = std::fs::File::create(signal.lock_path())?;
        lock.lock()?;
        Ok((signal, lock))
    }

    /// The signal a `/rho-wake` prompt names, if its token is well formed.
    pub(super) fn parse(token: &str) -> Option<Self> {
        Self::parse_in(&signal_dir().ok()?, token)
    }

    fn parse_in(dir: &Path, token: &str) -> Option<Self> {
        let token = uuid::Uuid::parse_str(token.trim()).ok()?.to_string();
        Some(Self {
            path: dir.join(&token),
            token,
        })
    }

    #[cfg(test)]
    pub(super) fn token(&self) -> &str {
        &self.token
    }

    fn lock_path(&self) -> PathBuf {
        self.path.with_extension("lock")
    }

    /// Writes `summary` whole: a temp file renamed into place, so a polling
    /// watcher never reads a partial file.
    fn fire(&self, summary: &str) -> std::io::Result<()> {
        let staging = self.path.with_extension("tmp");
        std::fs::write(&staging, summary)?;
        std::fs::rename(staging, &self.path)
    }

    fn remove(&self) {
        let _ = std::fs::remove_file(&self.path);
    }

    fn cleanup(&self) {
        self.remove();
        let _ = std::fs::remove_file(self.lock_path());
    }

    /// Whether the parent let go of its lock: it ended, or closed or
    /// replaced its session. The OS releases a dead process's lock.
    fn parent_ended(&self) -> bool {
        match std::fs::File::open(self.lock_path()) {
            Ok(lock) => lock.try_lock().is_ok(),
            Err(error) => error.kind() == std::io::ErrorKind::NotFound,
        }
    }

    /// Waits for the parent's signal, or for the parent to end, and cleans
    /// up, returning the watcher's result. `None` when `cancel` trips first.
    pub(super) async fn wait(&self, cancel: &CancellationToken) -> Option<String> {
        loop {
            if let Ok(summary) = std::fs::read_to_string(&self.path) {
                self.cleanup();
                return Some(summary);
            }
            if self.parent_ended() {
                self.cleanup();
                return Some(PARENT_ENDED.into());
            }
            tokio::select! {
                () = cancel.cancelled() => {
                    self.cleanup();
                    return None;
                }
                () = tokio::time::sleep(SIGNAL_POLL) => {}
            }
        }
    }
}

#[cfg(test)]
#[path = "t3_wake_tests.rs"]
mod tests;
