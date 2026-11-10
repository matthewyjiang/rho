//! A job's state and the events that move it.
//!
//! A session job's state is the state of its current or last turn: `idle`
//! before its first prompt, `running` while a turn runs,
//! `waiting_for_input` while that turn waits on an approval, a plan
//! approval, or a question, and then how the turn ended. A new prompt
//! starts the next turn from any ended state. `interrupted` is how a turn
//! ends when its daemon stops before the turn does. The turns of a session
//! another job started (#1714) are `queued` for capacity in the daemon-wide
//! budget before they run, as subagent runs are.
//!
//! A subagent job is one run, which is one turn: `queued` while it waits
//! for capacity in the daemon-wide budget, then `running`, then how it
//! ended. It never waits for input itself: its questions go to its parent.
//!
//! A workflow job is one drive of a workflow run: `running`, then how the
//! drive ended. Its agent nodes are subagent jobs whose parent it is.
//!
//! A state changes only through a [`JobEvent`], so a failed turn always
//! carries why it failed.

use serde::{Deserialize, Serialize};

/// Where a job is in its life. See the module docs for what each state
/// means for a session job.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::app::daemon_host) enum JobState {
    /// Created; no turn has started.
    Idle,
    /// A subagent run, or a turn of a session another job started,
    /// waiting for capacity.
    Queued,
    Running,
    /// The running turn waits for someone to answer an approval, a plan
    /// approval, or a question.
    WaitingForInput,
    /// The last turn ended.
    Ended(TurnEnd),
}

/// How a turn ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::app::daemon_host) enum TurnEnd {
    Completed,
    /// A client cancelled it.
    Cancelled,
    /// The daemon cancelled it at the limit its prompt set, `timeout_ms`.
    TimedOut {
        timeout_ms: u64,
    },
    /// The daemon stopped, or died, before the turn ended.
    Interrupted,
    /// The turn failed, for this reason.
    Failed(String),
}

/// Something that happened to a job's turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app::daemon_host) enum JobEvent {
    /// A prompt started a new turn.
    TurnStarted,
    /// A prompt started a new turn that waits for capacity first.
    TurnQueued,
    /// A queued run or turn got capacity and started. Only a queued job
    /// takes it, so a run that starts after the daemon's stop recorded its
    /// job interrupted cannot revive the job.
    Admitted,
    /// The turn asked for an approval, a plan approval, or a question while
    /// none was open.
    InputOpened,
    /// Nothing the turn asked is open any more.
    InputSettled,
    TurnEnded(TurnEnd),
}

/// An event the job's state does not allow.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("a job that is {from} cannot take {event}")]
pub(in crate::app::daemon_host) struct TransitionRefused {
    pub(in crate::app::daemon_host) from: JobState,
    pub(in crate::app::daemon_host) event: JobEvent,
}

impl JobState {
    /// The state after `event`, when this state allows it.
    pub(in crate::app::daemon_host) fn after(
        &self,
        event: JobEvent,
    ) -> Result<Self, TransitionRefused> {
        use JobEvent::{Admitted, InputOpened, InputSettled, TurnEnded, TurnQueued, TurnStarted};
        use JobState::{Ended, Idle, Queued, Running, WaitingForInput};
        match (self, event) {
            (Idle | Ended(_), TurnStarted) => Ok(Running),
            (Idle | Ended(_), TurnQueued) => Ok(Queued),
            (Queued, Admitted) => Ok(Running),
            (Running, InputOpened) => Ok(WaitingForInput),
            (WaitingForInput, InputSettled) => Ok(Running),
            (Queued | Running | WaitingForInput, TurnEnded(end)) => Ok(Ended(end)),
            (
                Idle | Queued | Running | WaitingForInput | Ended(_),
                event @ (TurnStarted | TurnQueued | Admitted | InputOpened | InputSettled
                | TurnEnded(_)),
            ) => Err(TransitionRefused {
                from: self.clone(),
                event,
            }),
        }
    }

    /// Whether a turn is under way: queued, running, or waiting for input.
    pub(in crate::app::daemon_host) fn in_progress(&self) -> bool {
        match self {
            Self::Queued | Self::Running | Self::WaitingForInput => true,
            Self::Idle | Self::Ended(_) => false,
        }
    }

    /// The state's name, as logs and errors show it.
    pub(in crate::app::daemon_host) fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Queued => "queued",
            Self::Running => "running",
            Self::WaitingForInput => "waiting_for_input",
            Self::Ended(TurnEnd::Completed) => "completed",
            Self::Ended(TurnEnd::Cancelled) => "cancelled",
            Self::Ended(TurnEnd::TimedOut { .. }) => "timed_out",
            Self::Ended(TurnEnd::Interrupted) => "interrupted",
            Self::Ended(TurnEnd::Failed(_)) => "failed",
        }
    }
}

impl std::fmt::Display for JobState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::fmt::Display for JobEvent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::TurnStarted => "turn_started",
            Self::TurnQueued => "turn_queued",
            Self::Admitted => "admitted",
            Self::InputOpened => "input_opened",
            Self::InputSettled => "input_settled",
            Self::TurnEnded(_) => "turn_ended",
        })
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
