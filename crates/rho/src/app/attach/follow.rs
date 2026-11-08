//! Where an attachment stands in a session's events, and whether the turn
//! it follows has ended.
//!
//! `rho attach` follows the session's turn under way, or reports its last
//! turn when none is. A reset's replay is history, so it ends the turn only
//! once it is over, by what it held of the last turn and the job's state
//! ([`ResetReplay`]). A tail replay holds only events the client has not
//! seen, so they end the turn as live ones do.

use rho_protocol::{
    Cursor, JobState, Replay, Seq, SessionAttachResponse, SessionEvent, TurnOutcome,
};

use crate::app::daemon_client::{Part, ReplayedTurn, ResetReplay, StreamPosition};

/// How the turn an attachment followed ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Ending {
    /// The turn ended as `turn_ended` said.
    Turn(TurnOutcome),
    /// The daemon stopped before the turn ended.
    Interrupted,
    /// A reset's replay ended in a finished job, but its last turn held no
    /// `turn_ended`, which the daemon promises it does: a daemon bug.
    Unreported,
}

/// One attachment's place in its session's events.
pub(super) struct Follow {
    position: StreamPosition,
    /// The last attach's replay, read as far as its events came; what it
    /// holds counts only when the replay is a reset.
    replay: ResetReplay,
}

impl Follow {
    /// Starts following from the first attach's response. Returns how the
    /// turn ended when the response alone says it.
    pub(super) fn attached(response: &SessionAttachResponse) -> (Self, Option<Ending>) {
        let follow = Self {
            position: StreamPosition::attached(response),
            replay: ResetReplay::new(response.state),
        };
        let ending = follow.settled_by(response);
        (follow, ending)
    }

    /// Starts over from a reattach's response, keeping the cursor unless it
    /// resets. Returns how the turn ended when the response alone says it.
    pub(super) fn reattached(&mut self, response: &SessionAttachResponse) -> Option<Ending> {
        self.position.reattached(response);
        self.replay = ResetReplay::new(response.state);
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
                self.replay.observe(event);
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
        match self.replay.state {
            JobState::Completed | JobState::Failed | JobState::Cancelled | JobState::TimedOut => {
                Some(match &self.replay.turn {
                    ReplayedTurn::Ended(outcome) => Ending::Turn(outcome.clone()),
                    ReplayedTurn::Unseen | ReplayedTurn::Began(_) => Ending::Unreported,
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
