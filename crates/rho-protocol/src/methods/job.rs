//! Jobs: every unit of agent work the daemon knows, including the ones an
//! earlier daemon ran. A session's turns are a job; subagent and workflow
//! runs become jobs in a later protocol minor (#1658).

use std::{fmt, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Method, SessionId};

/// List every job the daemon knows.
pub const JOB_LIST: &str = "rho/job/list";

/// Marker for [`JOB_LIST`].
pub struct JobList;

impl Method for JobList {
    const NAME: &'static str = JOB_LIST;
    type Params = JobListParams;
    type Result = JobListResponse;
}

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
    /// A kind a newer daemon sends that this crate does not know. Reading it
    /// leniently keeps the rest of the job list usable; daemons never send
    /// it.
    #[serde(other)]
    Unknown,
}

/// Where a job is. A session job's state is that of its current or last
/// turn.
/// Protocol minors may add states.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum JobState {
    /// Created; no turn has started.
    Idle,
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
    pub parent: Option<JobId>,
    /// The session the job writes.
    pub session: SessionId,
    /// Unix time, in seconds, when the job was created.
    pub created_at: u64,
    /// Unix time, in seconds, of the job's last state change.
    pub updated_at: u64,
    /// Why the job failed, when its state is [`JobState::Failed`].
    pub error: Option<String>,
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
