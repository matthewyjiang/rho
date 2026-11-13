//! Where an attachment stands in a session's events, and whether the turn
//! it follows has ended.
//!
//! `rho attach` follows one turn, by its id: the one the attach found
//! under way, the session's last when none is, or, on a session that has
//! run none, the first to start. That turn's `turn_ended` ends the
//! attachment, wherever it arrives: live, in a tail replay, or in a
//! reset's, which is history and so ends it only once it is over.

use rho_protocol::{
    Cursor, JobState, Replay, Seq, SessionAttachResponse, SessionEvent, TurnId, TurnOutcome,
};

use crate::app::daemon_client::{Part, StreamPosition};

/// How the turn an attachment followed ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Ending {
    /// The turn ended as `turn_ended` said.
    Turn(TurnOutcome),
    /// The daemon stopped before the turn ended.
    Interrupted,
    /// A reset's replay ended in a finished job, but held no `turn_ended`
    /// of the job's last turn, which the daemon promises it does: a daemon
    /// bug.
    Unreported,
}

/// One attachment's place in its session's events.
pub(super) struct Follow {
    position: StreamPosition,
    /// The turn followed; `None` until a session that has run none starts
    /// its first.
    turn: Option<TurnId>,
    /// The job state the last reset ends in, as its attach answered.
    state: JobState,
    /// How the followed turn ended, as a reset's replay held it, until
    /// that replay is over.
    replayed: Option<Ending>,
}

impl Follow {
    /// Starts following from the first attach's response. Returns how the
    /// turn ended when the response alone says it.
    pub(super) fn attached(response: &SessionAttachResponse) -> (Self, Option<Ending>) {
        let mut follow = Self {
            position: StreamPosition::attached(response),
            turn: response.turn.clone(),
            state: response.state,
            replayed: None,
        };
        let ending = follow.settled_by(response);
        (follow, ending)
    }

    /// Starts over from a reattach's response, keeping the cursor unless it
    /// resets. A tail goes on with the turn followed; a reset follows the
    /// turn it ends in. Returns how the turn ended when the response alone
    /// says it.
    pub(super) fn reattached(&mut self, response: &SessionAttachResponse) -> Option<Ending> {
        self.position.reattached(response);
        if response.replay == Replay::Reset {
            self.turn.clone_from(&response.turn);
            self.state = response.state;
            self.replayed = None;
        }
        self.settled_by(response)
    }

    /// How the turn ended when `response` alone says it: a reset with
    /// nothing to replay.
    fn settled_by(&mut self, response: &SessionAttachResponse) -> Option<Ending> {
        match (response.replay, response.replay_events) {
            (Replay::Reset, 0) => self.settled(),
            _ => None,
        }
    }

    /// Observes the session's next event, numbered `seq`. Returns how the
    /// turn ended once it has.
    pub(super) fn observe(&mut self, seq: Seq, event: &SessionEvent) -> Option<Ending> {
        let part = self.position.observe(seq);
        let ending = match event {
            SessionEvent::TurnEnded { turn, outcome } if self.follows(turn) => {
                Some(Ending::Turn(outcome.clone()))
            }
            SessionEvent::State {
                state: JobState::Interrupted,
                turn: Some(turn),
            } if self.follows(turn) => Some(Ending::Interrupted),
            // The first turn of a session that had run none.
            SessionEvent::State {
                turn: Some(turn), ..
            } if self.turn.is_none() => {
                self.turn = Some(turn.clone());
                None
            }
            _ => None,
        };
        match part {
            Part::Replayed {
                replay: Replay::Reset,
                left,
            } => {
                self.replayed = ending.or(self.replayed.take());
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
            | Part::Live => ending,
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

    fn follows(&self, turn: &TurnId) -> bool {
        self.turn.as_ref() == Some(turn)
    }

    /// How the followed turn ended, once a reset's replay is over, if the
    /// job is not running it.
    fn settled(&mut self) -> Option<Ending> {
        match self.state {
            JobState::Completed | JobState::Failed | JobState::Cancelled | JobState::TimedOut => {
                Some(self.replayed.take().unwrap_or(Ending::Unreported))
            }
            JobState::Interrupted => Some(Ending::Interrupted),
            // A session job is idle before its first prompt: the attachment
            // waits for that turn.
            JobState::Idle | JobState::Queued | JobState::Running | JobState::WaitingForInput => {
                None
            }
            // `JobState` is non-exhaustive; a state a newer daemon sends is
            // followed until the turn ends.
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "follow_tests.rs"]
mod tests;
