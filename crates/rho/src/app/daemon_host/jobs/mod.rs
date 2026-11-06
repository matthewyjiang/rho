//! The daemon's job registry: every unit of agent work the daemon knows,
//! with its state, where it runs, and the session it writes.
//!
//! A job is the turns of a session the daemon hosts (see `sessions/`), or a
//! subagent run one of those sessions started, whose parent is the
//! session's job (see `subagents.rs`). Workflow runs join as jobs of their
//! own kind when the daemon hosts them (#1658).
//! A job's state moves only through the events in `state.rs`, and every
//! transition rewrites the job's durable record (see `store.rs`), so a
//! daemon that restarts lists the jobs its predecessor ran and marks the
//! ones it left unfinished as interrupted.
//!
//! Records are never deleted yet: a retention policy is a follow-up.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

mod registry;
mod state;
mod store;

pub(super) use registry::{JobRegistry, StartTurnError};
pub(super) use state::{JobEvent, JobState, TurnEnd};
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
    /// The turns of a session the daemon hosts, prompted one by one.
    Session,
    /// One subagent run, started by the session job that is its parent.
    Subagent,
}

/// What the daemon knows about one job, as its record file holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct JobRecord {
    pub(super) id: JobId,
    pub(super) kind: JobKind,
    pub(super) state: JobState,
    /// The workspace the job runs in, canonicalized.
    pub(super) workspace: PathBuf,
    /// The job that started this one, such as a subagent run's parent; `None`
    /// for a job a client started.
    pub(super) parent: Option<JobId>,
    /// The Rho session the job writes; `None` for a subagent run on
    /// another runtime (Claude, Cursor, Antigravity), which keeps its own.
    pub(super) session: Option<rho_sdk::SessionId>,
    /// Unix time, in seconds, when the job was created.
    pub(super) created_at: u64,
    /// Unix time, in seconds, of the job's last transition.
    pub(super) updated_at: u64,
}

impl JobRecord {
    /// A new job `id` for the turns of `session` in `workspace`, before its
    /// first turn.
    pub(super) fn new_session(id: JobId, workspace: PathBuf, session: rho_sdk::SessionId) -> Self {
        let now = unix_now();
        Self {
            id,
            kind: JobKind::Session,
            state: JobState::Idle,
            workspace,
            parent: None,
            session: Some(session),
            created_at: now,
            updated_at: now,
        }
    }

    /// A new job for a subagent run that job `parent` started in
    /// `workspace`, waiting for capacity.
    pub(super) fn new_subagent(
        parent: JobId,
        workspace: PathBuf,
        session: Option<rho_sdk::SessionId>,
    ) -> Self {
        let now = unix_now();
        Self {
            id: JobId::generate(),
            kind: JobKind::Subagent,
            state: JobState::Queued,
            workspace,
            parent: Some(parent),
            session,
            created_at: now,
            updated_at: now,
        }
    }
}

/// The current Unix time in seconds; 0 on a clock set before 1970.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_secs())
}
