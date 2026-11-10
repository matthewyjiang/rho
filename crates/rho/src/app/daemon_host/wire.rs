//! The host's types as `rho-protocol` sends them. The registry's types stay
//! the host's own, so its record files and the wire can change apart.

use rho_protocol as wire;

use super::jobs::{JobId, JobRecord, JobState, JobSubject, Moved, TurnEnd};
use crate::app::agent_concurrency::{Budget, QueuePlace};

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

impl From<&JobState> for wire::JobState {
    fn from(state: &JobState) -> Self {
        match state {
            JobState::Idle => Self::Idle,
            JobState::Queued => Self::Queued,
            JobState::Running => Self::Running,
            JobState::WaitingForInput => Self::WaitingForInput,
            JobState::Ended(TurnEnd::Completed) => Self::Completed,
            JobState::Ended(TurnEnd::Failed(_)) => Self::Failed,
            JobState::Ended(TurnEnd::Cancelled) => Self::Cancelled,
            JobState::Ended(TurnEnd::TimedOut { .. }) => Self::TimedOut,
            JobState::Ended(TurnEnd::Interrupted) => Self::Interrupted,
        }
    }
}

/// Why a job in `state` failed, if it did.
fn failure(state: &JobState) -> Option<String> {
    match state {
        JobState::Ended(TurnEnd::Failed(error)) => Some(error.clone()),
        JobState::Idle
        | JobState::Queued
        | JobState::Running
        | JobState::WaitingForInput
        | JobState::Ended(
            TurnEnd::Completed
            | TurnEnd::Cancelled
            | TurnEnd::TimedOut { .. }
            | TurnEnd::Interrupted,
        ) => None,
    }
}

/// The event that tells attached clients where a session's job is now.
pub(super) fn state_event(moved: &Moved) -> wire::SessionEvent {
    wire::SessionEvent::State {
        state: (&moved.state).into(),
        turn: moved.turn.clone(),
    }
}

/// `record` as `rho/job/list` reports it, with where the job waits for
/// capacity, if it does.
pub(super) fn job_summary(record: JobRecord, queue: Option<QueuePlace>) -> wire::JobSummary {
    let JobRecord {
        id,
        subject,
        state,
        workspace,
        parent,
        admission: _,
        created_at,
        updated_at,
        outcome: _,
        turn: _,
    } = record;
    let (kind, session, run, workflow_run) = match subject {
        JobSubject::Session { session } => (wire::JobKind::Session, Some(session), None, None),
        JobSubject::Subagent { session, run } => (wire::JobKind::Subagent, session, run, None),
        JobSubject::Workflow { workflow_run } => (
            wire::JobKind::Workflow,
            None,
            None,
            Some(workflow_run.to_string()),
        ),
    };
    wire::JobSummary {
        id: (&id).into(),
        kind,
        state: (&state).into(),
        workspace,
        parent: parent.as_ref().map(Into::into),
        session: session.map(session_to_wire),
        run,
        workflow_run,
        created_at,
        updated_at,
        error: failure(&state),
        queue: queue.map(Into::into),
    }
}

impl From<Budget> for wire::Budget {
    fn from(budget: Budget) -> Self {
        match budget {
            Budget::AgentConcurrency => Self::AgentConcurrency,
            Budget::ClaudeAgentConcurrency => Self::ClaudeAgentConcurrency,
        }
    }
}

impl From<QueuePlace> for wire::QueuePlace {
    fn from(place: QueuePlace) -> Self {
        let QueuePlace {
            budget,
            limit,
            position,
        } = place;
        // Both are bounded far below `u32::MAX`: the limit by
        // `MAX_AGENT_CONCURRENCY`, the position by the runs one daemon
        // spawns.
        let count = |value: usize| u32::try_from(value).unwrap_or(u32::MAX);
        Self {
            budget: budget.into(),
            limit: count(limit),
            position: count(position),
        }
    }
}
