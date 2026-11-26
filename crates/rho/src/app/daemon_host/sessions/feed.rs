//! One loaded session's event stream (#1657): every wire event gets the
//! stream's next `seq`, goes to every attached client, and stays in a
//! buffer a client reattaching with its cursor resumes from.
//!
//! A client that attaches without a usable cursor gets a reset: the
//! session's saved transcript, as wire events, then the turn in progress
//! from the buffer. The feed keeps that transcript itself, extended with
//! each turn's saved rows as the turn saves, so an attach never waits for
//! the turn that holds the session's runtime. It keeps the rows, which
//! share their content with the session's history, and renders their
//! events for each reset ([`SavedTranscript`]). The transcript and the point
//! where the turn in progress begins move together under the feed's lock,
//! the same lock an attach reads both under, so a turn that saves while a
//! client attaches is neither sent twice nor skipped. A turn that could not
//! save stays in the buffer, so resets replay it live until a later turn
//! saves it.
//!
//! The feed also keeps the session's options, as the last
//! [`wire::SessionEvent::OptionsChanged`] left them, and its background
//! work, as the last [`wire::SessionEvent::Background`] did, so an attach,
//! and `rho/session/background/list`, report them without waiting for the
//! runtime either. It keeps the agent the session runs as, as the last
//! [`wire::SessionEvent::Agent`] said it, which every reset replays first:
//! no saved row holds it. It keeps what the sessions this one starts open
//! with, as the last [`wire::SessionEvent::StartedSessionDefaults`] said
//! it, which every reset replays next, and which `rho/session/new` reads
//! while this session's turn holds its runtime. It keeps the session's
//! pull requests, as the last
//! [`wire::SessionEvent::PullRequests`] sent them, so it sends them again
//! only when they change (see `pull_requests`).
//!
//! A loaded session's feed reports whether the session is settled (see
//! `settle`) as [`wire::SessionEvent::Activity`]: `active` goes out just
//! before a state event that starts a turn, and an attach's replay ends
//! with the current level: the event that last said it, when the replay
//! ends with that, else the level again, even in a replay that holds
//! nothing else. A feed
//! that was never told its activity, as a subagent run's, reports none.
//!
//! A reset's transcript events carry seq 0, except the last, which carries
//! the seq the turn in progress began after. A client's cursor is the
//! highest seq it has seen, so one that detaches partway through a
//! transcript holds no cursor past 0 and resets again.
//!
//! Each event goes out with when it happened
//! ([`wire::SessionEventEnvelope::at`]): a live event when the feed takes
//! it, which the buffer keeps for a tail, and a saved one when its row was
//! saved. The state a reset replays ahead of the transcript, and the
//! activity an attach's replay ends with, carry none.

use std::{
    collections::{BTreeMap, VecDeque},
    path::Path,
    sync::{Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use rho_protocol::{
    self as wire, Cursor, DetachReason, Published, Publisher, Replay, Seq, StreamEvent, StreamId,
    Subscription,
};

use crate::{
    app::session_events::{FeedEvent, SavedTranscript, SavedTurn},
    tools::ui::SurfaceViewer,
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
    /// The session's saved transcript, up to `turn_start`.
    transcript: SavedTranscript,
    /// The job state the last state event sent, so an attach reports the
    /// state its replay ends in.
    job: wire::JobState,
    /// The turn `job` is of, as that event named it.
    turn: Option<wire::TurnId>,
    /// When the feed first saw `turn` under way, while it is.
    turn_since: Option<Instant>,
    /// The input requests opened and not yet resolved, so an attach reports
    /// them even once the buffer no longer holds the events that opened
    /// them.
    inputs: BTreeMap<wire::InputRequestId, wire::InputRequest>,
    /// The session's title: the saved one, then the last title change sent,
    /// so an attach reports it though no transcript holds it.
    title: Option<String>,
    /// The session's options, as the last options event sent them.
    options: wire::SessionOptions,
    /// The id of the agent the session runs as: the one it opened as,
    /// then the last agent event sent.
    agent: String,
    /// What the sessions this one starts open with: what it opened with
    /// (see [`Feed::seed_started_session_defaults`]), then the last
    /// [`wire::SessionEvent::StartedSessionDefaults`] sent.
    started_session_defaults: wire::StartedSessionDefaults,
    /// The session's background work, as the last background event sent
    /// it.
    background: Vec<wire::BackgroundWork>,
    /// The session's pull requests, as the last pull requests event sent
    /// them; empty until the first.
    pull_requests: wire::SessionPullRequests,
    /// The session's context use and spend, as the last usage event sent
    /// them.
    usage: Option<wire::JobUsage>,
    /// `usage` as of `turn_start`, which a reset replays with the
    /// transcript: no saved row holds usage, and the turn in progress
    /// replays its own.
    saved_usage: Option<wire::JobUsage>,
    /// Whether the session is settled, as the last activity event sent it;
    /// `None` until the first.
    activity: Option<wire::SessionActivity>,
    subscribers: Vec<Subscriber>,
    /// Set by [`Feed::close`]: a client attaching later is detached at once.
    closed: Option<DetachReason>,
}

/// One attached client.
struct Subscriber {
    publisher: Publisher,
    /// Held while the client is attached, when it draws the session's
    /// surfaces: the session's `ui` tool waits for the user only while one
    /// such client follows it.
    _viewer: Option<SurfaceViewer>,
}

impl Subscriber {
    fn is_gone(&self) -> bool {
        self.publisher.is_gone()
    }
}

/// The events an attach replays, oldest first, and how many there are. A
/// reset's transcript renders as its client takes it.
struct ReplayEvents {
    len: usize,
    events: Box<dyn Iterator<Item = StreamEvent> + Send + Sync>,
}

struct Buffered {
    event: StreamEvent,
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
    /// The turn `job` is of.
    pub(super) turn: Option<wire::TurnId>,
    /// How long `turn` had been under way as of the attach; `None` when no
    /// turn is.
    pub(super) turn_elapsed: Option<Duration>,
    /// The input requests open as of the attach, oldest first.
    pub(super) inputs: Vec<wire::InputRequest>,
    /// The session's title as of the attach, if it has one.
    pub(super) title: Option<String>,
    /// The session's options as of the attach.
    pub(super) options: wire::SessionOptions,
    /// The session's background work as of the attach.
    pub(super) background: Vec<wire::BackgroundWork>,
    pub(super) subscription: Subscription,
}

/// The answer to an attach or observe of the session in `workspace`, and
/// the subscription its events then arrive on.
pub(super) fn attach_response(
    workspace: &Path,
    attached: Attached,
) -> (wire::SessionAttachResponse, Subscription) {
    let Attached {
        stream,
        replay,
        replay_events,
        job,
        turn,
        turn_elapsed,
        inputs,
        title,
        options,
        background,
        subscription,
    } = attached;
    let response = wire::SessionAttachResponse {
        stream,
        workspace: workspace.to_path_buf(),
        replay,
        replay_events,
        state: job,
        turn,
        // Far below `u64::MAX` milliseconds.
        turn_elapsed_ms: turn_elapsed
            .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)),
        inputs,
        title,
        options,
        background,
    };
    (response, subscription)
}

impl Feed {
    /// A stream with nothing saved yet, its job in `job`: idle for a new
    /// session, its last turn's end for a reopened one (#1661), whose saved
    /// conversation [`Self::end_turn`] then adds. `turn` is the turn `job`
    /// is of, and `title` the one the session has saved, if any; the
    /// session opened with `options`, running as agent `agent`.
    pub(super) fn new(
        job: wire::JobState,
        turn: Option<wire::TurnId>,
        title: Option<String>,
        options: wire::SessionOptions,
        agent: String,
    ) -> Self {
        Self {
            state: Mutex::new(FeedState {
                stream: StreamId::new(uuid::Uuid::new_v4().to_string()),
                last: Seq(0),
                buffer: VecDeque::new(),
                buffered_bytes: 0,
                turn_start: Seq(0),
                turn_start_evicted: false,
                transcript: SavedTranscript::default(),
                job,
                turn,
                // A stream that opens on a turn under way, as a subagent
                // run's does, times the turn from here.
                turn_since: job.in_progress().then(Instant::now),
                inputs: BTreeMap::new(),
                title,
                options,
                agent,
                started_session_defaults: wire::StartedSessionDefaults::default(),
                background: Vec::new(),
                pull_requests: wire::SessionPullRequests::default(),
                usage: None,
                saved_usage: None,
                activity: None,
                subscribers: Vec::new(),
                closed: None,
            }),
        }
    }

    /// Sends `events` to every attached client, in order, timed now, and
    /// buffers them.
    pub(super) fn publish<E: Into<FeedEvent>>(&self, events: impl IntoIterator<Item = E>) {
        let mut state = self.state();
        for event in events {
            state.push(event.into());
        }
    }

    /// Tells attached clients the session's options are now `options`,
    /// unless they already are.
    pub(super) fn update_options(&self, options: wire::SessionOptions) {
        let mut state = self.state();
        if state.options != options {
            state.push(FeedEvent::from(wire::SessionEvent::OptionsChanged {
                options: Box::new(options),
            }));
        }
    }

    /// Tells attached clients the session runs as agent `id` now, unless
    /// they already know: an `/agent:` rebind changes it.
    pub(super) fn update_agent(&self, id: &str) {
        let mut state = self.state();
        if state.agent != id {
            state.push(FeedEvent::from(wire::SessionEvent::Agent {
                id: id.to_owned(),
            }));
        }
    }

    /// Tells attached clients the sessions this one starts open with
    /// `defaults` now, unless they already do.
    pub(super) fn update_started_session_defaults(&self, defaults: wire::StartedSessionDefaults) {
        let mut state = self.state();
        if state.started_session_defaults != defaults {
            state.push(FeedEvent::from(wire::SessionEvent::StartedSessionDefaults(
                defaults,
            )));
        }
    }

    /// What the sessions this one starts open with.
    pub(super) fn started_session_defaults(&self) -> wire::StartedSessionDefaults {
        self.state().started_session_defaults.clone()
    }

    /// Tells attached clients the session's background work is now what
    /// `read` reads, unless it already is. `read` runs under the feed's
    /// lock, so of two reports the later read is the later publish.
    pub(super) fn update_background(&self, read: impl FnOnce() -> Vec<wire::BackgroundWork>) {
        let mut state = self.state();
        let work = read();
        if state.background != work {
            state.push(FeedEvent::from(wire::SessionEvent::Background { work }));
        }
    }

    /// Tells attached clients the session's pull requests are now what
    /// `read` reads, unless they already are, and returns them. `read` runs
    /// under the feed's lock, as [`Self::update_background`]'s does, so of
    /// two reports the later read is the later publish, and a request that
    /// answers with what this returns answers with what the feed last sent.
    pub(super) fn update_pull_requests(
        &self,
        read: impl FnOnce() -> wire::SessionPullRequests,
    ) -> wire::SessionPullRequests {
        let mut state = self.state();
        let pull_requests = read();
        if state.pull_requests != pull_requests {
            state.push(FeedEvent::from(wire::SessionEvent::PullRequests {
                pull_requests: Box::new(pull_requests.clone()),
            }));
        }
        pull_requests
    }

    /// Tells attached clients whether the session is settled, unless they
    /// already know: active while a turn is under way or `wake_due` says
    /// the daemon or the session's host will start one, settled otherwise.
    /// `wake_due` runs under the feed's lock, as [`Self::update_background`]'s
    /// read does, and only between turns.
    pub(super) fn update_activity(&self, wake_due: impl FnOnce() -> bool) {
        let mut state = self.state();
        let activity = if state.job.in_progress() || wake_due() {
            wire::SessionActivity::Active
        } else {
            wire::SessionActivity::Settled
        };
        if state.activity != Some(activity) {
            state.push(FeedEvent::from(wire::SessionEvent::Activity {
                state: activity,
            }));
        }
    }

    /// The session's background work, as the last background event sent
    /// it.
    pub(super) fn background(&self) -> Vec<wire::BackgroundWork> {
        self.state().background.clone()
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
            .map(|buffered| buffered.event.event.clone())
            .collect()
    }

    /// What a client attaching with no cursor replays, oldest first.
    #[cfg(test)]
    pub(super) fn reset_replay_for_tests(&self) -> Vec<wire::SessionEvent> {
        self.replay_for_tests(None)
            .into_iter()
            .map(|event| event.event)
            .collect()
    }

    /// What a client attaching with `after` replays, oldest first, as its
    /// subscription carries it.
    #[cfg(test)]
    pub(super) fn replay_for_tests(&self, after: Option<&Cursor>) -> Vec<StreamEvent> {
        let (_, replay) = self.state().replay_from(after);
        replay.events.collect()
    }

    /// Records that the turns since the last save saved as `saved`: it
    /// joins the transcript, and resets stop replaying those turns' live
    /// events.
    pub(super) fn end_turn(&self, saved: SavedTurn) {
        let mut state = self.state();
        state.transcript.push(saved);
        state.turn_start = state.last;
        state.turn_start_evicted = false;
        state.saved_usage = state.usage;
    }

    /// Replaces the conversation the feed holds with `transcript`, the
    /// session's saved rows after its conversation moved to another point
    /// of its tree, if it has any. The feed becomes a new stream, so no
    /// cursor of the old one resumes, and every attached client is detached
    /// as lagged, which it answers by attaching again and replaying
    /// `transcript`. Call between turns: the buffer holds no turn in
    /// progress then.
    pub(super) fn restart(&self, transcript: Option<SavedTurn>) {
        let mut state = self.state();
        state.stream = StreamId::new(uuid::Uuid::new_v4().to_string());
        state.transcript = SavedTranscript::default();
        if let Some(turn) = transcript {
            state.transcript.push(turn);
        }
        state.buffer.clear();
        state.buffered_bytes = 0;
        state.last = Seq(0);
        state.turn_start = Seq(0);
        state.turn_start_evicted = false;
        // The moved conversation's usage arrives as a live event.
        state.saved_usage = None;
        for subscriber in std::mem::take(&mut state.subscribers) {
            subscriber.publisher.close(DetachReason::Lagged);
        }
    }

    /// Subscribes a client resuming from `after`. A cursor this stream
    /// still buffers resumes with the events after it; anything else
    /// resets to the transcript and the turn in progress. Once the feed is
    /// closed, the client gets that replay, then is detached.
    pub(super) fn attach(&self, after: Option<&Cursor>) -> Attached {
        self.attach_viewer(after, /*viewer*/ None)
    }

    /// [`Self::attach`], for a client that may draw the session's
    /// surfaces: `viewer` is its place among those that do, given up once
    /// the client is gone.
    pub(super) fn attach_viewer(
        &self,
        after: Option<&Cursor>,
        viewer: Option<SurfaceViewer>,
    ) -> Attached {
        let mut state = self.state();
        let (replay, events) = state.replay_from(after);
        // A replay's events are counted in memory, so far below `u64::MAX`.
        let replay_events = u64::try_from(events.len).unwrap_or(u64::MAX);
        let (publisher, subscription) =
            rho_protocol::subscription_of(SUBSCRIBER_BACKLOG, events.events);
        // Clients that left an idle session would otherwise stay until the
        // next event.
        state.subscribers.retain(|subscriber| !subscriber.is_gone());
        match state.closed {
            Some(reason) => publisher.close(reason),
            None => state.subscribers.push(Subscriber {
                publisher,
                _viewer: viewer,
            }),
        }
        Attached {
            stream: state.stream.clone(),
            replay,
            replay_events,
            job: state.job,
            turn: state.turn.clone(),
            turn_elapsed: state.turn_since.map(|since| since.elapsed()),
            inputs: state.inputs.values().cloned().collect(),
            title: state.title.clone(),
            options: state.options.clone(),
            background: state.background.clone(),
            subscription,
        }
    }

    /// Starts the session's usage at `usage`, a usage event of the context
    /// its saved conversation fills, which no saved row holds. Sent to no
    /// live client, but a reset replays it until a saved turn's usage, with
    /// the session's cost, replaces it.
    pub(super) fn seed_usage(&self, usage: &wire::SessionEvent) {
        let mut state = self.state();
        state.usage = usage_of(usage);
        state.saved_usage = state.usage;
    }

    /// Starts the session with `defaults`, what the sessions it starts
    /// open with as it opened with them. Sent to no live client; a reset
    /// replays it.
    pub(super) fn seed_started_session_defaults(&self, defaults: wire::StartedSessionDefaults) {
        self.state().started_session_defaults = defaults;
    }

    /// The session's title, if it has one.
    pub(super) fn title(&self) -> Option<String> {
        self.state().title.clone()
    }

    /// What `rho/job/list` reports of the session's job from its stream.
    pub(super) fn live(&self) -> wire::JobLive {
        let state = self.state();
        wire::JobLive {
            title: state.title.clone(),
            model: state.options.model.current.clone(),
            // Far below `u64::MAX` milliseconds.
            turn_elapsed_ms: state
                .turn_since
                .map(|since| u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX)),
            usage: state.usage,
            inputs: state.inputs.values().cloned().collect(),
        }
    }

    /// Whether a client still follows the session's events.
    pub(super) fn followed(&self) -> bool {
        let mut state = self.state();
        state.subscribers.retain(|subscriber| !subscriber.is_gone());
        !state.subscribers.is_empty()
    }

    /// Ends every client's subscription with `reason`, and detaches each
    /// client that attaches later with it. Closing again keeps the first
    /// reason.
    pub(super) fn close(&self, reason: DetachReason) {
        let mut state = self.state();
        let reason = *state.closed.get_or_insert(reason);
        for subscriber in std::mem::take(&mut state.subscribers) {
            subscriber.publisher.close(reason);
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
    fn push(&mut self, pushed: FeedEvent) {
        let Some(bytes) = pushed.encoded_len() else {
            return;
        };
        let FeedEvent {
            event,
            notification,
        } = pushed;
        // A turn starting makes a settled session active, and clients hear
        // that first.
        if self.activity == Some(wire::SessionActivity::Settled)
            && matches!(&event, wire::SessionEvent::State { state, .. } if state.in_progress())
        {
            self.push(FeedEvent::from(wire::SessionEvent::Activity {
                state: wire::SessionActivity::Active,
            }));
        }
        match &event {
            wire::SessionEvent::State { state, turn } => {
                // A turn waiting for input is still the same turn under way.
                let continues = self.job.in_progress() && self.turn == *turn;
                self.turn_since = match (state.in_progress(), continues) {
                    (false, _) => None,
                    (true, true) => self.turn_since.or_else(|| Some(Instant::now())),
                    (true, false) => Some(Instant::now()),
                };
                self.job = *state;
                self.turn.clone_from(turn);
            }
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
            wire::SessionEvent::Usage { .. } => self.usage = usage_of(&event),
            wire::SessionEvent::OptionsChanged { options } => self.options = (**options).clone(),
            wire::SessionEvent::Agent { id } => self.agent.clone_from(id),
            wire::SessionEvent::StartedSessionDefaults(defaults) => {
                self.started_session_defaults.clone_from(defaults);
            }
            wire::SessionEvent::Background { work } => self.background.clone_from(work),
            wire::SessionEvent::PullRequests { pull_requests } => {
                self.pull_requests = (**pull_requests).clone();
            }
            wire::SessionEvent::Activity { state } => self.activity = Some(*state),
            _ => {}
        }
        let seq = Seq(self.last.0 + 1);
        self.last = seq;
        let mut event = StreamEvent::new(seq, event);
        event.at = Some(now_ms());
        event.notification = notification;
        self.subscribers.retain(|subscriber| {
            match subscriber.publisher.try_publish_event(event.clone()) {
                Ok(()) => true,
                Err(Published::Lagged | Published::Gone) => false,
            }
        });
        self.buffer.push_back(Buffered { event, bytes });
        self.buffered_bytes += bytes;
        while self.buffered_bytes > REPLAY_BUFFER_BYTES {
            let Some(evicted) = self.buffer.pop_front() else {
                break;
            };
            self.buffered_bytes -= evicted.bytes;
            if evicted.event.seq > self.turn_start && !self.turn_start_evicted {
                self.turn_start_evicted = true;
                tracing::warn!(
                    limit = REPLAY_BUFFER_BYTES,
                    "a turn outgrew REPLAY_BUFFER_BYTES; clients that reset into it miss its start until it saves"
                );
            }
        }
    }

    /// What a client resuming from `after` is sent before the live events,
    /// ending with the session's activity, once the feed reports it.
    fn replay_from(&self, after: Option<&Cursor>) -> (Replay, ReplayEvents) {
        // The replay's buffered events are those after `from`.
        let (replay, events, from) = match after.filter(|cursor| self.resumes(cursor)) {
            Some(cursor) => {
                let tail = self.events_after(cursor.seq);
                let events = ReplayEvents {
                    len: tail.len(),
                    events: Box::new(tail.into_iter()),
                };
                (Replay::Tail, events, cursor.seq)
            }
            None => (Replay::Reset, self.reset_events(), self.turn_start),
        };
        // A replay whose last event is the activity event says the level
        // already; any other gets it again, under the stream's last seq.
        // The buffer's back is the stream's newest event unless that one
        // alone outgrew the buffer and was evicted (an activity event never
        // does), so the back says the level only while its seq is the last.
        // NEXT_MAJOR(rho-protocol): report a session's activity as SessionAttachResponse::activity and stop replaying a trailing synthetic SessionEvent::Activity.
        let said = self.buffer.back().is_some_and(|newest| {
            newest.event.seq == self.last
                && newest.event.seq > from
                && matches!(newest.event.event, wire::SessionEvent::Activity { .. })
        });
        let activity = self
            .activity
            .filter(|_| !said)
            .map(|state| StreamEvent::new(self.last, wire::SessionEvent::Activity { state }));
        let events = ReplayEvents {
            len: events.len + usize::from(activity.is_some()),
            events: Box::new(events.events.chain(activity)),
        };
        (replay, events)
    }

    /// Whether `cursor` is a point of this stream the buffer resumes from:
    /// every event after it is still buffered. Seq 0 is not a point: a
    /// client holding it may have stopped partway through a transcript.
    fn resumes(&self, cursor: &Cursor) -> bool {
        let oldest = self
            .buffer
            .front()
            .map_or(Seq(self.last.0 + 1), |buffered| buffered.event.seq);
        cursor.stream == self.stream
            && cursor.seq > Seq(0)
            && cursor.seq <= self.last
            && cursor.seq.0 + 1 >= oldest.0
    }

    fn events_after(&self, seq: Seq) -> Vec<StreamEvent> {
        self.buffer
            .iter()
            .filter(|buffered| buffered.event.seq > seq)
            .map(|buffered| buffered.event.clone())
            .collect()
    }

    /// The agent, what the sessions this one starts open with, the
    /// transcript, and the usage as of its last save at seq 0 but the last
    /// event, which carries the point the turn in progress began after, then
    /// the turn's buffered events. No saved row holds the agent, that choice,
    /// or usage, so without them a client that resets would not know the
    /// agent or the choice, and would show no context or spend until the
    /// next turn.
    /// The transcript renders as the client takes it, so a reset never
    /// holds a long session's events at once.
    fn reset_events(&self) -> ReplayEvents {
        let agent = wire::SessionEvent::Agent {
            id: self.agent.clone(),
        };
        let evicted = self.turn_start_evicted.then(|| wire::SessionEvent::Notice {
            text: TURN_START_EVICTED.to_owned(),
        });
        let started =
            wire::SessionEvent::StartedSessionDefaults(self.started_session_defaults.clone());
        let usage = self.saved_usage.map(usage_event);
        let saved = 2
            + self.transcript.len()
            + usize::from(evicted.is_some())
            + usize::from(usage.is_some());
        let turn_start = self.turn_start;
        let tail = self.events_after(turn_start);
        let len = saved + tail.len();
        let untimed = |event: wire::SessionEvent| (None, FeedEvent::from(event));
        let events = [agent, started]
            .into_iter()
            .map(untimed)
            .chain(self.transcript.events())
            .chain(evicted.map(untimed))
            .chain(usage.map(untimed))
            .enumerate()
            .map(move |(index, (at, event))| {
                let seq = if index + 1 == saved {
                    turn_start
                } else {
                    Seq(0)
                };
                let mut sent = StreamEvent::new(seq, event.event);
                sent.at = at;
                sent.notification = event.notification;
                sent
            })
            .chain(tail);
        ReplayEvents {
            len,
            events: Box::new(events),
        }
    }
}

/// Now, as Unix time in milliseconds.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// What `event` says of the session's context use and spend, if it is a
/// usage event.
fn usage_of(event: &wire::SessionEvent) -> Option<wire::JobUsage> {
    match event {
        wire::SessionEvent::Usage {
            used_tokens,
            window_tokens,
            cost_usd_micros,
        } => Some(wire::JobUsage {
            used_tokens: *used_tokens,
            window_tokens: *window_tokens,
            cost_usd_micros: *cost_usd_micros,
        }),
        _ => None,
    }
}

/// The usage event that reports `usage`; [`usage_of`] reverses it.
fn usage_event(usage: wire::JobUsage) -> wire::SessionEvent {
    wire::SessionEvent::Usage {
        used_tokens: usage.used_tokens,
        window_tokens: usage.window_tokens,
        cost_usd_micros: usage.cost_usd_micros,
    }
}

#[cfg(test)]
#[path = "feed_tests.rs"]
mod tests;
