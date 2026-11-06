//! The host's types as `rho-protocol` sends them. The registry's types stay
//! the host's own, so its record files and the wire can change apart.

use rho_protocol as wire;

use super::jobs::{JobId, JobKind, JobRecord, JobState, TurnEnd};

impl From<&JobId> for wire::JobId {
    fn from(id: &JobId) -> Self {
        Self::new(id.as_str())
    }
}

impl From<JobKind> for wire::JobKind {
    fn from(kind: JobKind) -> Self {
        match kind {
            JobKind::Thread => Self::Thread,
        }
    }
}

impl From<&JobState> for wire::JobState {
    fn from(state: &JobState) -> Self {
        match state {
            JobState::Idle => Self::Idle,
            JobState::Running => Self::Running,
            JobState::WaitingForInput => Self::WaitingForInput,
            JobState::Ended(TurnEnd::Completed) => Self::Completed,
            JobState::Ended(TurnEnd::Failed(_)) => Self::Failed,
            JobState::Ended(TurnEnd::Cancelled) => Self::Cancelled,
            JobState::Ended(TurnEnd::Interrupted) => Self::Interrupted,
        }
    }
}

impl From<JobRecord> for wire::JobSummary {
    fn from(record: JobRecord) -> Self {
        let JobRecord {
            id,
            kind,
            state,
            project_root,
            parent,
            session_id,
            created_at,
            updated_at,
        } = record;
        Self {
            id: (&id).into(),
            kind: kind.into(),
            state: (&state).into(),
            project_root,
            parent: parent.as_ref().map(Into::into),
            session_id: session_id.into_string(),
            created_at,
            updated_at,
            error: match state {
                JobState::Ended(TurnEnd::Failed(error)) => Some(error),
                JobState::Idle
                | JobState::Running
                | JobState::WaitingForInput
                | JobState::Ended(TurnEnd::Completed | TurnEnd::Cancelled | TurnEnd::Interrupted) => {
                    None
                }
            },
        }
    }
}
