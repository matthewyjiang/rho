//! One owner for the interactive TUI's background tasks.
//!
//! A feature spawns a task with a [`TaskId`] and a wrapper that turns the
//! task's result (or join error) into a [`TaskOutput`]. The event loops only
//! ask [`BackgroundTasks`] whether anything is pending or finished, apply
//! finished outputs through one dispatch point, and cancel everything on
//! shutdown. Result handling stays in each feature's module.
//!
//! Adding a task: add a [`TaskId`] variant, a [`UiOutput`] or
//! [`SessionOutput`] variant, and its arm in the matching `apply_*` below.
//! `Ui` outputs apply in every loop (idle, running turn, goal waits);
//! `Session` outputs may change the session or runtime and apply only in the
//! idle loop.

use futures_util::{future::BoxFuture, FutureExt};
use rho_providers::model::{ModelMetadata, ReasoningRequestSource};
use rho_sdk::ReasoningLevel;
use tokio::task::{AbortHandle, JoinError, JoinHandle};

use super::{
    changelog_command::ChangelogFetchResult, github_pr::GithubPrLookup,
    limits_command::LimitsFetchResult, limits_command::LimitsSectionId, App, DefaultTerminal,
    InteractiveRuntime,
};
use crate::doctor::{DoctorProbeId, DoctorProbeOutcome};

#[cfg(test)]
#[path = "background_tasks_tests.rs"]
mod tests;

/// Identity of a registered task, used to dedupe starts and cancel by feature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TaskId {
    ModelMetadata,
    UpdateNotice,
    CustomModels,
    CursorModels,
    SyntaxWarmup,
    HerdrGraphics,
    InteractiveLogin,
    GithubPr,
    UsageLimits(LimitsSectionId),
    DoctorProbe(DoctorProbeId),
    InfoRuntimes,
    InfoTree,
    SpendLoad,
    /// Patch load for the `/diff` file at this index.
    DiffPatch(usize),
    Changelog,
    WebSearchTest,
}

/// A finished task's result, routed to its feature by the dispatch point.
pub(super) enum TaskOutput {
    Ui(UiOutput),
    Session(SessionOutput),
}

/// Results that only touch overlays and chrome, safe during a running turn.
pub(super) enum UiOutput {
    GithubPr(Result<GithubPrLookup, JoinError>),
    UsageLimits(LimitsSectionId, LimitsFetchResult),
    DoctorProbe(DoctorProbeOutcome),
    InfoRuntimes(Result<Vec<String>, JoinError>),
    InfoTree(Result<anyhow::Result<crate::session::tree::SessionTreeFacts>, JoinError>),
    SpendLoad(super::spend_overlay::LoadResult),
    DiffPatch(usize, anyhow::Result<Vec<rho_tools::tool_card::DiffRow>>),
    Changelog(Result<ChangelogFetchResult, JoinError>),
    WebSearchTest(Result<Result<usize, rho_tools::tool::ToolError>, JoinError>),
}

/// Results that may change the session or runtime; applied only when idle.
pub(super) enum SessionOutput {
    ModelMetadata {
        /// Reasoning selection when the fetch started, so a user change made
        /// while it ran is not overwritten.
        reasoning_at_start: (ReasoningLevel, ReasoningRequestSource),
        metadata: Result<Option<ModelMetadata>, JoinError>,
    },
    UpdateNotice(Result<Option<String>, JoinError>),
    CustomModels,
    CursorModels(Result<crate::cursor_runtime::models::RefreshResult, JoinError>),
    SyntaxWarmup,
    HerdrGraphics(Result<crate::herdr::HerdrGraphicsCapability, JoinError>),
    InteractiveLogin(super::login::FinishedInteractiveLogin),
}

impl From<UiOutput> for TaskOutput {
    fn from(output: UiOutput) -> Self {
        Self::Ui(output)
    }
}

impl From<SessionOutput> for TaskOutput {
    fn from(output: SessionOutput) -> Self {
        Self::Session(output)
    }
}

/// What cancellation does with a running task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OnCancel {
    /// Abort and wait until the task has stopped, so its resources are gone.
    Await,
    /// Abort without waiting. `spawn_blocking` work ignores abort once it
    /// runs; waiting on it would stall the event loop, so it finishes
    /// unobserved.
    Detach,
    /// Never abort; stop observing and let it finish. For work that must not
    /// stop halfway, such as writing a cache file.
    RunToCompletion,
}

struct RunningTask {
    id: TaskId,
    abort: AbortHandle,
    on_cancel: OnCancel,
    /// The join handle mapped through the feature's wrapper.
    output: BoxFuture<'static, TaskOutput>,
}

/// Background tasks owned by the interactive app. See the module docs.
#[derive(Default)]
pub(super) struct BackgroundTasks {
    running: Vec<RunningTask>,
    /// Finished outputs not yet applied, for example model metadata held
    /// while the session is busy.
    ready: Vec<(TaskId, TaskOutput)>,
}

impl BackgroundTasks {
    /// Adopt the tasks bootstrap started before the app existed.
    pub(super) fn from_startup(services: &mut super::ApplicationServices) -> Self {
        let mut tasks = Self::default();
        if let Some(handle) = services.pending_update_notice.take() {
            tasks.track(TaskId::UpdateNotice, handle, OnCancel::Await, |result| {
                SessionOutput::UpdateNotice(result).into()
            });
        }
        if let Some(handle) = services.pending_custom_models.take() {
            tasks.track(
                TaskId::CustomModels,
                handle,
                OnCancel::RunToCompletion,
                |_| SessionOutput::CustomModels.into(),
            );
        }
        if let Some(handle) = services.pending_syntax_warmup.take() {
            tasks.track(TaskId::SyntaxWarmup, handle, OnCancel::Detach, |_| {
                SessionOutput::SyntaxWarmup.into()
            });
        }
        tasks
    }

    pub(super) fn spawn<T: Send + 'static>(
        &mut self,
        id: TaskId,
        future: impl std::future::Future<Output = T> + Send + 'static,
        wrap: impl FnOnce(Result<T, JoinError>) -> TaskOutput + Send + 'static,
    ) {
        self.track(id, tokio::spawn(future), OnCancel::Await, wrap);
    }

    pub(super) fn spawn_blocking<T: Send + 'static>(
        &mut self,
        id: TaskId,
        work: impl FnOnce() -> T + Send + 'static,
        wrap: impl FnOnce(Result<T, JoinError>) -> TaskOutput + Send + 'static,
    ) {
        self.track(
            id,
            tokio::task::spawn_blocking(work),
            OnCancel::Detach,
            wrap,
        );
    }

    /// Register a task spawned elsewhere, such as startup work.
    pub(super) fn track<T: Send + 'static>(
        &mut self,
        id: TaskId,
        handle: JoinHandle<T>,
        on_cancel: OnCancel,
        wrap: impl FnOnce(Result<T, JoinError>) -> TaskOutput + Send + 'static,
    ) {
        self.running.push(RunningTask {
            id,
            abort: handle.abort_handle(),
            on_cancel,
            output: async move { wrap(handle.await) }.boxed(),
        });
    }

    /// Ids of running tasks and of finished outputs not yet applied.
    pub(super) fn ids(&self) -> impl Iterator<Item = &TaskId> {
        self.running
            .iter()
            .map(|task| &task.id)
            .chain(self.ready.iter().map(|(id, _)| id))
    }

    pub(super) fn contains(&self, matches: impl Fn(&TaskId) -> bool) -> bool {
        self.ids().any(matches)
    }

    /// Whether any task is running or waiting to be applied. The event loop
    /// uses fast ticks while this holds.
    pub(super) fn has_pending(&self) -> bool {
        !self.running.is_empty() || !self.ready.is_empty()
    }

    /// Whether a finished output is waiting for the dispatch point.
    pub(super) fn has_finished(&self) -> bool {
        !self.ready.is_empty() || self.running.iter().any(|task| task.abort.is_finished())
    }

    /// Take finished outputs that `select` accepts (`Ok`); rejected outputs
    /// (`Err`) stay queued for a later call.
    pub(super) fn take_finished<U>(
        &mut self,
        mut select: impl FnMut(TaskOutput) -> Result<U, TaskOutput>,
    ) -> Vec<U> {
        let mut index = 0;
        while index < self.running.len() {
            if let Some(output) = (&mut self.running[index].output).now_or_never() {
                let task = self.running.remove(index);
                self.ready.push((task.id, output));
            } else {
                index += 1;
            }
        }
        let mut taken = Vec::new();
        for (id, output) in std::mem::take(&mut self.ready) {
            match select(output) {
                Ok(output) => taken.push(output),
                Err(output) => self.ready.push((id, output)),
            }
        }
        taken
    }

    /// Drop matching tasks and their unapplied outputs without waiting.
    pub(super) fn abort(&mut self, matches: impl Fn(&TaskId) -> bool) {
        for task in self.remove(matches) {
            if task.on_cancel != OnCancel::RunToCompletion {
                task.abort.abort();
            }
        }
    }

    /// Drop matching tasks and wait for [`OnCancel::Await`] tasks to stop.
    pub(super) async fn cancel(&mut self, matches: impl Fn(&TaskId) -> bool) {
        for task in self.remove(matches) {
            match task.on_cancel {
                OnCancel::Await => {
                    task.abort.abort();
                    let _ = task.output.await;
                }
                OnCancel::Detach => task.abort.abort(),
                OnCancel::RunToCompletion => {}
            }
        }
    }

    pub(super) async fn cancel_all(&mut self) {
        self.cancel(|_| true).await;
    }

    fn remove(&mut self, matches: impl Fn(&TaskId) -> bool) -> Vec<RunningTask> {
        self.ready.retain(|(id, _)| !matches(id));
        let (removed, kept) = std::mem::take(&mut self.running)
            .into_iter()
            .partition(|task| matches(&task.id));
        self.running = kept;
        removed
    }
}

impl SessionOutput {
    /// Applying metadata can rebuild compaction and reasoning. Keep it queued
    /// while a provider turn or compact owns the session instead of dropping
    /// it after a SessionBusy error.
    fn waits_for_idle_session(&self) -> bool {
        matches!(self, Self::ModelMetadata { .. })
    }
}

impl App {
    /// Apply finished `Ui` outputs. Every loop calls this through
    /// `update_activity_panels`.
    pub(super) fn apply_finished_ui_tasks(&mut self) -> bool {
        let outputs = self.tasks.take_finished(|output| match output {
            TaskOutput::Ui(output) => Ok(output),
            output @ TaskOutput::Session(_) => Err(output),
        });
        let mut changed = false;
        for output in outputs {
            changed |= self.apply_ui_output(output);
        }
        changed
    }

    /// Idle-loop dispatch point: apply every finished output the session can
    /// take right now.
    pub(super) async fn apply_finished_tasks(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        let outputs = self.take_finished_tasks(agent);
        let mut changed = false;
        for output in outputs {
            changed |= match output {
                TaskOutput::Ui(output) => self.apply_ui_output(output),
                TaskOutput::Session(output) => {
                    self.apply_session_output(output, terminal, agent).await?
                }
            };
        }
        Ok(changed)
    }

    /// Finished outputs the session can apply now; the rest stay queued.
    pub(super) fn take_finished_tasks(&mut self, agent: &InteractiveRuntime) -> Vec<TaskOutput> {
        let session_busy = agent.is_session_busy();
        self.tasks.take_finished(|output| match output {
            TaskOutput::Session(output) if session_busy && output.waits_for_idle_session() => {
                Err(output.into())
            }
            output => Ok(output),
        })
    }

    /// Returns whether the screen changed.
    fn apply_ui_output(&mut self, output: UiOutput) -> bool {
        match output {
            UiOutput::GithubPr(result) => self.apply_github_pr(result),
            UiOutput::UsageLimits(id, result) => self.apply_limits_fetch(id, result),
            UiOutput::DoctorProbe(outcome) => self.apply_doctor_probe(&outcome),
            UiOutput::InfoRuntimes(result) => self.apply_info_runtimes_result(result),
            UiOutput::InfoTree(result) => self.apply_info_tree_result(result),
            UiOutput::SpendLoad(result) => self.apply_spend_load(result),
            UiOutput::DiffPatch(index, result) => self.apply_diff_patch(index, result),
            UiOutput::Changelog(result) => self.apply_changelog_fetch(result),
            UiOutput::WebSearchTest(result) => self.apply_web_search_test(result),
        }
    }

    /// Returns whether the screen changed.
    async fn apply_session_output(
        &mut self,
        output: SessionOutput,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        match output {
            SessionOutput::ModelMetadata {
                reasoning_at_start,
                metadata,
            } => {
                self.apply_model_metadata(agent, reasoning_at_start, metadata)
                    .await;
                Ok(false)
            }
            SessionOutput::UpdateNotice(result) => {
                if let Ok(Some(notice)) = result {
                    self.info.services.update_notice = Some(notice);
                }
                Ok(false)
            }
            SessionOutput::CustomModels => Ok(false),
            SessionOutput::CursorModels(result) => {
                self.apply_cursor_model_refresh(result);
                Ok(false)
            }
            SessionOutput::SyntaxWarmup => {
                // Rebuild history once the dump is ready so a plain first
                // paint gets roles.
                self.history.invalidate_from(0);
                Ok(true)
            }
            SessionOutput::HerdrGraphics(result) => {
                if let Ok(capability) = result {
                    self.image_picker = super::feed_image::picker_from_environment(capability);
                }
                Ok(false)
            }
            SessionOutput::InteractiveLogin(finished) => {
                self.apply_interactive_login(finished, terminal, agent)
                    .await?;
                Ok(false)
            }
        }
    }
}
