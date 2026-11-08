//! What a reset's replay tells a client of its session's last turn.
//!
//! A reset replays the session's saved conversation, then its recent
//! events, which hold the last turn's `turn_ended` until the next turn
//! saves, so only once the replay is over does a client know whether that
//! turn ended; the job state the attach answered with tells it alongside.
//! Each client settles what it follows from both: `rho acp` the prompt it
//! armed (`acp/sessions/pump.rs`), `rho attach` its exit
//! (`attach/follow.rs`). A tail replay holds only events the client missed,
//! which it handles as live.

use rho_protocol::{JobState, MessageId, SessionEvent, TurnOutcome};

/// The last turn a replay holds, as far as its user messages and turn ends
/// tell.
#[derive(Debug)]
pub(crate) enum ReplayedTurn {
    /// No turn.
    Unseen,
    /// The turn that began with this user message, not ended yet.
    Began(MessageId),
    /// A turn that ended this way.
    Ended(TurnOutcome),
}

/// A reset's replay, as far as a client has read it.
#[derive(Debug)]
pub(crate) struct ResetReplay {
    /// The job state the replay ends in, as the attach answered.
    pub(crate) state: JobState,
    /// The last turn the events read so far hold.
    pub(crate) turn: ReplayedTurn,
}

impl ResetReplay {
    /// The replay of an attach that answered with the job in `state`,
    /// before its first event.
    pub(crate) fn new(state: JobState) -> Self {
        Self {
            state,
            turn: ReplayedTurn::Unseen,
        }
    }

    /// Reads the replay's next event.
    pub(crate) fn observe(&mut self, event: &SessionEvent) {
        match event {
            SessionEvent::UserMessage { message, .. } => {
                self.turn = ReplayedTurn::Began(message.clone());
            }
            SessionEvent::TurnEnded { outcome } => {
                self.turn = ReplayedTurn::Ended(outcome.clone());
            }
            _ => {}
        }
    }
}
