//! Jobs: every unit of agent work the daemon knows, including the ones an
//! earlier daemon ran. A session's turns are a job, and so is each subagent
//! run a session starts; workflow runs become jobs in a later protocol minor
//! (#1658).

use std::{fmt, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::SessionId;

/// List every job the daemon knows.
pub const JOB_LIST: &str = "rho/job/list";

/// A job's id. Opaque to clients: send back the string the daemon gave.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct JobId(String);

impl JobId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// What a job runs. Protocol minors may add kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum JobKind {
    /// The turns of a session the daemon hosts; see [`super::SESSION_NEW`].
    Session,
    /// One subagent run, started by the session job that is its
    /// [`JobSummary::parent`].
    Subagent,
    /// A kind a newer daemon sends that this crate does not know. Reading it
    /// leniently keeps the rest of the job list usable; daemons never send
    /// it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// Where a job is. A session job's state is that of its current or last
/// turn; a subagent job's is that of its one run.
/// Protocol minors may add states.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum JobState {
    /// Created; no turn has started.
    Idle,
    /// A subagent run waiting for capacity; [`JobSummary::queue`] says
    /// where it stands.
    Queued,
    Running,
    /// The turn waits for an approval, a plan approval, or a question to be
    /// answered. Nothing is auto-denied because no client is attached; until
    /// clients can answer (#1660), only [`super::SESSION_CANCEL`] ends the
    /// wait.
    WaitingForInput,
    Completed,
    /// The turn failed; [`JobSummary::error`] says why.
    Failed,
    Cancelled,
    /// The daemon stopped, or died, before the turn ended.
    Interrupted,
    /// A state a newer daemon sends that this crate does not know. Reading
    /// it leniently keeps the rest of the job list usable; daemons never
    /// send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// One job, as [`JOB_LIST`] reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobSummary {
    pub id: JobId,
    pub kind: JobKind,
    pub state: JobState,
    /// The workspace the job runs in.
    pub workspace: PathBuf,
    /// The job that started this one; `None` for a job a client started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<JobId>,
    /// The Rho session the job writes; `None` for a subagent run on another
    /// runtime (Claude, Cursor, Antigravity), which keeps its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionId>,
    /// Unix time, in seconds, when the job was created.
    pub created_at: u64,
    /// Unix time, in seconds, of the job's last state change.
    pub updated_at: u64,
    /// Why the job failed, when its state is [`JobState::Failed`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Where the job waits for capacity, when its state is
    /// [`JobState::Queued`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue: Option<QueuePlace>,
}

/// Where a queued job waits: the budget it waits on, that budget's limit,
/// and its place in the budget's queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct QueuePlace {
    pub budget: Budget,
    /// How many runs the budget lets run at once.
    pub limit: u32,
    /// 1 for the job that starts when the next slot frees.
    pub position: u32,
}

/// A concurrency budget jobs wait on, named for the setting that sizes it.
/// Protocol minors may add budgets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Budget {
    /// Every subagent run of every session the daemon hosts:
    /// `behavior.agent_concurrency` in the daemon's config.
    AgentConcurrency,
    /// Claude-cli subagent runs, nested under [`Self::AgentConcurrency`]:
    /// `RHO_CLAUDE_AGENT_CONCURRENCY` in the daemon's environment.
    ClaudeAgentConcurrency,
    /// A budget a newer daemon sends that this crate does not know. Reading
    /// it leniently keeps the rest of the job list usable; daemons never
    /// send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// Params of [`JOB_LIST`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobListParams {}

/// Result of [`JOB_LIST`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobListResponse {
    /// Every job, oldest first.
    pub jobs: Vec<JobSummary>,
}

#[cfg(test)]
#[path = "jobs_tests.rs"]
mod tests;
