//! The daemon's job registry: every unit of agent work the daemon knows,
//! with its state, where it runs, and the session it writes.
//!
//! A job is the turns of a session the daemon hosts (see `sessions/`), a
//! workflow run one of those sessions drives, or a subagent run, whose
//! parent is the job of the session or workflow run that started it (see
//! `subagents.rs`).
//! A job's state moves only through the events in `state.rs`, and every
//! transition rewrites the job's durable record (see `store.rs`), so a
//! daemon that restarts lists the jobs its predecessor ran and marks the
//! ones it left unfinished as interrupted.
//!
//! A record is deleted only with a session the daemon discards having saved
//! nothing (see `sessions/unload.rs`): no request could reopen it. Others
//! stay; a retention policy is a follow-up.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::workflow::RunId;

mod registry;
mod state;
mod store;

pub(super) use registry::{JobRegistry, RegisterError, SessionJob, StartTurnError};
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

/// What a job runs, and what that run is known by. A record holds it
/// flattened: `kind` names the variant beside its fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum JobSubject {
    /// The turns of a session the daemon hosts, prompted one by one.
    Session { session: rho_sdk::SessionId },
    /// One subagent run, started by the session or workflow job that is its
    /// parent. `session` is the Rho session it writes; `None` for a run on
    /// another runtime (Claude, Cursor, Antigravity), which keeps its own.
    Subagent {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<rho_sdk::SessionId>,
    },
    /// One drive of workflow run `workflow_run`, started by the session job
    /// that is its parent. The run's agent nodes are its subagent jobs.
    Workflow { workflow_run: RunId },
}

/// How a session job's turns get under way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Admission {
    /// At once, as a person's session's do.
    #[default]
    Immediate,
    /// Once they have a slot in the daemon-wide agent budget, which
    /// subagent runs share, as the turns of a session another session
    /// started do (#1714).
    SharedBudget,
}

/// What the daemon knows about one job, as its record file holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct JobRecord {
    pub(super) id: JobId,
    #[serde(flatten)]
    pub(super) subject: JobSubject,
    pub(super) state: JobState,
    /// The workspace the job runs in, canonicalized.
    pub(super) workspace: PathBuf,
    /// The job that started this one, such as a subagent run's parent; `None`
    /// for a job a client started.
    pub(super) parent: Option<JobId>,
    /// How a session job's turns get under way; subagent and workflow jobs
    /// keep [`Admission::Immediate`], their runs taking capacity as
    /// `subagents.rs` says.
    #[serde(default, skip_serializing_if = "Admission::is_immediate")]
    pub(super) admission: Admission,
    /// Unix time, in seconds, when the job was created.
    pub(super) created_at: u64,
    /// Unix time, in seconds, of the job's last transition.
    pub(super) updated_at: u64,
    /// How the job's last turn ended, as the session's clients were told,
    /// so a later daemon that reopens the session tells them again (#1661).
    /// `None` while no turn has ended, for a turn the daemon's stop or death
    /// interrupted, and for jobs other than a session's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) outcome: Option<rho_protocol::TurnOutcome>,
}

impl JobRecord {
    /// A new job `id` for the turns of `session` in `workspace`, before its
    /// first turn.
    pub(super) fn new_session(id: JobId, workspace: PathBuf, session: rho_sdk::SessionId) -> Self {
        Self::new(
            id,
            JobSubject::Session { session },
            JobState::Idle,
            workspace,
            None,
        )
    }

    /// A new job for a subagent run that job `parent` started in
    /// `workspace`, waiting for capacity.
    pub(super) fn new_subagent(
        parent: JobId,
        workspace: PathBuf,
        session: Option<rho_sdk::SessionId>,
    ) -> Self {
        Self::new(
            JobId::generate(),
            JobSubject::Subagent { session },
            JobState::Queued,
            workspace,
            Some(parent),
        )
    }

    /// A new job for a drive of workflow run `run` that job `parent`
    /// started in `workspace`, running from the start: its agent nodes wait
    /// for capacity, the drive does not.
    pub(super) fn new_workflow(parent: JobId, workspace: PathBuf, run: RunId) -> Self {
        Self::new(
            JobId::generate(),
            JobSubject::Workflow { workflow_run: run },
            JobState::Running,
            workspace,
            Some(parent),
        )
    }

    fn new(
        id: JobId,
        subject: JobSubject,
        state: JobState,
        workspace: PathBuf,
        parent: Option<JobId>,
    ) -> Self {
        let now = unix_now();
        Self {
            id,
            subject,
            state,
            workspace,
            parent,
            admission: Admission::Immediate,
            created_at: now,
            updated_at: now,
            outcome: None,
        }
    }
}

impl Admission {
    fn is_immediate(&self) -> bool {
        *self == Self::Immediate
    }
}

/// The current Unix time in seconds; 0 on a clock set before 1970.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_secs())
}
