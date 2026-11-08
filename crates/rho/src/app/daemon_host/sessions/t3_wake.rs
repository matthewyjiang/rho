//! Wakes an idle T3 Code thread when background work finishes (sessions
//! whose children run as the host's threads, which `rho acp --t3code` asks
//! for).
//!
//! The daemon never starts a turn by itself, and T3 refuses a thread's own
//! writes, such as `t3_thread_send`, once its run ends. T3 does wake a
//! parent when a delegated task finishes, even after the parent's turn
//! ended. So when a turn ends with background processes or workflows still
//! running, the session delegates one watcher task through the host whose
//! prompt is the unadvertised `/rho-wake <token>` command. The watcher, a
//! fresh daemon session T3 opens, makes no model call: it waits for the
//! signal file this parent writes once a result is ready (see [`signal`]),
//! then ends its turn, and T3 wakes this thread with a prompt. The result
//! itself stays here and reaches the model through the usual boundary
//! delivery.
//!
//! Work that finishes during a turn is delivered in that turn, so a quick
//! `cargo check` never shows a watcher in T3's subagent panel.
//!
//! The parent's lock is held by the daemon, so a watcher sees its parent
//! end once the parent session is unloaded or deleted, or the daemon dies.

use std::{
    future::Future,
    num::NonZeroUsize,
    sync::{Arc, Mutex},
};

use rho_sdk::{
    tool::{tool_progress_channel, ToolContext, ToolError, ToolErrorKind},
    CancellationToken, ToolCallId,
};
use tokio::task::JoinHandle;

use crate::{
    app::session_runtime::{BackgroundWait, CommandRejected, SessionRuntime, SessionTools},
    tools::{agent::HostContract, process::ProcessManager, workflow_tracker::WorkflowRunTracker},
};

mod signal;

pub(super) use signal::{WakeDir, WakeSignal};

/// The watcher's prompt command. Not advertised, so hosts never offer it.
const WAKE_COMMAND: &str = "/rho-wake";

/// Longest background label in a watcher title; T3's roster truncates anyway.
const TITLE_LABEL_CHARS: usize = 60;

/// How many ended watchers a parent keeps the locks of; past that the
/// oldest goes. Tripwire, not measured: T3 delivers a cancel, or its notice
/// that a watcher finished, within a turn or two of the parent, so older
/// entries are watchers T3 already finished, whose locks only hold file
/// descriptors. Dropping one early only lets a watcher T3 never cancelled
/// report that its parent ended.
const ENDED_WATCHERS_KEPT: usize = 16;

/// The parent's watcher state, kept across turns.
pub(super) struct T3Wake {
    /// Where this parent writes signals and its watchers look for them.
    dir: WakeDir,
    armed: Option<Watcher>,
    /// Watchers no longer armed, oldest first: fired, or cancelled. T3's
    /// notice that one finished may still be on its way as a later prompt.
    /// At most [`ENDED_WATCHERS_KEPT`].
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
/// stays held until T3 reports it finished, the session ends, or
/// [`ENDED_WATCHERS_KEPT`] later watchers ended. Releasing it earlier would
/// make the watcher report that this parent ended.
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
    session_id: String,
    processes: Option<ProcessManager>,
    workflows: WorkflowRunTracker,
}

impl Background {
    pub(super) fn of(tools: SessionTools<'_>, session_id: String) -> Self {
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
    /// The wake state of the session `runtime` runs, with its files in
    /// `dir`, when its host wakes it.
    pub(super) fn of(runtime: &SessionRuntime, dir: &WakeDir) -> Option<Self> {
        match runtime.background_wait() {
            BackgroundWait::HoldPrompt => None,
            BackgroundWait::HostWakes => Some(Self::new(dir.clone())),
        }
    }

    fn new(dir: WakeDir) -> Self {
        Self {
            dir,
            armed: None,
            ended: Vec::new(),
        }
    }

    /// Where this session's watchers wait, and, as a watcher, it records
    /// its own wait.
    pub(super) fn dir(&self) -> &WakeDir {
        &self.dir
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
                self.end(EndedWatcher {
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

    /// Call as a prompt's turn ends, before its end is told: T3 refuses a
    /// thread's writes once its run ended. Arms a watcher while work is
    /// outstanding, and retires one that has nothing left to wait for. The
    /// host calls give up once `interrupt` trips. Fails when T3 refused to
    /// start the watcher, or the call was interrupted, so the session can
    /// say that results wait for the next message.
    pub(super) async fn prompt_ended(
        &mut self,
        tools: SessionTools<'_>,
        background: Background,
        interrupt: &CancellationToken,
    ) -> anyhow::Result<()> {
        if !background.outstanding() {
            if let Some(watcher) = self.armed.take() {
                let ended = retire(tools, watcher, interrupt).await;
                self.end(ended);
            }
            return Ok(());
        }
        if self.armed.is_none() {
            let labels = background.live_labels();
            self.armed = Some(arm(tools, &self.dir, &labels, interrupt).await?);
        }
        let watcher = self.armed.as_mut().expect("armed watcher");
        let notifier = Notifier::spawn(background, watcher.signal.clone());
        if let Some(old) = watcher.notifier.replace(notifier) {
            old.quiet();
        }
        Ok(())
    }

    /// Keeps `watcher`'s lock until T3 reports it finished, dropping the
    /// oldest ended watcher past [`ENDED_WATCHERS_KEPT`].
    fn end(&mut self, watcher: EndedWatcher) {
        if self.ended.len() == ENDED_WATCHERS_KEPT {
            let oldest = self.ended.remove(0);
            tracing::info!(
                task_id = %oldest.task_id,
                limit = ENDED_WATCHERS_KEPT,
                "releasing the oldest ended T3 wake watcher past ENDED_WATCHERS_KEPT"
            );
        }
        self.ended.push(watcher);
    }
}

/// Dropping the state, as unloading or deleting the session does, releases
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

/// The watcher's wait a prompt whose leading text is `text` asks for, as
/// `/rho-wake <token>` with its signal in `dir`: `None` when the prompt is
/// not the command, rejected when the token is malformed.
pub(super) fn wake_command(
    dir: &WakeDir,
    text: Option<&str>,
) -> Option<Result<WakeSignal, CommandRejected>> {
    let token = text?
        .strip_prefix(WAKE_COMMAND)
        .filter(|rest| rest.starts_with(char::is_whitespace))?;
    Some(
        WakeSignal::parse(dir, token)
            .ok_or_else(|| CommandRejected(format!("usage: {WAKE_COMMAND} <token>"))),
    )
}

/// Delegates a watcher titled after the running work, its signal in `dir`.
async fn arm(
    tools: SessionTools<'_>,
    dir: &WakeDir,
    labels: &[String],
    interrupt: &CancellationToken,
) -> anyhow::Result<Watcher> {
    let host = host_tasks(tools)?;
    let (signal, lock) = WakeSignal::create(dir)?;
    let task = format!("{WAKE_COMMAND} {}", signal.token());
    let title = watcher_title(labels);
    let delegated = host_call(interrupt, |context| {
        host.delegate(call_id(), &task, Some(title.as_str()), context)
    })
    .await;
    let task_id = match delegated {
        Ok(task) => task.task_id,
        Err(error) => {
            signal.cleanup();
            return Err(anyhow::Error::msg(error.message().to_owned()));
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
async fn retire(
    tools: SessionTools<'_>,
    mut watcher: Watcher,
    interrupt: &CancellationToken,
) -> EndedWatcher {
    if let Some(notifier) = watcher.notifier.take() {
        notifier.quiet();
    }
    watcher.signal.remove();
    let cancelled = match host_tasks(tools) {
        Ok(host) => host_call(interrupt, |context| {
            host.cancel(call_id(), &watcher.task_id, /*reason*/ None, context)
        })
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

/// The host's task tools, bound from the session's `t3-code` MCP server.
fn host_tasks(tools: SessionTools<'_>) -> anyhow::Result<HostContract> {
    HostContract::bind(|name| tools.tool_named(name)).map_err(anyhow::Error::msg)
}

fn call_id() -> ToolCallId {
    ToolCallId::from_string(format!("t3-wake-{}", uuid::Uuid::new_v4()))
        .expect("uuid call ids are valid")
}

/// Runs the host call `call` makes with a host-originated context, as these
/// calls belong to no model turn, whose cancellation is `interrupt`. Gives
/// up once `interrupt` trips, even when the tool does not watch its context.
async fn host_call<T, F>(
    interrupt: &CancellationToken,
    call: impl FnOnce(ToolContext) -> F,
) -> Result<T, ToolError>
where
    F: Future<Output = Result<T, ToolError>>,
{
    // Nobody reads progress: the receiver goes now, so a send fails at once
    // instead of waiting on a full channel.
    let (progress, _) = tool_progress_channel(NonZeroUsize::MIN);
    let call = call(ToolContext::new(
        /*workspace*/ None,
        interrupt.clone(),
        progress,
    ));
    tokio::select! {
        biased;
        () = interrupt.cancelled() => Err(ToolError::new(
            ToolErrorKind::Cancelled,
            "interrupted: the turn was cancelled or the daemon is stopping",
        )),
        result = call => result,
    }
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

#[cfg(test)]
#[path = "t3_wake_tests.rs"]
mod tests;
