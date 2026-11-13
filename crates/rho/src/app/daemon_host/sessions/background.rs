//! A loaded session's background work: the processes, workflow runs and
//! subagents its turns started that run on their own. Its clients follow it
//! as [`wire::SessionEvent::Background`] and stop one piece with
//! [`wire::SESSION_BACKGROUND_STOP`].
//!
//! The work is reported as a turn begins, so a client hears how the work a
//! delivery turn takes ended first; as a turn ends, so it hears what the
//! turn left running first; and by one watcher per loaded session as work
//! starts or ends, turn or no turn. Each report reads the work under the
//! feed's lock, so two reporters never publish out of order.

use std::sync::{Arc, Mutex, PoisonError, Weak};

use rho_protocol::{self as wire, ErrorCode, RpcError};
use rho_sdk::CancellationToken;
use tokio::sync::Notify;

use super::{LoadedSession, Sessions};
use crate::{
    app::{
        daemon_host::jobs::JobRegistry,
        session_runtime::SessionRuntime,
        subagent_manager::{BackgroundChild, SubagentManager},
    },
    subagent::RunState,
    tools::{
        process::{LiveProcessSummary, ProcessManager, State, STOP_GRACE},
        workflow_tracker::{WorkflowRailSummary, WorkflowRunTracker},
    },
    workflow::{RunLifecycle, WorkflowOutcome},
};

/// A loaded session's background work, read without its runtime's lock.
pub(super) struct SessionBackground {
    /// The parent id the session's workflow runs and subagents report to.
    session_id: String,
    sources: Mutex<Sources>,
    workflows: WorkflowRunTracker,
    /// Wakes the watcher when the sources were replaced, so it waits on
    /// the new ones.
    replaced: Arc<Notify>,
    /// Cancelled when the session is dropped, which ends the watcher.
    dropped: CancellationToken,
}

/// The session's tools that run work of their own, when it has them. A
/// reconfigured tool set may replace them.
#[derive(Clone)]
struct Sources {
    processes: Option<ProcessManager>,
    subagents: Option<SubagentManager>,
}

impl Sources {
    fn of(runtime: &SessionRuntime) -> Self {
        Self {
            processes: runtime.tools().processes().cloned(),
            subagents: runtime.tools().subagents().cloned(),
        }
    }
}

impl SessionBackground {
    pub(super) fn of(runtime: &SessionRuntime) -> Self {
        Self {
            session_id: runtime.session_id().to_string(),
            sources: Mutex::new(Sources::of(runtime)),
            workflows: runtime.tools().workflow_tracker().clone(),
            replaced: Arc::new(Notify::new()),
            dropped: CancellationToken::new(),
        }
    }

    /// Takes the tools `runtime` has now, as after a configure.
    pub(super) fn refresh(&self, runtime: &SessionRuntime) {
        *self.sources.lock().unwrap_or_else(PoisonError::into_inner) = Sources::of(runtime);
        self.replaced.notify_one();
    }

    /// The session's process manager, when its tools have one.
    pub(super) fn processes(&self) -> Option<ProcessManager> {
        self.sources().processes
    }

    fn sources(&self) -> Sources {
        self.sources
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The work as a client sees it: processes, then workflow runs, then
    /// subagents, each oldest first.
    fn snapshot(&self) -> Vec<wire::BackgroundWork> {
        let sources = self.sources();
        let processes = sources
            .processes
            .iter()
            .flat_map(ProcessManager::live_summaries)
            .map(process_work);
        let workflows = self
            .workflows
            .rail_summaries(&self.session_id)
            .into_iter()
            .map(workflow_work);
        let subagents = sources
            .subagents
            .iter()
            .flat_map(|subagents| subagents.background(&self.session_id))
            .map(subagent_work);
        processes.chain(workflows).chain(subagents).collect()
    }

    /// Resolves once a process, subagent or workflow run may have started
    /// or ended from now on, except a workflow run's start, which its job's
    /// registration tells (see [`watch`]).
    fn changes(&self) -> impl std::future::Future<Output = ()> + Send + 'static {
        let sources = self.sources();
        // Subscribed here, not at the first poll.
        let exited = sources
            .processes
            .as_ref()
            .map(ProcessManager::notified_owned);
        let started = sources
            .processes
            .as_ref()
            .map(ProcessManager::started_owned);
        let subagents = sources.subagents.as_ref().map(SubagentManager::changed);
        let finished = self.workflows.notified_owned();
        let replaced = Arc::clone(&self.replaced);
        async move {
            tokio::select! {
                () = or_pending(exited) => {}
                () = or_pending(started) => {}
                () = or_pending(subagents) => {}
                () = finished => {}
                () = replaced.notified() => {}
            }
        }
    }

    /// Asks `item` to stop. Fails with invalid params for work the session
    /// does not know or that already ended.
    async fn stop(&self, item: &wire::BackgroundItem) -> Result<(), String> {
        let sources = self.sources();
        match item {
            wire::BackgroundItem::Process { id } => {
                let processes = sources.processes.ok_or("the session runs no processes")?;
                processes.stop(id, STOP_GRACE).await
            }
            wire::BackgroundItem::Workflow { run } => {
                let row = self
                    .workflows
                    .rail_summaries(&self.session_id)
                    .into_iter()
                    .find(|row| row.run_id == *run && row.is_live())
                    .ok_or_else(|| format!("no running workflow run '{run}'"))?;
                let run_id = run.parse().map_err(|error| format!("{error}"))?;
                let rho_home = crate::paths::rho_dir().map_err(|error| format!("{error:#}"))?;
                crate::app::workflow_cli::request_cancellation(&rho_home, run_id, row.lifecycle)
                    .await
                    .map(drop)
                    .map_err(|error| error.to_string())
            }
            wire::BackgroundItem::Subagent { run, .. } => sources
                .subagents
                .ok_or("the session runs no subagents")?
                .cancel(&self.session_id, run)
                .map_err(|error| error.to_string()),
            _ => Err("this daemon does not know that kind of work".into()),
        }
    }
}

impl Drop for SessionBackground {
    fn drop(&mut self) {
        self.dropped.cancel();
    }
}

async fn or_pending(future: Option<impl std::future::Future<Output = ()>>) {
    match future {
        Some(future) => future.await,
        None => std::future::pending().await,
    }
}

impl LoadedSession {
    /// Tells the session's clients its background work, if it changed
    /// since they were last told.
    pub(super) fn report_background(&self) {
        self.feed.update_background(|| self.background.snapshot());
    }
}

impl Sessions {
    /// Starts `session`'s background watcher. Call once, as the session is
    /// added.
    pub(super) fn watch_background(&self, session: &Arc<LoadedSession>) {
        let watcher = watch(Arc::downgrade(session), Arc::clone(&session.jobs));
        let dropped = session.background.dropped.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = dropped.cancelled() => {}
                () = watcher => {}
            }
        });
    }

    /// `rho/session/background/stop`.
    pub(in crate::app::daemon_host) async fn background_stop(
        &self,
        params: wire::SessionBackgroundStopParams,
    ) -> Result<wire::SessionBackgroundStopResponse, RpcError> {
        self.refuse_when_stopping()?;
        let not_loaded = || {
            RpcError::new(
                ErrorCode::SessionNotLoaded,
                "the daemon has not loaded the session; attach to it first",
            )
        };
        let session = self
            .loaded_session(&params.session)
            .await?
            .ok_or_else(not_loaded)?;
        session
            .background
            .stop(&params.work)
            .await
            .map_err(|message| {
                RpcError::new(
                    ErrorCode::InvalidParams,
                    format!("could not stop background work: {message}"),
                )
            })?;
        Ok(wire::SessionBackgroundStopResponse {})
    }
}

/// Reports `session`'s background work each time it may have changed,
/// until the session is gone. Holds the session weakly, so it never keeps
/// one loaded.
async fn watch(session: Weak<LoadedSession>, jobs: Arc<JobRegistry>) {
    loop {
        // Subscribed before the report, so a change in between still wakes
        // the next look. A workflow run registers its job once the tracker
        // holds it, so a job change tells its start.
        let job = jobs.changed();
        tokio::pin!(job);
        job.as_mut().enable();
        let changes = {
            let Some(session) = session.upgrade() else {
                return;
            };
            let changes = session.background.changes();
            session.report_background();
            changes
        };
        tokio::select! {
            () = changes => {}
            () = job => {}
        }
    }
}

fn process_work(process: LiveProcessSummary) -> wire::BackgroundWork {
    let status = match process.state {
        State::Starting | State::Running => wire::BackgroundStatus::Running,
        State::Exited => wire::BackgroundStatus::Completed,
        State::Terminated => wire::BackgroundStatus::Cancelled,
        State::TimedOut | State::FailedToStart => wire::BackgroundStatus::Failed,
    };
    wire::BackgroundWork {
        item: wire::BackgroundItem::Process {
            id: process.process_id,
        },
        title: process.command,
        status,
        exit_code: process.exit_code,
    }
}

fn workflow_work(run: WorkflowRailSummary) -> wire::BackgroundWork {
    let status = if run.is_live() {
        wire::BackgroundStatus::Running
    } else if run.outcome == Some(WorkflowOutcome::Cancellation) {
        wire::BackgroundStatus::Cancelled
    } else if run.is_failure() || run.lifecycle != RunLifecycle::Completed {
        wire::BackgroundStatus::Failed
    } else {
        wire::BackgroundStatus::Completed
    };
    wire::BackgroundWork {
        item: wire::BackgroundItem::Workflow { run: run.run_id },
        title: run.workflow_name,
        status,
        exit_code: None,
    }
}

fn subagent_work(child: BackgroundChild) -> wire::BackgroundWork {
    let status = match child.state {
        RunState::Queued | RunState::Starting | RunState::Running => {
            wire::BackgroundStatus::Running
        }
        RunState::Ok => wire::BackgroundStatus::Completed,
        RunState::Error => wire::BackgroundStatus::Failed,
        RunState::Stopped => wire::BackgroundStatus::Cancelled,
    };
    wire::BackgroundWork {
        item: wire::BackgroundItem::Subagent {
            run: child.run,
            session: child
                .session
                .map(|session| wire::SessionId::new(session.to_string())),
        },
        title: child.title,
        status,
        exit_code: None,
    }
}
