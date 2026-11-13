//! Where a client stands in a daemon session's event stream: the cursor a
//! reattach resumes after, and which events the last attach's replay holds.

use rho_protocol::{Cursor, Replay, Seq, SessionAttachResponse, StreamId};

/// Which part of an attachment an event belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Part {
    /// The attach's replay, a `replay` one, with `left` of its events still
    /// to come.
    Replayed { replay: Replay, left: u64 },
    /// News.
    Live,
}

/// One client's place in a session's events.
#[derive(Debug)]
pub(crate) struct StreamPosition {
    stream: StreamId,
    /// The kind of the last replay.
    replay: Replay,
    /// Events of the last replay still to come.
    replaying: u64,
    /// The highest seq seen, which a reattach resumes after.
    cursor: Option<Cursor>,
}

impl StreamPosition {
    /// The position at the start of a first attach's `response`.
    pub(crate) fn attached(response: &SessionAttachResponse) -> Self {
        Self {
            stream: response.stream.clone(),
            replay: response.replay,
            replaying: response.replay_events,
            cursor: None,
        }
    }

    /// Starts over from a reattach's `response`, keeping the cursor unless
    /// it resets: a reset starts the client over, cursor included.
    pub(crate) fn reattached(&mut self, response: &SessionAttachResponse) {
        let cursor = match response.replay {
            Replay::Tail => self.cursor.take(),
            Replay::Reset => None,
        };
        *self = Self {
            cursor,
            ..Self::attached(response)
        };
    }

    /// Steps past the session's next event, numbered `seq`, and says which
    /// part of the attachment it belongs to.
    pub(crate) fn observe(&mut self, seq: Seq) -> Part {
        // A reset's saved conversation carries seq 0, which is no position
        // to resume after.
        if seq > Seq(0) {
            self.cursor = Some(Cursor {
                stream: self.stream.clone(),
                seq,
            });
        }
        if self.replaying == 0 {
            return Part::Live;
        }
        self.replaying -= 1;
        Part::Replayed {
            replay: self.replay,
            left: self.replaying,
        }
    }

    /// Whether the next event is one the replay holds, rather than news.
    pub(crate) fn replaying(&self) -> bool {
        self.replaying > 0
    }

    /// Where a reattach resumes from.
    pub(crate) fn cursor(&self) -> Option<&Cursor> {
        self.cursor.as_ref()
    }
}

#[cfg(test)]
#[path = "position_tests.rs"]
mod tests;
