//! Jobs: every unit of agent work the daemon knows, including the ones an
//! earlier daemon ran. A thread is a job; subagent and workflow runs become
//! jobs in a later protocol minor.

use std::{fmt, path::PathBuf};

use serde::{Deserialize, Serialize};

use super::Method;

/// List every job the daemon knows.
pub const JOBS_LIST: &str = "rho/jobs/list";

/// Marker for [`JOBS_LIST`].
pub struct JobsList;

impl Method for JobsList {
    const NAME: &'static str = JOBS_LIST;
    type Params = JobsListParams;
    type Result = JobsListResponse;
}

/// A job's id. Opaque to clients: send back the string the daemon gave.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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

/// What a job runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    /// A conversation the daemon hosts; see [`super::THREAD_CREATE`].
    Thread,
}

/// Where a job is. A thread's state is that of its current or last turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Created; no turn has started.
    Idle,
    Running,
    /// The turn waits for an approval, a plan approval, or a question to be
    /// answered. Nothing is auto-denied because no client is attached; until
    /// clients can answer, only [`super::THREAD_CANCEL`] ends the wait.
    WaitingForInput,
    Completed,
    /// The turn failed; [`JobSummary::error`] says why.
    Failed,
    Cancelled,
    /// The daemon stopped, or died, before the turn ended.
    Interrupted,
}

/// One job, as [`JOBS_LIST`] reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSummary {
    pub id: JobId,
    pub kind: JobKind,
    pub state: JobState,
    /// The workspace the job runs in.
    pub project_root: PathBuf,
    /// The job that started this one; `None` for a job a client started.
    pub parent: Option<JobId>,
    /// The session the job writes.
    pub session_id: String,
    /// Unix time, in seconds, when the job was created.
    pub created_at: u64,
    /// Unix time, in seconds, of the job's last state change.
    pub updated_at: u64,
    /// Why the job failed, when its state is [`JobState::Failed`].
    pub error: Option<String>,
}

/// Params of [`JOBS_LIST`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobsListParams {}

/// Result of [`JOBS_LIST`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobsListResponse {
    /// Every job, oldest first.
    pub jobs: Vec<JobSummary>,
}
