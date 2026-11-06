//! One loaded session's event stream (#1657): every wire event gets the
//! stream's next `seq`, goes to every attached client, and stays in a
//! buffer a client reattaching with its cursor resumes from.
//!
//! A client that attaches without a usable cursor gets a reset: the
//! session's saved transcript, as wire events, then the turn in progress
//! from the buffer. The feed keeps that transcript itself, extended with
//! each turn's saved rows when the turn's end is recorded, so an attach
//! never waits for the turn that holds the session's runtime. The
//! transcript and the point where the turn in progress begins move together
//! under the feed's lock, the same lock an attach reads both under, so a turn
//! that saves while a client attaches is neither sent twice nor skipped.

use std::{
    collections::VecDeque,
    sync::{Mutex, MutexGuard, PoisonError},
};

use rho_protocol::{
    self as wire, Cursor, DetachReason, Published, Publisher, Replay, Seq, StreamId, Subscription,
};

/// How many events one client may fall behind before the daemon drops its
/// subscription; it then reattaches with its cursor.
//
// Measured: a real turn streamed 21 text deltas per second on average and
// 24 in its busiest second, about 104 bytes each, so this covers a client
// that stops reading for about 170 s of the busiest streaming measured.
const SUBSCRIBER_BACKLOG: usize = 4096;

/// How many bytes of encoded events the buffer keeps for reattaching
/// clients.
//
// Measured over 1,697 real turns in 229 saved sessions: a turn's saved
// content was at most 1.57 MB (p99 1.05 MB). Live events add JSON framing
// per delta, so this keeps the largest turn seen about ten times over.
// Evicting events of the turn in progress is a tripwire: it is logged, and a
// reset replay says the turn's start is unavailable until the turn saves.
const REPLAY_BUFFER_BYTES: usize = 16 * 1024 * 1024;

/// Tells a client reset into a turn whose start the buffer no longer holds.
const TURN_START_EVICTED: &str =
    "the start of this turn is no longer buffered; it appears once the turn is saved";

/// A loaded session's event stream.
pub(super) struct Feed {
    state: Mutex<FeedState>,
}

struct FeedState {
    stream: StreamId,
    /// The seq of the last event sent; `Seq(0)` before the first.
    last: Seq,
    buffer: VecDeque<Buffered>,
    buffered_bytes: usize,
    /// The seq the turn in progress began after. Events after it are the
    /// turn's; the transcript holds everything up to it.
    turn_start: Seq,
    /// Whether the buffer evicted events of the turn in progress.
    turn_start_evicted: bool,
    /// The session's saved transcript as wire events, up to `turn_start`.
    transcript: Vec<wire::SessionEvent>,
    /// How many saved display rows `transcript` replays.
    saved_rows: usize,
    subscribers: Vec<Publisher>,
    /// Set by [`Feed::close`]: a client attaching later is detached at once.
    closed: Option<DetachReason>,
}

struct Buffered {
    seq: Seq,
    event: wire::SessionEvent,
    bytes: usize,
}

/// What an attach sends, before the session's live events.
pub(super) struct Attached {
    pub(super) stream: StreamId,
    pub(super) replay: Replay,
    pub(super) subscription: Subscription,
}

impl Feed {
    /// The stream of a session whose saved transcript replays as
    /// `transcript`, from `saved_rows` display rows.
    pub(super) fn new(transcript: Vec<wire::SessionEvent>, saved_rows: usize) -> Self {
        Self {
            state: Mutex::new(FeedState {
                stream: StreamId::new(uuid::Uuid::new_v4().to_string()),
                last: Seq(0),
                buffer: VecDeque::new(),
                buffered_bytes: 0,
                turn_start: Seq(0),
                turn_start_evicted: false,
                transcript,
                saved_rows,
                subscribers: Vec::new(),
                closed: None,
            }),
        }
    }

    /// Sends `events` to every attached client, in order, and buffers them.
    pub(super) fn publish(&self, events: impl IntoIterator<Item = wire::SessionEvent>) {
        let mut state = self.state();
        for event in events {
            state.push(event);
        }
    }

    /// Marks where a new turn begins: events from here on are the turn's
    /// until [`Self::end_turn`].
    pub(super) fn begin_turn(&self) {
        let mut state = self.state();
        state.turn_start = state.last;
        state.turn_start_evicted = false;
    }

    /// Records that the turn in progress saved: `replay_rows` turns the
    /// saved display rows from index `saved_rows` on into wire events,
    /// which join the transcript, and the next turn begins after the last
    /// event sent. `rows` is how many display rows the session has now.
    pub(super) fn end_turn(
        &self,
        rows: usize,
        replay_rows: impl FnOnce(usize) -> Vec<wire::SessionEvent>,
    ) {
        let mut state = self.state();
        let saved = state.saved_rows;
        state.transcript.extend(replay_rows(saved));
        state.saved_rows = rows;
        state.turn_start = state.last;
        state.turn_start_evicted = false;
    }

    /// Subscribes a client resuming from `after`. A cursor this stream
    /// still buffers resumes with the events after it; anything else
    /// resets to the transcript and the turn in progress. Once the feed is
    /// closed, the client gets that replay, then is detached.
    pub(super) fn attach(&self, after: Option<&Cursor>) -> Attached {
        let mut state = self.state();
        let (replay, events) = state.replay_from(after);
        let (publisher, subscription) = rho_protocol::subscription(SUBSCRIBER_BACKLOG, events);
        match state.closed {
            Some(reason) => publisher.close(reason),
            None => state.subscribers.push(publisher),
        }
        Attached {
            stream: state.stream.clone(),
            replay,
            subscription,
        }
    }

    /// Ends every client's subscription with `reason`, and detaches each
    /// client that attaches later with it. Closing again keeps the first
    /// reason.
    pub(super) fn close(&self, reason: DetachReason) {
        let mut state = self.state();
        let reason = *state.closed.get_or_insert(reason);
        for publisher in std::mem::take(&mut state.subscribers) {
            publisher.close(reason);
        }
    }

    fn state(&self) -> MutexGuard<'_, FeedState> {
        // Each change completes before the lock is released except a push
        // that panicked mid-fan-out, which at worst leaves a subscriber
        // that the next push drops.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl FeedState {
    fn push(&mut self, event: wire::SessionEvent) {
        let seq = Seq(self.last.0 + 1);
        self.last = seq;
        self.subscribers.retain(
            |publisher| match publisher.try_publish(seq, event.clone()) {
                Ok(()) => true,
                Err(Published::Lagged | Published::Gone) => false,
            },
        );
        // Encoding never fails for wire events, which hold only JSON data.
        let bytes = serde_json::to_vec(&event).map_or(0, |encoded| encoded.len());
        self.buffer.push_back(Buffered { seq, event, bytes });
        self.buffered_bytes += bytes;
        while self.buffered_bytes > REPLAY_BUFFER_BYTES {
            let Some(evicted) = self.buffer.pop_front() else {
                break;
            };
            self.buffered_bytes -= evicted.bytes;
            if evicted.seq > self.turn_start && !self.turn_start_evicted {
                self.turn_start_evicted = true;
                tracing::warn!(
                    limit = REPLAY_BUFFER_BYTES,
                    "a turn outgrew REPLAY_BUFFER_BYTES; clients that reset into it miss its start until it saves"
                );
            }
        }
    }

    /// What a client resuming from `after` is sent before the live events.
    fn replay_from(&self, after: Option<&Cursor>) -> (Replay, Vec<(Seq, wire::SessionEvent)>) {
        match after.filter(|cursor| self.resumes(cursor)) {
            Some(cursor) => (Replay::Tail, self.events_after(cursor.seq)),
            None => (Replay::Reset, self.reset_events()),
        }
    }

    /// Whether `cursor` is a point of this stream the buffer resumes from:
    /// every event after it is still buffered.
    fn resumes(&self, cursor: &Cursor) -> bool {
        let oldest = self
            .buffer
            .front()
            .map_or(Seq(self.last.0 + 1), |event| event.seq);
        cursor.stream == self.stream && cursor.seq <= self.last && cursor.seq.0 + 1 >= oldest.0
    }

    fn events_after(&self, seq: Seq) -> Vec<(Seq, wire::SessionEvent)> {
        self.buffer
            .iter()
            .filter(|event| event.seq > seq)
            .map(|event| (event.seq, event.event.clone()))
            .collect()
    }

    /// The transcript, numbered as the point the turn in progress began
    /// after, then the turn's buffered events.
    fn reset_events(&self) -> Vec<(Seq, wire::SessionEvent)> {
        let mut events: Vec<_> = self
            .transcript
            .iter()
            .map(|event| (self.turn_start, event.clone()))
            .collect();
        if self.turn_start_evicted {
            events.push((
                self.turn_start,
                wire::SessionEvent::Notice {
                    text: TURN_START_EVICTED.to_owned(),
                },
            ));
        }
        events.extend(self.events_after(self.turn_start));
        events
    }
}

#[cfg(test)]
#[path = "feed_tests.rs"]
mod tests;
