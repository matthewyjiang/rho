//! Jobs: every unit of agent work the daemon knows, including the ones an
//! earlier daemon ran. A session's turns are a job, and so is each subagent
//! run a session starts; workflow runs become jobs in a later protocol minor
//! (#1658).

use std::{
    fmt,
    path::{Path, PathBuf},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{InputRequest, ModelRef, SessionId};

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
    /// One subagent run, started by the session or workflow job that is its
    /// [`JobSummary::parent`].
    Subagent,
    /// One drive of a workflow run, started by the session job that is its
    /// [`JobSummary::parent`]; the run's agent nodes are its child
    /// [`JobKind::Subagent`] jobs.
    Workflow,
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
    /// A subagent run, or the turn of a session another job started
    /// ([`super::SessionNewParams::parent`]), waiting for capacity;
    /// [`JobSummary::queue`] says where it stands.
    Queued,
    Running,
    /// The turn waits for an approval, a plan approval, or a question to be
    /// answered with [`super::SESSION_ANSWER`]. Nothing is denied because no
    /// client is attached; the wait ends with an answer, a cancel, or a
    /// timeout the user configured ([`super::InputTimeout`]).
    WaitingForInput,
    Completed,
    /// The turn failed; [`JobSummary::error`] says why.
    Failed,
    Cancelled,
    /// The turn ran past the limit its prompt set and was cancelled;
    /// [`crate::TurnOutcome::TimedOut`] says the limit.
    TimedOut,
    /// The daemon stopped, or died, before the turn ended.
    Interrupted,
    /// A state a newer daemon sends that this crate does not know. Reading
    /// it leniently keeps the rest of the job list usable; daemons never
    /// send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

impl JobState {
    /// Whether a turn is under way in this state: running, queued, or
    /// waiting for input.
    pub const fn in_progress(self) -> bool {
        matches!(self, Self::Running | Self::Queued | Self::WaitingForInput)
    }
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
    /// What the turn under way, or the last one, of a
    /// [`JobKind::Session`] job may do through session control: what the
    /// request that started the turn asked for, held to `grant_limit`;
    /// before the first turn, what the session opened with. Absent for a
    /// turn without limits, as a person's own are, and for other jobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant: Option<super::SessionGrant>,
    /// The most `grant` may be, for a session that opened under a limit, as
    /// one an agent starts through session control does: the grant it
    /// opened with. The daemon holds every later grant to it, whoever
    /// prompts the session: no shallower `depth`, no wider `access` or
    /// `workspaces`. Absent for a session that opened without a limit, as
    /// a person's own does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_limit: Option<super::SessionGrant>,
    /// The Rho session the job writes; `None` for a subagent run on another
    /// runtime (Claude, Cursor, Antigravity), which keeps its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionId>,
    /// The id of the subagent run a [`JobKind::Subagent`] job follows. For
    /// a run a session started, it names the run's artifact directory
    /// under the daemon's rho home.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// The workflow run a [`JobKind::Workflow`] job drives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run: Option<String>,
    /// Whether the result of a [`JobKind::Workflow`] job's run has yet to
    /// reach the session that started it. A run's result is pending once
    /// its drive ends, until the daemon gives the session the result in a
    /// turn, at once when the session is idle. `false` for other jobs, and
    /// for a run whose session the daemon has not loaded.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub result_pending: bool,
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
    /// What the daemon follows of the job right now. Present for a
    /// [`JobKind::Session`] job whose session the daemon has loaded, and
    /// for a [`JobKind::Subagent`] job whose run is under way and has
    /// opened its session; absent for every other job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live: Option<JobLive>,
    /// The git worktree a [`JobKind::Session`] job's session works in, for
    /// one opened with [`super::SessionNewParams::worktree`]; `workspace`
    /// is then in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<super::SessionWorktree>,
    /// Whether `worktree` is stale: still on disk, neither its session
    /// loaded nor a job in it under way, and no change to this job for 14
    /// days. The daemon removes a worktree only once it is clean and its
    /// branch merged ([`super::WORKTREE_PRUNE`]), so a stale one holds work
    /// nobody merged; it stays until a person merges or removes it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub worktree_stale: bool,
}

impl JobSummary {
    /// The project the job works for: for a session in a worktree, the
    /// workspace the worktree was added from; else its `workspace`.
    pub fn project(&self) -> &Path {
        self.worktree
            .as_ref()
            .map_or(&self.workspace, |worktree| &worktree.origin)
    }
}

/// What the daemon follows of a job whose session's events it streams: the
/// last its stream said of each. A client lists jobs to show these without
/// attaching to every session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobLive {
    /// The session's title, once it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The model the session runs on.
    pub model: ModelRef,
    /// How long the turn under way has run, in milliseconds; absent while
    /// none is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_elapsed_ms: Option<u64>,
    /// The session's context use and spend, as its last
    /// [`super::SessionEvent::Usage`] said; absent before the first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<JobUsage>,
    /// The input requests open on the session, oldest first, each answered
    /// with [`super::SESSION_ANSWER`]: the turn's own, which put the job in
    /// [`JobState::WaitingForInput`], and its subagents' questions, which
    /// do not.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<InputRequest>,
}

/// A session's context use and spend, as [`super::SessionEvent::Usage`]
/// reports them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobUsage {
    pub used_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_tokens: Option<u64>,
    /// The session's whole spend so far in millionths of a US dollar.
    /// Absent while no request has a price, for a subagent run, whose
    /// spend its parent session's counts, and for a session the daemon
    /// reopened, until its next turn reports usage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd_micros: Option<u64>,
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
