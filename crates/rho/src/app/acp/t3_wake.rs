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
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
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
    /// Watchers that fired and were dropped: their T3 wake notice may still
    /// be on its way as a later prompt.
    fired: Vec<String>,
}

struct Watcher {
    task_id: String,
    signal: WakeSignal,
    /// Set once the signal is written. The watcher consumes the file, so
    /// its absence says nothing.
    fired: Arc<AtomicBool>,
    notifier: Option<JoinHandle<()>>,
}

/// What a prompt was, as far as waking is concerned.
#[derive(Debug, PartialEq)]
pub(super) enum PromptOrigin {
    /// T3's notice that a watcher finished: background results are ready.
    Wake,
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

    /// Labels for work still running, oldest first.
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
            fired: Vec::new(),
        }
    }

    /// Call as a prompt starts, with its leading text. The prompt delivers
    /// results itself, so the armed watcher's notifier stops until the
    /// prompt ends. A watcher that already fired is dropped: its wake is
    /// this prompt or a later one.
    pub(super) fn prompt_started(&mut self, text: Option<&str>) -> PromptOrigin {
        if let Some(watcher) = &mut self.armed {
            if let Some(notifier) = watcher.notifier.take() {
                notifier.abort();
            }
            if watcher.fired.load(Ordering::Acquire) {
                let watcher = self.armed.take().expect("armed watcher");
                self.fired.push(watcher.task_id);
            }
        }
        let Some(text) = text else {
            return PromptOrigin::Other;
        };
        match self.fired.iter().position(|id| text.contains(id.as_str())) {
            Some(index) => {
                self.fired.remove(index);
                PromptOrigin::Wake
            }
            None => PromptOrigin::Other,
        }
    }

    /// Call as a prompt ends. Arms a watcher while work is still running or
    /// a result is undelivered, and retires one that has nothing to wait for.
    /// Fails when T3 refused to start the watcher, so the host can say that
    /// results wait for the next message.
    pub(super) async fn prompt_ended(
        &mut self,
        tools: &AppToolSet,
        background: Background,
    ) -> anyhow::Result<()> {
        let labels = background.live_labels();
        if labels.is_empty() && !background.has_result() {
            if let Some(watcher) = self.armed.take() {
                retire(tools, watcher).await;
            }
            return Ok(());
        }
        if self.armed.is_none() {
            self.armed = Some(arm(tools, &self.signal_dir, &labels).await?);
        }
        let watcher = self.armed.as_mut().expect("armed watcher");
        let signal = watcher.signal.clone();
        let fired = Arc::clone(&watcher.fired);
        watcher.notifier = Some(tokio::spawn(async move {
            background.clone().result_ready().await;
            // Marked first, so whoever sees the signal file sees the mark.
            fired.store(true, Ordering::Release);
            if let Err(error) = signal.fire(&finished_summary(&background)) {
                fired.store(false, Ordering::Release);
                tracing::warn!(%error, "could not signal the T3 wake watcher");
            }
        }));
        Ok(())
    }
}

impl Drop for T3Wake {
    fn drop(&mut self) {
        if let Some(watcher) = &mut self.armed {
            if let Some(notifier) = watcher.notifier.take() {
                notifier.abort();
            }
        }
    }
}

/// Delegates a watcher titled after the running work.
async fn arm(tools: &AppToolSet, signal_dir: &Path, labels: &[String]) -> anyhow::Result<Watcher> {
    let delegate = t3_tool(tools, "delegate_task")?;
    let signal = WakeSignal::create(signal_dir)?;
    let arguments = json!({
        "task": format!("{WAKE_COMMAND} {}", signal.token),
        "title": watcher_title(labels),
        "mode": "async",
    });
    let output = delegate
        .call(invocation(arguments), host_context())
        .await
        .map_err(|error| anyhow::anyhow!("delegate_task failed: {}", error.message()))?;
    if output.is_failure() {
        anyhow::bail!("delegate_task failed: {}", output.content());
    }
    let task_id = output
        .structured_content()
        .and_then(|data| data.get("taskId"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| anyhow::anyhow!("delegate_task returned no taskId"))?;
    Ok(Watcher {
        task_id: task_id.to_owned(),
        signal,
        fired: Arc::new(AtomicBool::new(false)),
        notifier: None,
    })
}

/// Cancels a watcher whose work was delivered in-prompt. Best effort: a
/// leftover watcher only costs an idle child in T3's panel.
async fn retire(tools: &AppToolSet, mut watcher: Watcher) {
    if let Some(notifier) = watcher.notifier.take() {
        notifier.abort();
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

/// The file a parent writes to end one watcher. The token is a UUID, so a
/// watcher prompt cannot name a path outside the signal directory.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct WakeSignal {
    token: String,
    path: PathBuf,
}

impl WakeSignal {
    fn create(dir: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let token = uuid::Uuid::new_v4().to_string();
        Ok(Self {
            path: dir.join(&token),
            token,
        })
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

    /// Waits for the parent's signal and consumes it, returning its summary.
    /// `None` when `cancel` trips first.
    pub(super) async fn wait(&self, cancel: &CancellationToken) -> Option<String> {
        loop {
            if let Ok(summary) = std::fs::read_to_string(&self.path) {
                self.remove();
                return Some(summary);
            }
            tokio::select! {
                () = cancel.cancelled() => return None,
                () = tokio::time::sleep(SIGNAL_POLL) => {}
            }
        }
    }
}

#[cfg(test)]
#[path = "t3_wake_tests.rs"]
mod tests;
