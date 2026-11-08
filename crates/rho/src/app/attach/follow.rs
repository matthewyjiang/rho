//! Where an attachment stands in a session's events: which of them replay
//! what the client missed, and whether the turn it follows has ended.
//!
//! `rho attach` follows the session's turn under way, or reports its last
//! turn when none is. A reset replays the session's saved conversation,
//! then its recent events, which hold the last turn's `turn_ended` until the
//! next turn saves, so only once the replay is over does the attachment know
//! whether the turn it reports already ended. A tail replay holds only
//! events the client has not seen, so they end the turn as live ones do.

use rho_protocol::{
    Cursor, JobState, Replay, Seq, SessionAttachResponse, SessionEvent, TurnOutcome,
};

use crate::app::stream_position::{Part, StreamPosition};

/// How the turn an attachment followed ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Ending {
    /// The turn ended as `turn_ended` said.
    Turn(TurnOutcome),
    /// The daemon stopped before the turn ended.
    Interrupted,
    /// A reset's replay ended in a finished job but held no `turn_ended`,
    /// which the daemon promises it does: a daemon bug.
    Unreported,
}

/// One attachment's place in its session's events.
pub(super) struct Follow {
    position: StreamPosition,
    /// The job state the replay ends in.
    state: JobState,
    /// How the last turn the replay holds ended.
    replayed: Option<TurnOutcome>,
}

impl Follow {
    /// Starts following from the first attach's response. Returns how the
    /// turn ended when the response alone says it.
    pub(super) fn attached(response: &SessionAttachResponse) -> (Self, Option<Ending>) {
        let follow = Self {
            position: StreamPosition::attached(response),
            state: response.state,
            replayed: None,
        };
        let ending = follow.settled_by(response);
        (follow, ending)
    }

    /// Starts over from a reattach's response, keeping the cursor unless it
    /// resets. Returns how the turn ended when the response alone says it.
    pub(super) fn reattached(&mut self, response: &SessionAttachResponse) -> Option<Ending> {
        self.position.reattached(response);
        self.state = response.state;
        self.replayed = None;
        self.settled_by(response)
    }

    /// How the turn ended when `response` alone says it: a reset with
    /// nothing to replay.
    fn settled_by(&self, response: &SessionAttachResponse) -> Option<Ending> {
        match (response.replay, response.replay_events) {
            (Replay::Reset, 0) => self.settled(),
            _ => None,
        }
    }

    /// Observes the session's next event, numbered `seq`. Returns how the
    /// turn ended once it has.
    pub(super) fn observe(&mut self, seq: Seq, event: &SessionEvent) -> Option<Ending> {
        match self.position.observe(seq) {
            Part::Replayed {
                replay: Replay::Reset,
                left,
            } => {
                if let SessionEvent::TurnEnded { outcome } = event {
                    self.replayed = Some(outcome.clone());
                }
                if left == 0 {
                    self.settled()
                } else {
                    None
                }
            }
            Part::Replayed {
                replay: Replay::Tail,
                ..
            }
            | Part::Live => match event {
                SessionEvent::TurnEnded { outcome } => Some(Ending::Turn(outcome.clone())),
                SessionEvent::State {
                    state: JobState::Interrupted,
                } => Some(Ending::Interrupted),
                _ => None,
            },
        }
    }

    /// Whether the next event is one a replay holds, rather than news.
    pub(super) fn replaying(&self) -> bool {
        self.position.replaying()
    }

    /// Where a reattach resumes from.
    pub(super) fn cursor(&self) -> Option<&Cursor> {
        self.position.cursor()
    }

    /// How the job's last turn ended, once a reset's replay is over, if the
    /// job is not running one.
    fn settled(&self) -> Option<Ending> {
        match self.state {
            JobState::Completed | JobState::Failed | JobState::Cancelled | JobState::TimedOut => {
                Some(match &self.replayed {
                    Some(outcome) => Ending::Turn(outcome.clone()),
                    None => Ending::Unreported,
                })
            }
            JobState::Interrupted => Some(Ending::Interrupted),
            // A session job is idle before its first prompt: the attachment
            // waits for that turn.
            JobState::Idle | JobState::Queued | JobState::Running | JobState::WaitingForInput => {
                None
            }
            // `JobState` is non-exhaustive; a state a newer daemon sends is
            // followed until a turn ends.
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "follow_tests.rs"]
mod tests;
