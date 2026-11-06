//! The host's types as `rho-protocol` sends them. The registry's types stay
//! the host's own, so its record files and the wire can change apart.

use rho_protocol as wire;

use super::jobs::{JobId, JobKind, JobRecord, JobState, TurnEnd};

/// A session id as the wire sends it.
pub(super) fn session_to_wire(id: rho_sdk::SessionId) -> wire::SessionId {
    wire::SessionId::new(id.into_string())
}

/// The session id a request named, or `InvalidId` for a string no session
/// id can be.
pub(super) fn session_from_wire(
    id: &wire::SessionId,
) -> Result<rho_sdk::SessionId, rho_sdk::InvalidId> {
    rho_sdk::SessionId::from_string(id.as_str())
}

impl From<&JobId> for wire::JobId {
    fn from(id: &JobId) -> Self {
        Self::new(id.as_str())
    }
}

impl From<JobKind> for wire::JobKind {
    fn from(kind: JobKind) -> Self {
        match kind {
            JobKind::Session => Self::Session,
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

/// Why a job in `state` failed, if it did.
fn failure(state: &JobState) -> Option<String> {
    match state {
        JobState::Ended(TurnEnd::Failed(error)) => Some(error.clone()),
        JobState::Idle
        | JobState::Running
        | JobState::WaitingForInput
        | JobState::Ended(TurnEnd::Completed | TurnEnd::Cancelled | TurnEnd::Interrupted) => None,
    }
}

/// The event that tells attached clients a session's job is now in `state`.
pub(super) fn state_event(state: &JobState) -> wire::SessionEvent {
    wire::SessionEvent::State {
        state: state.into(),
        error: failure(state),
    }
}

impl From<JobRecord> for wire::JobSummary {
    fn from(record: JobRecord) -> Self {
        let JobRecord {
            id,
            kind,
            state,
            workspace,
            parent,
            session,
            created_at,
            updated_at,
        } = record;
        Self {
            id: (&id).into(),
            kind: kind.into(),
            state: (&state).into(),
            workspace,
            parent: parent.as_ref().map(Into::into),
            session: session_to_wire(session),
            created_at,
            updated_at,
            error: failure(&state),
        }
    }
}
