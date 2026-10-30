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
    /// Watchers no longer armed: fired, or cancelled. T3's notice that one
    /// finished may still be on its way as a later prompt.
    ended: Vec<EndedWatcher>,
}

struct Watcher {
    task_id: String,
    signal: WakeSignal,
    /// Held while this parent lives; the watcher ends once it is released.
    lock: std::fs::File,
    notifier: Option<Notifier>,
}

/// A watcher that fired or was cancelled. A fired one may not have read its
/// signal yet, and a cancelled one may not have seen its cancel, so its lock
/// stays held until T3 reports it finished or the session ends. Releasing
/// it earlier would make the watcher report that this parent ended.
struct EndedWatcher {
    task_id: String,
    _lock: std::fs::File,
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
                // A fired watcher consumes its signal itself.
                if !fired {
                    watcher.signal.remove();
                }
                self.ended.push(EndedWatcher {
                    task_id: watcher.task_id,
                    _lock: watcher.lock,
                });
            }
        }
        let before = self.ended.len();
        self.ended.retain(|ended| !mentions(&ended.task_id));
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
        if !background.outstanding() {
            if let Some(watcher) = self.armed.take() {
                self.ended.push(retire(tools, watcher).await);
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

/// Cancels a watcher whose work was delivered in-prompt, so it exits by
/// cancel rather than by a released lock. Best effort: a leftover watcher
/// only costs an idle child in T3's panel until this parent ends.
async fn retire(tools: &AppToolSet, mut watcher: Watcher) -> EndedWatcher {
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
    EndedWatcher {
        task_id: watcher.task_id,
        _lock: watcher.lock,
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

    /// The signal watcher session `session_id` waited on in an earlier
    /// process, if any. T3 Code reopens a watcher a restart cut off and
    /// sends it a prompt to continue, which must resume the wait, or repeat
    /// its result, instead of starting a model turn.
    pub(super) fn recorded(session_id: &str) -> Option<Self> {
        let dir = signal_dir().ok()?;
        let token = WaitRecord::of(&dir, session_id)?.read()?.token;
        Self::parse_in(&dir, &token)
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

    fn record(&self, session_id: &str) -> Option<WaitRecord> {
        WaitRecord::of(self.path.parent()?, session_id)
    }

    /// Waits, as watcher session `session_id`, for the parent's signal or
    /// for the parent to end, and cleans up, returning the watcher's result.
    ///
    /// The wait and then its result stay recorded under the session, so a
    /// watcher whose process ends mid-wait, or before T3 Code saw its
    /// result, resumes or repeats it on the next prompt (see
    /// [`Self::recorded`]). `None` when `cancel` trips first, leaving the
    /// wait recorded: the caller [abandons](Self::abandon) it unless the
    /// session is closing with its process.
    pub(super) async fn wait(
        &self,
        session_id: &str,
        cancel: &CancellationToken,
    ) -> Option<String> {
        let record = self.record(session_id);
        if let Some(record) = &record {
            if let Some(summary) = record.read().and_then(|recorded| {
                (recorded.token == self.token)
                    .then_some(recorded.summary)
                    .flatten()
            }) {
                return Some(summary);
            }
            record.prune_finished();
            // A watcher left unrecorded only loses restart recovery.
            let _ = record.write(&self.token, /*summary*/ None);
        }
        let summary = loop {
            if let Ok(summary) = std::fs::read_to_string(&self.path) {
                break summary;
            }
            if self.parent_ended() {
                break PARENT_ENDED.into();
            }
            tokio::select! {
                () = cancel.cancelled() => return None,
                () = tokio::time::sleep(SIGNAL_POLL) => {}
            }
        };
        // The signal and lock go only once the record holds the result: a
        // later process repeats it from either, and from neither would
        // report that the parent ended.
        let recorded = record
            .as_ref()
            .is_none_or(|record| record.write(&self.token, Some(&summary)));
        if recorded {
            self.cleanup();
        }
        Some(summary)
    }

    /// Drops the signal, the parent's lock file, and the wait session
    /// `session_id` recorded on them.
    pub(super) fn abandon(&self, session_id: &str) {
        self.cleanup();
        if let Some(record) = self.record(session_id) {
            record.remove();
        }
    }
}

/// How long a finished watcher's record outlives its result. T3 Code asks
/// a watcher to continue only when it restarted before it saw the result,
/// and it asks as it starts again, so the record matters until T3's next
/// start. A month covers a machine left off for weeks; past that a lost
/// result costs one model turn in the watcher, as before records existed.
const FINISHED_RECORD_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// One watcher session's file naming the signal it waits on, and, once the
/// wait ends, its result: the token on the first line, then the result.
struct WaitRecord {
    path: PathBuf,
}

struct RecordedWait {
    token: String,
    /// The result, once the wait ended.
    summary: Option<String>,
}

impl WaitRecord {
    /// `None` for a session id that is not a UUID, which Rho never creates,
    /// so a host's session id cannot name a path outside `dir`.
    fn of(dir: &Path, session_id: &str) -> Option<Self> {
        let session_id = uuid::Uuid::parse_str(session_id).ok()?;
        Some(Self {
            path: dir.join(format!("{session_id}.watcher")),
        })
    }

    fn read(&self) -> Option<RecordedWait> {
        std::fs::read_to_string(&self.path)
            .ok()
            .map(|contents| Self::parse(&contents))
    }

    fn parse(contents: &str) -> RecordedWait {
        let (token, summary) = match contents.split_once('\n') {
            Some((token, summary)) => (token, Some(summary.to_owned())),
            None => (contents, None),
        };
        RecordedWait {
            token: token.to_owned(),
            summary,
        }
    }

    /// Writes whole, through a temp file, so a crash never leaves a waiting
    /// record half turned into a finished one. Returns whether it wrote;
    /// a failure is logged.
    fn write(&self, token: &str, summary: Option<&str>) -> bool {
        let contents = match summary {
            Some(summary) => format!("{token}\n{summary}"),
            None => token.to_owned(),
        };
        let staging = self.path.with_extension("watcher-tmp");
        let written =
            std::fs::write(&staging, contents).and_then(|()| std::fs::rename(&staging, &self.path));
        if let Err(error) = &written {
            tracing::warn!(%error, "could not record the T3 wake watcher's wait");
        }
        written.is_ok()
    }

    fn remove(&self) {
        let _ = std::fs::remove_file(&self.path);
    }

    /// Removes finished records older than [`FINISHED_RECORD_TTL`] beside
    /// this one. Waiting records stay: their watcher can still resume.
    fn prune_finished(&self) {
        let Some(entries) = self
            .path
            .parent()
            .and_then(|dir| std::fs::read_dir(dir).ok())
        else {
            return;
        };
        for path in entries.flatten().map(|entry| entry.path()) {
            let expired = path
                .extension()
                .is_some_and(|extension| extension == "watcher")
                && std::fs::metadata(&path)
                    .and_then(|metadata| metadata.modified())
                    .is_ok_and(|modified| {
                        modified
                            .elapsed()
                            .is_ok_and(|age| age > FINISHED_RECORD_TTL)
                    })
                && std::fs::read_to_string(&path)
                    .is_ok_and(|contents| Self::parse(&contents).summary.is_some());
            if expired {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

#[cfg(test)]
#[path = "t3_wake_tests.rs"]
mod tests;
