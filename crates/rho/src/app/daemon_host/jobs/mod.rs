//! The daemon's job registry: every unit of agent work the daemon knows,
//! with its state, where it runs, and the session it writes.
//!
//! A job today is a thread (see `threads/`). Subagent runs and workflow runs
//! join as jobs with their own kinds when the daemon hosts them (2c-3,
//! 2c-4). A job's state moves only through the explicit transitions in
//! `state.rs`, and every transition rewrites the job's durable record (see
//! `store.rs`), so a daemon that restarts lists the jobs its predecessor ran
//! and marks the ones it left unfinished as interrupted.
//!
//! Records are never deleted yet: a retention policy is a follow-up.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

mod registry;
mod state;
mod store;

pub(super) use registry::{JobRegistry, StartTurnError, UnknownJob};
pub(super) use state::JobState;
pub(super) use store::JobStore;

/// One job's identity: a UUID v4, as a string on the wire and as the name
/// of its record file. Only [`JobId::generate`] and [`JobId::parse`] make
/// one, so every id names a safe file.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(super) struct JobId(String);

/// A string that is not a job id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("not a job id")]
pub(super) struct InvalidJobId;

impl JobId {
    /// A new, random id.
    pub(super) fn generate() -> Self {
        Self(uuid::Uuid::new_v4().hyphenated().to_string())
    }

    /// The id `value` names, in its canonical (lowercase, hyphenated) form.
    pub(super) fn parse(value: &str) -> Result<Self, InvalidJobId> {
        uuid::Uuid::parse_str(value)
            .map(|uuid| Self(uuid.hyphenated().to_string()))
            .map_err(|_| InvalidJobId)
    }

    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for JobId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for JobId {
    type Error = InvalidJobId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<JobId> for String {
    fn from(id: JobId) -> Self {
        id.0
    }
}

/// What a job runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum JobKind {
    /// A conversation the daemon hosts: one session, prompted turn by turn.
    Thread,
}

/// What the daemon knows about one job, as its record file holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct JobRecord {
    pub(super) id: JobId,
    pub(super) kind: JobKind,
    pub(super) state: JobState,
    /// The workspace the job runs in, canonicalized.
    pub(super) project_root: PathBuf,
    /// The job that started this one, such as a subagent's thread; `None`
    /// for a job a client started.
    pub(super) parent: Option<JobId>,
    /// The session the job writes.
    pub(super) session_id: rho_sdk::SessionId,
    /// Unix time, in seconds, when the job was created.
    pub(super) created_at: u64,
    /// Unix time, in seconds, of the job's last transition.
    pub(super) updated_at: u64,
    /// Why the job's last turn failed. Set only in [`JobState::Failed`].
    pub(super) error: Option<String>,
}

impl JobRecord {
    /// A new thread job on `session_id` in `project_root`, before its
    /// first turn.
    pub(super) fn new_thread(project_root: PathBuf, session_id: rho_sdk::SessionId) -> Self {
        let now = unix_now();
        Self {
            id: JobId::generate(),
            kind: JobKind::Thread,
            state: JobState::Idle,
            project_root,
            parent: None,
            session_id,
            created_at: now,
            updated_at: now,
            error: None,
        }
    }
}

/// The current Unix time in seconds; 0 on a clock set before 1970.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_secs())
}
