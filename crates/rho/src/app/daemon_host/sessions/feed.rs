//! One loaded session's event stream (#1657): every wire event gets the
//! stream's next `seq`, goes to every attached client, and stays in a
//! buffer a client reattaching with its cursor resumes from.
//!
//! A client that attaches without a usable cursor gets a reset: the
//! session's saved transcript, as wire events, then the turn in progress
//! from the buffer. The feed keeps that transcript itself, extended with
//! each turn's saved rows as the turn saves, so an attach never waits for
//! the turn that holds the session's runtime. The transcript and the point
//! where the turn in progress begins move together under the feed's lock,
//! the same lock an attach reads both under, so a turn that saves while a
//! client attaches is neither sent twice nor skipped. A turn that could not
//! save stays in the buffer, so resets replay it live until a later turn
//! saves it.
//!
//! The feed also keeps the session's options, as the last
//! [`wire::SessionEvent::OptionsChanged`] left them, so an attach reports
//! them without waiting for the runtime either.
//!
//! A reset's transcript events carry seq 0, except the last, which carries
//! the seq the turn in progress began after. A client's cursor is the
//! highest seq it has seen, so one that detaches partway through a
//! transcript holds no cursor past 0 and resets again.

use std::{
    collections::{BTreeMap, VecDeque},
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
    /// The job state the last state event sent, so an attach reports the
    /// state its replay ends in.
    job: wire::JobState,
    /// The input requests opened and not yet resolved, so an attach reports
    /// them even once the buffer no longer holds the events that opened
    /// them.
    inputs: BTreeMap<wire::InputRequestId, wire::InputRequest>,
    /// The session's title: the saved one, then the last title change sent,
    /// so an attach reports it though no transcript holds it.
    title: Option<String>,
    /// The session's options, as the last options event sent them.
    options: wire::SessionOptions,
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
    /// How many events the replay holds.
    pub(super) replay_events: u64,
    /// The job state as of the attach, which the replay ends in.
    pub(super) job: wire::JobState,
    /// The input requests open as of the attach, oldest first.
    pub(super) inputs: Vec<wire::InputRequest>,
    /// The session's title as of the attach, if it has one.
    pub(super) title: Option<String>,
    /// The session's options as of the attach.
    pub(super) options: wire::SessionOptions,
    pub(super) subscription: Subscription,
}

impl Feed {
    /// A stream with nothing saved yet, its job in `job`: idle for a new
    /// session, its last turn's end for a reopened one (#1661), whose saved
    /// conversation [`Self::end_turn`] then adds. `title` is the one the
    /// session has saved, if any; the session opened with `options`.
    pub(super) fn new(
        job: wire::JobState,
        title: Option<String>,
        options: wire::SessionOptions,
    ) -> Self {
        Self {
            state: Mutex::new(FeedState {
                stream: StreamId::new(uuid::Uuid::new_v4().to_string()),
                last: Seq(0),
                buffer: VecDeque::new(),
                buffered_bytes: 0,
                turn_start: Seq(0),
                turn_start_evicted: false,
                transcript: Vec::new(),
                job,
                inputs: BTreeMap::new(),
                title,
                options,
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

    /// Tells attached clients the session's options are now `options`,
    /// unless they already are.
    pub(super) fn update_options(&self, mut options: wire::SessionOptions) {
        let mut state = self.state();
        // The goal controller changes independently of runtime settings. Never
        // overwrite a concurrent clear with an older runtime snapshot.
        options.goal.clone_from(&state.options.goal);
        if state.options != options {
            state.push(wire::SessionEvent::OptionsChanged { options });
        }
    }

    /// Publish the goal under the same lock as runtime options and attach.
    pub(super) fn update_goal(&self, goal: Option<Box<wire::SessionGoal>>) {
        let mut state = self.state();
        let mut options = state.options.clone();
        options.goal = goal;
        if state.options != options {
            state.push(wire::SessionEvent::OptionsChanged { options });
        }
    }

    /// The session's options now.
    pub(super) fn options(&self) -> wire::SessionOptions {
        self.state().options.clone()
    }

    /// The live events the buffer holds, oldest first.
    #[cfg(test)]
    pub(super) fn buffered_for_tests(&self) -> Vec<wire::SessionEvent> {
        let state = self.state();
        state
            .buffer
            .iter()
            .map(|buffered| buffered.event.clone())
            .collect()
    }

    /// Records that the turns since the last save saved as `saved`, their
    /// replay from the saved rows: it joins the transcript, and resets stop
    /// replaying those turns' live events.
    pub(super) fn end_turn(&self, saved: Vec<wire::SessionEvent>) {
        let saved = saved
            .into_iter()
            .filter(|event| encoded_len(event).is_some());
        let mut state = self.state();
        state.transcript.extend(saved);
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
        // A replay is in memory, so far below `u64::MAX` events.
        let replay_events = u64::try_from(events.len()).unwrap_or(u64::MAX);
        let (publisher, subscription) = rho_protocol::subscription(SUBSCRIBER_BACKLOG, events);
        // Clients that left an idle session would otherwise stay until the
        // next event.
        state.subscribers.retain(|publisher| !publisher.is_gone());
        match state.closed {
            Some(reason) => publisher.close(reason),
            None => state.subscribers.push(publisher),
        }
        Attached {
            stream: state.stream.clone(),
            replay,
            replay_events,
            job: state.job,
            inputs: state.inputs.values().cloned().collect(),
            title: state.title.clone(),
            options: state.options.clone(),
            subscription,
        }
    }

    /// Whether a client still follows the session's events.
    pub(super) fn followed(&self) -> bool {
        let mut state = self.state();
        state.subscribers.retain(|publisher| !publisher.is_gone());
        !state.subscribers.is_empty()
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
        let Some(bytes) = encoded_len(&event) else {
            return;
        };
        match &event {
            wire::SessionEvent::State { state, .. } => self.job = *state,
            wire::SessionEvent::InputRequested { request } => {
                self.inputs.insert(request.id, (**request).clone());
            }
            wire::SessionEvent::InputResolved { id, .. } => {
                self.inputs.remove(id);
            }
            wire::SessionEvent::InputHeld { id } => {
                if let Some(request) = self.inputs.get_mut(id) {
                    request.timeout = None;
                }
            }
            wire::SessionEvent::TitleChanged { title } => self.title = Some(title.clone()),
            wire::SessionEvent::OptionsChanged { options } => self.options = options.clone(),
            _ => {}
        }
        let seq = Seq(self.last.0 + 1);
        self.last = seq;
        self.subscribers.retain(
            |publisher| match publisher.try_publish(seq, event.clone()) {
                Ok(()) => true,
                Err(Published::Lagged | Published::Gone) => false,
            },
        );
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
    /// every event after it is still buffered. Seq 0 is not a point: a
    /// client holding it may have stopped partway through a transcript.
    fn resumes(&self, cursor: &Cursor) -> bool {
        let oldest = self
            .buffer
            .front()
            .map_or(Seq(self.last.0 + 1), |event| event.seq);
        cursor.stream == self.stream
            && cursor.seq > Seq(0)
            && cursor.seq <= self.last
            && cursor.seq.0 + 1 >= oldest.0
    }

    fn events_after(&self, seq: Seq) -> Vec<(Seq, wire::SessionEvent)> {
        self.buffer
            .iter()
            .filter(|event| event.seq > seq)
            .map(|event| (event.seq, event.event.clone()))
            .collect()
    }

    /// The transcript at seq 0 but its last event, which carries the point
    /// the turn in progress began after, then the turn's buffered events.
    fn reset_events(&self) -> Vec<(Seq, wire::SessionEvent)> {
        let evicted = self.turn_start_evicted.then(|| wire::SessionEvent::Notice {
            text: TURN_START_EVICTED.to_owned(),
        });
        let mut events: Vec<_> = self
            .transcript
            .iter()
            .cloned()
            .chain(evicted)
            .map(|event| (Seq(0), event))
            .collect();
        if let Some((seq, _)) = events.last_mut() {
            *seq = self.turn_start;
        }
        events.extend(self.events_after(self.turn_start));
        events
    }
}

/// How many bytes `event` encodes to, or `None`, logged, for one JSON cannot
/// hold, such as one with a non-UTF-8 path. Every event the feed keeps,
/// live or transcript, passes this, so none takes a seq or reaches a
/// connection's forwarder, which relies on it (`Publisher`).
fn encoded_len(event: &wire::SessionEvent) -> Option<usize> {
    match serde_json::to_vec(event) {
        Ok(encoded) => Some(encoded.len()),
        Err(error) => {
            tracing::warn!(%error, "dropped a session event JSON cannot hold");
            None
        }
    }
}

#[cfg(test)]
#[path = "feed_tests.rs"]
mod tests;
