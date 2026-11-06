//! What a session's in-process subagents share with the host that runs the
//! session.
//!
//! Most hosts run each session's subagents on the session's own pool
//! sized by its config's `agent_concurrency`. The daemon shares one pool
//! across every session it hosts, so the budget is daemon-wide, and hears of
//! each run so it can list the run as a child job of its session's (#1658).

use std::{path::PathBuf, sync::Arc};

use super::agent_concurrency::{AgentConcurrency, QueueWatch};
use crate::subagent::RunStatus;

/// Where a session's subagents take capacity, and who else hears of them.
#[derive(Clone)]
pub(crate) enum SubagentPool {
    /// The session's own pool, sized by its config's `agent_concurrency`;
    /// nobody else hears of its runs.
    Own,
    /// A pool the host shares across its sessions, and the observer of this
    /// session's runs.
    Shared {
        concurrency: AgentConcurrency,
        observer: Arc<dyn SubagentObserver>,
    },
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

/// Hears of every subagent run a session starts. The daemon implements it to
/// record each run as a child job of the session's job.
///
/// [`Self::started`] is called on the task that spawned the run, right after
/// the spawn, possibly before the run has capacity and possibly after it
/// already ended. Implementors must return
/// promptly and follow the run through [`StartedSubagent::status`] on a task
/// of their own.
pub(crate) trait SubagentObserver: Send + Sync {
    fn started(&self, run: StartedSubagent);
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
