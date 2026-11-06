//! A job's state and the transitions between states.
//!
//! A thread job's state is the state of its current or last turn: `idle`
//! before its first prompt, `running` while a turn runs,
//! `waiting_for_input` while that turn waits on an approval, a plan
//! approval, or a question, and then how the turn ended. A new prompt
//! starts the next turn from any ended state. `interrupted` is how a turn
//! ends when its daemon stops before the turn does.

use serde::{Deserialize, Serialize};

/// Where a job is in its life. See the module docs for what each state
/// means for a thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::app::daemon_host) enum JobState {
    /// Created; no turn has started.
    Idle,
    Running,
    /// The running turn waits for someone to answer an approval, a plan
    /// approval, or a question.
    WaitingForInput,
    Completed,
    Failed,
    Cancelled,
    /// The daemon stopped, or died, before the turn ended.
    Interrupted,
}

/// A transition the job's state does not allow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("a job cannot go from {from} to {to}")]
pub(in crate::app::daemon_host) struct TransitionRefused {
    pub(in crate::app::daemon_host) from: JobState,
    pub(in crate::app::daemon_host) to: JobState,
}

impl JobState {
    /// The state after moving to `to`, when this state allows it.
    pub(in crate::app::daemon_host) fn transition(
        self,
        to: Self,
    ) -> Result<Self, TransitionRefused> {
        use JobState::{Cancelled, Completed, Failed, Idle, Interrupted, Running, WaitingForInput};
        match (self, to) {
            // A prompt starts the next turn.
            (Idle | Completed | Failed | Cancelled | Interrupted, Running)
            // The turn asked for input, or nothing it asked is open any more.
            | (Running, WaitingForInput)
            | (WaitingForInput, Running)
            // The turn ended, waiting or not.
            | (Running | WaitingForInput, Completed | Failed | Cancelled | Interrupted) => Ok(to),
            (
                Idle | Running | WaitingForInput | Completed | Failed | Cancelled | Interrupted,
                Idle | Running | WaitingForInput | Completed | Failed | Cancelled | Interrupted,
            ) => Err(TransitionRefused { from: self, to }),
        }
    }

    /// Whether a turn is under way: running or waiting for input.
    pub(in crate::app::daemon_host) fn in_progress(self) -> bool {
        match self {
            Self::Running | Self::WaitingForInput => true,
            Self::Idle | Self::Completed | Self::Failed | Self::Cancelled | Self::Interrupted => {
                false
            }
        }
    }

    /// The state's wire name.
    pub(in crate::app::daemon_host) fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::WaitingForInput => "waiting_for_input",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }
}

impl std::fmt::Display for JobState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
