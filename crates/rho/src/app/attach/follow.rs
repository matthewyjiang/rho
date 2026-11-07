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
    Cursor, JobState, Replay, Seq, SessionAttachResponse, SessionEvent, StreamId, TurnOutcome,
};

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
    stream: StreamId,
    /// The kind of the last replay.
    replay: Replay,
    /// Events of the last replay still to come.
    replaying: u64,
    /// The job state the replay ends in.
    state: JobState,
    /// How the last turn the replay holds ended.
    replayed: Option<TurnOutcome>,
    /// The highest seq seen, which a reattach resumes after.
    cursor: Option<Cursor>,
}

impl Follow {
    /// Starts following from the first attach's response. Returns how the
    /// turn ended when the response alone says it.
    pub(super) fn attached(response: &SessionAttachResponse) -> (Self, Option<Ending>) {
        let mut follow = Self {
            stream: response.stream.clone(),
            replay: response.replay,
            replaying: 0,
            state: JobState::Idle,
            replayed: None,
            cursor: None,
        };
        let ending = follow.reattached(response);
        (follow, ending)
    }

    /// Starts over from a reattach's response, keeping the cursor unless it
    /// resets. Returns how the turn ended when the response alone says it.
    pub(super) fn reattached(&mut self, response: &SessionAttachResponse) -> Option<Ending> {
        self.stream = response.stream.clone();
        self.state = response.state;
        self.replayed = None;
        self.replay = response.replay;
        self.replaying = response.replay_events;
        match response.replay {
            Replay::Reset => {
                // A reset starts the client over, cursor included.
                self.cursor = None;
                if self.replaying == 0 {
                    self.settled()
                } else {
                    None
                }
            }
            Replay::Tail => None,
        }
    }

    /// Observes the session's next event, numbered `seq`. Returns how the
    /// turn ended once it has.
    pub(super) fn observe(&mut self, seq: Seq, event: &SessionEvent) -> Option<Ending> {
        // A reset's saved conversation carries seq 0, which is no position
        // to resume after.
        if seq > Seq(0) {
            self.cursor = Some(Cursor {
                stream: self.stream.clone(),
                seq,
            });
        }
        let replayed = self.replaying > 0;
        self.replaying = self.replaying.saturating_sub(1);
        if replayed && self.replay == Replay::Reset {
            if let SessionEvent::TurnEnded { outcome } = event {
                self.replayed = Some(outcome.clone());
            }
            return if self.replaying == 0 {
                self.settled()
            } else {
                None
            };
        }
        match event {
            SessionEvent::TurnEnded { outcome } => Some(Ending::Turn(outcome.clone())),
            SessionEvent::State {
                state: JobState::Interrupted,
            } => Some(Ending::Interrupted),
            _ => None,
        }
    }

    /// Whether the next event is one a replay holds, rather than news.
    pub(super) fn replaying(&self) -> bool {
        self.replaying > 0
    }

    /// Where a reattach resumes from.
    pub(super) fn cursor(&self) -> Option<&Cursor> {
        self.cursor.as_ref()
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
