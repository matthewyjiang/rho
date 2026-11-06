//! What a session's in-process subagents share with the host that runs the
//! session.
//!
//! Most hosts run each session's subagents, and the agent nodes of the
//! workflow runs it starts, on a pool of their own sized by the config's
//! `agent_concurrency`. The daemon shares one pool across every session it
//! hosts, so the budget is daemon-wide, and hears of each subagent and
//! workflow run so it can list the run as a child job of its session's
//! (#1658).

use std::{path::PathBuf, sync::Arc};

use super::{
    agent_concurrency::{AgentConcurrency, QueueWatch},
    agent_executor::AgentRunHandle,
};
use crate::{subagent::RunStatus, workflow::RunId};

/// Where a session's in-process agent runs take capacity, its subagents and
/// its workflow runs' agent nodes, and who else hears of them.
#[derive(Clone)]
pub(crate) enum SubagentPool {
    /// The session's own pool, sized by its config's `agent_concurrency`;
    /// nobody else hears of its runs.
    Own,
    /// A pool the host shares across its sessions, and the observer of the
    /// runs started here.
    Shared {
        concurrency: AgentConcurrency,
        observer: Arc<dyn ChildObserver>,
    },
}

impl SubagentPool {
    /// The pool runs take capacity from, an own one sized to
    /// `agent_concurrency`, and the observer that hears of them, if any.
    pub(crate) fn into_parts(
        self,
        agent_concurrency: usize,
    ) -> (AgentConcurrency, Option<Arc<dyn ChildObserver>>) {
        match self {
            Self::Own => (AgentConcurrency::from_config(agent_concurrency), None),
            Self::Shared {
                concurrency,
                observer,
            } => (concurrency, Some(observer)),
        }
    }

    /// The pool workflow run `run`'s agent nodes take capacity from: this
    /// one, whose observer hears of the run first and returns the observer
    /// of its nodes, and where to report how the run ended, if anyone
    /// follows it.
    pub(crate) fn host_workflow(
        self,
        run: StartedWorkflow,
    ) -> (Self, Option<tokio::sync::oneshot::Sender<WorkflowEnd>>) {
        match self {
            Self::Own => (Self::Own, None),
            Self::Shared {
                concurrency,
                observer,
            } => {
                let HostedWorkflow { observer, end } = observer.workflow_started(run);
                (
                    Self::Shared {
                        concurrency,
                        observer,
                    },
                    Some(end),
                )
            }
        }
    }
}

impl std::fmt::Debug for SubagentPool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Own => formatter.write_str("Own"),
            Self::Shared { concurrency, .. } => formatter
                .debug_struct("Shared")
                .field("limit", &concurrency.total_limit())
                .finish_non_exhaustive(),
        }
    }
}

/// Hears of the subagent and workflow runs a session starts, and of the
/// agent node runs of those workflow runs. The daemon implements it to
/// record each run as a child job of the job that started it.
///
/// Both methods are called on the task that starts the run and must return
/// promptly; implementors follow the run on a task of their own.
pub(crate) trait ChildObserver: Send + Sync {
    /// Called right after the run is spawned, possibly before it has
    /// capacity and possibly after it already ended.
    fn subagent_started(&self, run: StartedSubagent);

    /// Called before the run is driven. Returns the observer of the run's
    /// agent node runs, and where the run reports how it ended. Workflow
    /// agent nodes start no workflow runs, so the returned observer only
    /// hears [`Self::subagent_started`].
    fn workflow_started(&self, run: StartedWorkflow) -> HostedWorkflow;
}

/// A subagent run that just started, as its observer follows it.
pub(crate) struct StartedSubagent {
    /// The run's id among its parent session's runs.
    pub(crate) run_id: String,
    /// The Rho session the run writes; `None` for a run on another runtime
    /// (Claude, Cursor, Antigravity), which keeps that runtime's own.
    pub(crate) session: Option<rho_sdk::SessionId>,
    /// The directory the run works in.
    pub(crate) workspace: PathBuf,
    /// The run's status, from `queued` until it is terminal.
    pub(crate) status: tokio::sync::watch::Receiver<RunStatus>,
    /// Where the run waits for capacity, while it does.
    pub(crate) queue: QueueWatch,
}

impl StartedSubagent {
    /// The run `handle` follows, started as `run_id` in `workspace`.
    pub(crate) fn of(run_id: String, workspace: PathBuf, handle: &AgentRunHandle) -> Self {
        Self {
            run_id,
            session: handle.session().cloned(),
            workspace,
            status: handle.clone_status_watch(),
            queue: handle.queue(),
        }
    }
}

/// A workflow run about to be driven in the background.
pub(crate) struct StartedWorkflow {
    pub(crate) run_id: RunId,
    /// The workspace the run was planned in, where its nodes work.
    pub(crate) workspace: PathBuf,
}

/// How the host follows a workflow run it heard of.
pub(crate) struct HostedWorkflow {
    /// Hears of each of the run's agent node runs.
    pub(crate) observer: Arc<dyn ChildObserver>,
    /// Takes how the run ended. Dropped unsent when the drive stops without
    /// an end, such as when the host's runtime shuts down under it.
    pub(crate) end: tokio::sync::oneshot::Sender<WorkflowEnd>,
}

/// How a workflow run's drive ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WorkflowEnd {
    /// Every node finished and the run succeeded.
    Completed,
    /// The run was cancelled.
    Cancelled,
    /// The run failed, was denied or blocked, or could not be driven.
    Failed(String),
}
