//! A session's pump: the one task that relays its attachments' events to
//! the host, in order, with the [`Control`]s its requests hand it.
//!
//! The pump owns the session's place in the stream ([`StreamPosition`]),
//! so it alone attaches again: when a request finds the session stale
//! ([`Control::Resume`]), when an ACP v2 host resumes the session it has
//! open ([`Control::Resync`]), keeping the turn it follows, and at once
//! when the daemon drops an attachment that fell behind (`Lagged`).
//!
//! A prompt of this host's is matched to its turn by the turn's id, which
//! the daemon answers the prompt with ([`Armed`]): the pump reads nothing
//! past [`Control::Arm`] until it has that id, so it never guesses whose
//! turn an event is of. Until the host prompts, the pump follows the turn
//! the host was told is running when it attached ([`Following::Announced`]).
//!
//! The pump also keeps the session's background work for the host
//! ([`Shared`]'s): each attach's, once its replay is through, and each
//! change the daemon sends, which it tells the host through
//! [`AcpClientPort::background_changed`].
//!
//! When the daemon link is lost, the host hears that the session can no
//! longer be observed ([`AcpClientPort::unobservable`]), and once the pump
//! attaches again, the session's state.

use std::sync::{Arc, PoisonError};

use agent_client_protocol::{
    v1::schema::{MessageId, SessionInfoUpdate, SessionUpdate, UsageUpdate},
    Error as AcpError,
};
use rho_protocol::{
    BackgroundWork, DetachReason, InputRequest, JobState, Replay, Seq, SessionAttachResponse,
    SessionEvent, SessionOptions, SessionSetup, TurnId, TurnOutcome,
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

use super::{
    busy, lock, Attacher, Attachment, Control, Current, Inbound, Opening, ReplayFrom, Replayed,
    Resync, Shared, Show, Waiter,
};
use crate::{
    app::{
        acp::{
            daemon,
            inputs::{InputContext, Inputs},
            prompt,
            updates::{self, Delivery, Updates},
            AcpClientPort, BackgroundChanged, Foreground, TurnEnd,
        },
        daemon_client::{HerdrStatus, Part, Routed, StreamPosition},
    },
    herdr::HerdrReporter,
};

/// Starts the pump of session `context.session` on the attach that opened
/// it, before it reads anything else from `inbound`.
pub(super) fn spawn(
    context: InputContext,
    attacher: Arc<dyn Attacher>,
    (sender, inbound): (
        mpsc::WeakUnboundedSender<Inbound>,
        mpsc::UnboundedReceiver<Inbound>,
    ),
    shared: Arc<Shared>,
    setup: SessionSetup,
    herdr: &HerdrReporter,
    opening: Opening,
) -> JoinHandle<()> {
    let pump = Pump {
        updates: Updates::new(context.features),
        herdr: HerdrStatus::new(herdr, &context.session),
        position: StreamPosition::attached(&opening.response),
        context,
        attacher,
        sender,
        inbound,
        shared,
        setup,
        turn: None,
        following: None,
        inputs: Inputs::default(),
        replaying: None,
        observed: Observed::Yes,
        usage: None,
        resync: None,
    };
    tokio::spawn(pump.run(opening))
}

/// An attachment's replay in progress.
struct Replaying {
    show: Show,
    /// The session's title when the attachment began: the [`Current`]
    /// title, which a host shown history also gets once it is through
    /// unless the history carried one.
    title: Option<String>,
    /// Whether the history shown carried a title.
    titled: bool,
    /// How the followed turn ended, if a history shown ends it: settled
    /// once the host has the response, as the replay may hold no states.
    ended: Option<TurnOutcome>,
    /// The input requests open when the attachment began.
    inputs: Vec<InputRequest>,
    /// The background work the replay ends with, as the attach answered.
    background: Vec<BackgroundWork>,
    /// The job state the replay ends in, and the turn it is of, as the
    /// attach answered.
    state: JobState,
    turn: Option<TurnId>,
    done: Replayed,
    reporter: Reporter,
}

impl Replaying {
    /// Whether this is the replay of the attach that opened the session,
    /// whose host has yet to hear of it.
    fn opening(&self) -> bool {
        matches!(self.reporter, Reporter::Opening(_))
    }
}

/// Who tells the host what an attachment found: the session's options,
/// background work, and state.
enum Reporter {
    /// The response of the request that opened the session, released once
    /// the host has it.
    Opening(oneshot::Receiver<()>),
    /// The response of a resume's resync, released once the host has it.
    Resync(oneshot::Receiver<()>),
    /// The pump, as the attach is its own: a reattach after the cursor.
    Pump,
}

impl Reporter {
    /// Released once the host has the response that reports the
    /// attachment, if a request answers it.
    fn gate(self) -> Option<oneshot::Receiver<()>> {
        match self {
            Self::Opening(gate) | Self::Resync(gate) => Some(gate),
            Self::Pump => None,
        }
    }
}

/// The turn the pump follows for the host, whose end it must report.
enum Following {
    /// The prompt of this host's that the pump waits on.
    Prompt(Armed),
    /// A turn the host was told is running that no prompt of its own runs:
    /// the one under way when a request's attach began, as ACP v2
    /// `session/resume` reports `running` from the same attach state
    /// (`v2::state`), or one that began while the pump followed none.
    /// A prompt of this host's replaces it unreported, since that prompt's
    /// own updates then give the session's state.
    Announced(TurnId),
}

impl Following {
    fn turn(&self) -> &TurnId {
        match self {
            Self::Prompt(armed) => &armed.turn,
            Self::Announced(turn) => turn,
        }
    }
}

/// The prompt of this host's that the pump waits on, by its turn.
struct Armed {
    /// The turn the daemon answered the prompt with.
    turn: TurnId,
    waiter: Waiter,
    /// Whether the turn's user message, the prompt's own, went by.
    shown: bool,
    /// Whether a background change went unrepeated in a `state_update`
    /// before the prompt was acknowledged (see [`Pump::told_foreground`]).
    withheld: bool,
}

struct Pump {
    context: InputContext,
    attacher: Arc<dyn Attacher>,
    /// The pump's own inbound, for [`Inbound::Drained`].
    sender: mpsc::WeakUnboundedSender<Inbound>,
    inbound: mpsc::UnboundedReceiver<Inbound>,
    updates: Updates,
    shared: Arc<Shared>,
    setup: SessionSetup,
    herdr: HerdrStatus,
    /// The turn the session's job is in or last ran, as its last state
    /// said.
    turn: Option<TurnId>,
    following: Option<Following>,
    inputs: Inputs,
    position: StreamPosition,
    replaying: Option<Replaying>,
    observed: Observed,
    /// The session's last usage report, which a resume reports again.
    usage: Option<UsageUpdate>,
    /// A resync waiting for its detached attachment's events.
    resync: Option<Resync>,
}

/// Whether the host can tell what the session does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Observed {
    Yes,
    /// The host was told it can no longer observe the session
    /// ([`AcpClientPort::unobservable`]) and awaits its state again.
    Lost,
    /// Attached again with the session idle. The host hears that before
    /// whatever the pump handles next, unless that is a prompt's
    /// [`Control::Arm`]: the prompt's acknowledgement then reports
    /// `running` instead, as an `idle` first would end T3 Code's pending
    /// prompt (see [`Pump::told_foreground`]).
    IdleOwed,
}

impl Pump {
    async fn run(mut self, opening: Opening) {
        let Opening {
            response,
            show,
            replayed,
            gate,
        } = opening;
        self.begin(response, show, replayed, Reporter::Opening(gate))
            .await;
        while let Some(next) = self.inbound.recv().await {
            // A resume reports the state itself.
            if self.observed == Observed::IdleOwed
                && !matches!(
                    next,
                    Inbound::Control(Control::Arm { .. } | Control::Resync(_)) | Inbound::Drained
                )
            {
                self.report_state(Foreground::Idle);
            }
            match next {
                Inbound::Routed(routed) => self.routed(routed).await,
                Inbound::Control(control) => self.control(control).await,
                Inbound::Drained => {
                    if let Some(resync) = self.resync.take() {
                        self.resync(resync).await;
                    }
                }
            }
        }
    }

    /// Whether the attachment delivers; what the router reads for an ended
    /// one is dropped.
    fn attached(&self) -> bool {
        matches!(*self.shared.attachment.borrow(), Attachment::Attached(_))
    }

    async fn routed(&mut self, routed: Routed) {
        if !self.attached() {
            return;
        }
        match routed {
            Routed::Event { seq, event } => self.event(seq, event).await,
            Routed::Detached(reason) => self.detached(reason).await,
            Routed::Lost => self.end(Attachment::Stale, daemon::lost()),
        }
    }

    async fn control(&mut self, control: Control) {
        match control {
            Control::Resume(replayed) => match self.reattach().await {
                Ok((response, show)) => self.begin(response, show, replayed, Reporter::Pump).await,
                Err(error) => {
                    let _ = replayed.send(Err(error));
                }
            },
            Control::Resync(resync) => self.detach_for(*resync).await,
            // Waits for the daemon's answer to the prompt before it reads
            // on: the turn's events, which queue behind this, are then told
            // from every other turn's by its id. A prompt the daemon
            // refused, or one that ended before it began, has no turn to
            // wait on.
            Control::Arm { waiter, turn } if self.attached() => match turn.await {
                Ok(turn) => {
                    if self.observed == Observed::IdleOwed {
                        self.observed = Observed::Yes;
                    }
                    self.following = Some(Following::Prompt(Armed {
                        turn,
                        waiter,
                        shown: false,
                        withheld: false,
                    }));
                }
                Err(_) if self.observed == Observed::IdleOwed => {
                    self.report_state(Foreground::Idle);
                }
                Err(_) => {}
            },
            // The attachment ended between the prompt's reattach and now.
            Control::Arm { waiter, .. } => {
                let _ = waiter.outcome.send(Err(daemon::lost()));
            }
            Control::Options(options, told) => {
                self.options_changed(*options).await;
                let _ = told.send(());
            }
            Control::Flush(done) => {
                let _ = done.send(());
            }
        }
    }

    /// Attaches again after the cursor. Returns the response with what its
    /// replay shows: the events the host missed show as live, and a reset's
    /// history, which the host has, is skipped.
    async fn reattach(&self) -> Result<(SessionAttachResponse, Show), AcpError> {
        let response = self
            .attacher
            .attach(&self.context.session, self.position.cursor(), &self.setup)
            .await?;
        let show = Show::resumed(response.replay);
        Ok((response, show))
    }

    /// Detaches for `resync`, which attaches once the pump read every event
    /// the detached attachment sent.
    async fn detach_for(&mut self, resync: Resync) {
        if self.attached() {
            if let Err(error) = self.attacher.detach(&self.context.session).await {
                let _ = resync.replayed.send(Err(error));
                return;
            }
        }
        // A pump that is still running holds a sender in its session.
        if let Some(sender) = self.sender.upgrade() {
            self.resync = Some(resync);
            let _ = sender.send(Inbound::Drained);
        }
    }

    /// Attaches again in place for a host's resume: a history replays from
    /// the start, and otherwise the events after the cursor show as
    /// missed. The followed turn is kept. An attach that fails leaves the
    /// session stale, as its earlier attachment is detached.
    async fn resync(&mut self, resync: Resync) {
        let Resync {
            from,
            setup,
            replayed,
            gate,
        } = resync;
        self.setup = setup;
        let attached = match from {
            ReplayFrom::Start(port) => self
                .attacher
                .attach(&self.context.session, /*after*/ None, &self.setup)
                .await
                .map(|response| (response, Show::History(port))),
            ReplayFrom::Cursor => self.reattach().await,
        };
        match attached {
            Ok((response, show)) => {
                self.begin(response, show, replayed, Reporter::Resync(gate))
                    .await;
            }
            Err(error) => {
                if self.attached() {
                    self.end(Attachment::Stale, error.clone());
                }
                let _ = replayed.send(Err(error));
            }
        }
    }

    /// Starts relaying the attachment `response` began.
    async fn begin(
        &mut self,
        response: SessionAttachResponse,
        show: Show,
        done: Replayed,
        reporter: Reporter,
    ) {
        self.position.reattached(&response);
        match reporter {
            Reporter::Opening(_) | Reporter::Resync(_) => {
                *lock(&self.shared.options) = response.options;
                self.observed = Observed::Yes;
            }
            Reporter::Pump => self.options_changed(response.options).await,
        }
        // A history shown starts the host's transcript over.
        if matches!(show, Show::History(_)) {
            self.updates = Updates::new(self.context.features);
        }
        self.shared
            .attachment
            .send_replace(Attachment::Attached(response.state));
        self.herdr.report(response.state).await;
        // A reset starts over with the saved conversation, whose messages
        // are of turns its states have yet to name.
        if response.replay == Replay::Reset {
            self.turn = None;
            self.usage = None;
        }
        self.replaying = Some(Replaying {
            show,
            title: response.title,
            titled: false,
            ended: None,
            inputs: response.inputs,
            background: response.background,
            state: response.state,
            turn: response.turn,
            done,
            reporter,
        });
        if !self.position.replaying() {
            self.end_replay().await;
        }
    }

    async fn event(&mut self, seq: Seq, event: SessionEvent) {
        if let SessionEvent::Usage {
            used_tokens,
            window_tokens,
            cost_usd_micros,
        } = &event
        {
            self.usage = Some(updates::usage_update(
                *used_tokens,
                *window_tokens,
                *cost_usd_micros,
            ));
        }
        let left = match self.position.observe(seq) {
            Part::Live => return self.live(event).await,
            Part::Replayed { left, .. } => left,
        };
        let Some(replaying) = self.replaying.as_mut() else {
            return;
        };
        match replaying.show.clone() {
            Show::History(port) => {
                match &event {
                    SessionEvent::TitleChanged { .. } => replaying.titled = true,
                    SessionEvent::TurnEnded { turn, outcome }
                        if self
                            .following
                            .as_ref()
                            .is_some_and(|following| following.turn() == turn) =>
                    {
                        replaying.ended = Some(outcome.clone());
                    }
                    _ => {}
                }
                for update in self.updates.map(&event) {
                    self.send(&*port, update, Delivery::Replay).await;
                }
            }
            Show::Missed => self.live(event).await,
            // The host has the history; only the followed turn reads it.
            Show::Skip => {
                self.follow_turn(&event);
            }
        }
        if left == 0 {
            self.end_replay().await;
        }
    }

    /// Sends the title a history may lack, fails a prompt whose turn's
    /// end a skipped replay no longer held, takes the attach's background
    /// work, tells a host that could not observe the session its state,
    /// answers whoever waits for the replay with the session's info, waits
    /// for the host's response, settles a followed turn a history ended or
    /// no longer holds, then puts the open input requests to the host.
    async fn end_replay(&mut self) {
        let Some(Replaying {
            show,
            title,
            titled,
            ended,
            inputs,
            background,
            state,
            turn,
            done,
            reporter,
        }) = self.replaying.take()
        else {
            return;
        };
        self.turn.clone_from(&turn);
        let gate = reporter.gate();
        let answers_request = gate.is_some();
        let mut settled = match (ended, &show) {
            (Some(outcome), _) => Some(Ok(outcome)),
            // A missed tail went by live, the followed turn's end with it.
            (None, Show::Missed) => None,
            (None, Show::History(_) | Show::Skip) => {
                self.ended_unseen(state).then(unreplayed).map(Err)
            }
        };
        // An ACP v2 replay port drops it: v2 replays no titles, and its
        // resume reports the `Current` title instead.
        if let (Show::History(port), Some(title)) = (&show, &title) {
            if !titled {
                let update =
                    SessionUpdate::SessionInfoUpdate(SessionInfoUpdate::new().title(title.clone()));
                self.send(&**port, update, Delivery::Replay).await;
            }
        }
        // The host hears the end at once unless a request awaits its
        // response, which must come first, as the replay holds no states.
        if !answers_request {
            if let Some(end) = settled.take() {
                self.settle(end);
            }
        }
        // A reattach tells the host what changed meanwhile; a request's
        // response reports this work itself. A host that could not observe
        // the session hears no state with it, only the snapshot after.
        if answers_request {
            *lock(&self.shared.background) = background;
        } else {
            self.background_changed(background);
        }
        if self.observed == Observed::Lost {
            if !busy(state) {
                self.observed = Observed::IdleOwed;
            } else {
                if self.following.is_none() {
                    self.following = turn.map(Following::Announced);
                }
                self.report_state(Foreground::of(state));
            }
        }
        let usage = self.usage.clone();
        let _ = done.send(Ok(Current { title, usage }));
        if let Some(gate) = gate {
            // A request that failed to respond drops the gate; the events
            // still belong to the session.
            let _ = gate.await;
        }
        if let Some(end) = settled {
            self.settle(end);
        }
        // The response reported the turn under way as running, so the pump
        // reports its end.
        if answers_request && self.following.is_none() && busy(state) {
            self.following = self.turn.clone().map(Following::Announced);
        }
        // The attach lists every request still open; a skipped replay did
        // not say how the others were resolved.
        self.inputs.retain(&inputs);
        for request in inputs {
            self.inputs.open(request, &self.context);
        }
    }

    /// Whether the followed turn ended unseen, once a replay ending with
    /// the job in `state`, of the turn the attach named (now `self.turn`),
    /// is over: a skipped reset or a history held its end unless another
    /// turn saved since, so a followed turn no longer under way ended where
    /// the daemon no longer replays it.
    fn ended_unseen(&self, state: JobState) -> bool {
        let under_way = busy(state).then_some(self.turn.as_ref()).flatten();
        self.following
            .as_ref()
            .is_some_and(|following| under_way != Some(following.turn()))
    }

    async fn live(&mut self, event: SessionEvent) {
        let own = self.follow_turn(&event);
        match event {
            // The host already shows its own prompt.
            SessionEvent::UserMessage { .. } if own => {}
            // The followed turn's, which `follow_turn` settled, or one the
            // host follows no other way.
            SessionEvent::TurnEnded { .. } => {}
            SessionEvent::State { state, turn } => {
                let before = self
                    .shared
                    .attachment
                    .send_replace(Attachment::Attached(state))
                    .foreground();
                self.foreground_changed(before, Foreground::of(state), turn);
                self.herdr.report(state).await;
            }
            SessionEvent::InputRequested { request } => self.inputs.open(*request, &self.context),
            SessionEvent::InputResolved { id, .. } => self.inputs.resolve(id),
            SessionEvent::OptionsChanged { options } => self.options_changed(*options).await,
            SessionEvent::Background { work } => self.background_changed(work),
            // Another client's prompt, or a steer.
            event => self.send_mapped(&event).await,
        }
    }

    /// Reads what `event` says of the followed turn: the first user message
    /// of an armed prompt's turn that is no steer is the prompt's own, which
    /// accepts it, and the followed turn's end settles it. Returns whether
    /// `event` is that user message. A prompt that gave up (its receiver is
    /// gone) is dropped.
    fn follow_turn(&mut self, event: &SessionEvent) -> bool {
        if let SessionEvent::State { turn, .. } = event {
            self.turn.clone_from(turn);
        }
        if matches!(
            &self.following,
            Some(Following::Prompt(armed)) if armed.waiter.outcome.is_closed()
        ) {
            self.following = None;
        }
        if let SessionEvent::TurnEnded { turn, outcome } = event {
            if self
                .following
                .as_ref()
                .is_some_and(|following| following.turn() == turn)
            {
                self.settle(Ok(outcome.clone()));
            }
            return false;
        }
        let Some(Following::Prompt(armed)) = self.following.as_mut() else {
            return false;
        };
        match event {
            // A steer applies within the turn under way; it never starts
            // one.
            SessionEvent::UserMessage {
                message,
                steer: None,
                ..
            } if !armed.shown && self.turn.as_ref() == Some(&armed.turn) => {
                armed.shown = true;
                let accept = armed
                    .waiter
                    .accept
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take();
                if let Some(accept) = accept {
                    accept(MessageId::new(message.as_str()));
                }
                true
            }
            _ => false,
        }
    }

    /// Settles the followed turn, if any. The host hears it end before
    /// anything the pump reads next, as this is the only place a turn's
    /// end is told: an announced turn with the outcome's stop reason, or
    /// unseen when the pump lost track of it (`end` is the error); an armed
    /// prompt's turn with the stop reason or error, once acknowledged,
    /// before the prompt hears `end`. A prompt whose turn ended before its
    /// user message went by is acknowledged here, as the daemon saved it.
    fn settle(&mut self, end: Result<TurnOutcome, AcpError>) {
        let told = match self.following.take() {
            Some(Following::Prompt(armed)) => {
                let told = match (&end, armed.shown) {
                    (Ok(_), shown) => {
                        if !shown {
                            let accept = armed
                                .waiter
                                .accept
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .take();
                            if let Some(accept) = accept {
                                accept(MessageId::new(format!(
                                    "rho-user-{}",
                                    uuid::Uuid::new_v4()
                                )));
                            }
                        }
                        Some(TurnEnd::Ended(end.clone().and_then(prompt::stop_reason)))
                    }
                    (Err(error), true) => Some(TurnEnd::Ended(Err(error.clone()))),
                    // The prompt fails unacknowledged; only the state its
                    // wait withheld is owed.
                    (Err(_), false) => armed.withheld.then_some(TurnEnd::Unseen),
                };
                self.tell_turn_ended(told);
                let _ = armed.waiter.outcome.send(end);
                return;
            }
            Some(Following::Announced(_)) => Some(match end {
                Ok(outcome) => TurnEnd::Ended(prompt::stop_reason(outcome)),
                Err(_) => TurnEnd::Unseen,
            }),
            None => None,
        };
        self.tell_turn_ended(told);
    }

    fn tell_turn_ended(&self, end: Option<TurnEnd>) {
        if let Some(end) = end {
            let background = lock(&self.shared.background);
            self.context
                .port
                .turn_ended(&self.context.acp_id, end, &background);
        }
    }

    /// Takes `work` as the session's background work and tells the host
    /// how it changed, if it did.
    fn background_changed(&mut self, work: Vec<BackgroundWork>) {
        let foreground = self.told_foreground();
        let mut background = lock(&self.shared.background);
        if *background == work {
            return;
        }
        if let Some(Following::Prompt(armed)) = &mut self.following {
            armed.withheld |= foreground.is_none();
        }
        let previous = std::mem::replace(&mut *background, work);
        self.context.port.background_changed(
            &self.context.acp_id,
            BackgroundChanged {
                previous: &previous,
                current: &background,
                foreground,
            },
        );
    }

    /// What the host was told the session's foreground work is doing: a
    /// turn it follows, as the job's state has it, or else idle, since the
    /// host hears of no other turn. `None` while a prompt of the host's
    /// waits to be acknowledged: the host was told idle, but T3 Code ends
    /// its pending prompt on any `idle`, and the acknowledgement's
    /// `running` comes next. `None` too while the host awaits the
    /// session's state after a loss ([`Observed`]), which the pump reports
    /// itself.
    fn told_foreground(&self) -> Option<Foreground> {
        if self.observed != Observed::Yes {
            return None;
        }
        match &self.following {
            Some(Following::Announced(_) | Following::Prompt(Armed { shown: true, .. })) => {
                Some(self.shared.attachment.borrow().foreground())
            }
            Some(Following::Prompt(Armed { shown: false, .. })) => None,
            None => Some(Foreground::Idle),
        }
    }

    /// Tells the host of a change from `before` to `now` it hears no other
    /// way: a turn it does not follow beginning, as another client's prompt
    /// or a turn the daemon starts as background work finishes, which the
    /// pump follows from then on ([`Following::Announced`]); or the turn it
    /// follows starting or stopping to wait for a person. A prompt reports
    /// its own turn's start, and a followed turn's end is reported as it
    /// settles.
    fn foreground_changed(&mut self, before: Foreground, now: Foreground, turn: Option<TurnId>) {
        if before == Foreground::Idle && now != Foreground::Idle {
            let (None, Some(turn)) = (&self.following, turn) else {
                return;
            };
            self.following = Some(Following::Announced(turn));
        } else if before == now || now == Foreground::Idle {
            return;
        }
        if self
            .told_foreground()
            .is_none_or(|told| told == Foreground::Idle)
        {
            return;
        }
        let background = lock(&self.shared.background);
        self.context
            .port
            .foreground_changed(&self.context.acp_id, now, &background);
    }

    async fn options_changed(&mut self, options: SessionOptions) {
        let old = std::mem::replace(&mut *lock(&self.shared.options), options.clone());
        let port = Arc::clone(&self.context.port);
        for update in updates::options_changed(&old, &options, self.context.features) {
            self.send(&*port, update, Delivery::Live).await;
        }
    }

    async fn detached(&mut self, reason: DetachReason) {
        match reason {
            // A pending resync attaches instead.
            DetachReason::Lagged if self.resync.is_some() => {}
            // Fell behind: resume after the cursor at once. Nobody waits
            // for this replay.
            DetachReason::Lagged => match self.reattach().await {
                Ok((response, show)) => {
                    let (replayed, _) = oneshot::channel();
                    self.begin(response, show, replayed, Reporter::Pump).await;
                }
                Err(error) => self.end(Attachment::Stale, error),
            },
            DetachReason::DaemonStopping => self.end(
                Attachment::Stale,
                AcpError::internal_error().data("rho daemon is stopping"),
            ),
            // Re-executing in place: the next request attaches again on a
            // new connection, which waits for the restarted daemon. Only a
            // prompt waiting on a turn the restart interrupted hears of it.
            DetachReason::Restarting => self.end(
                Attachment::Stale,
                AcpError::internal_error().data("rho daemon restarted"),
            ),
            DetachReason::Deleted => {
                let error =
                    crate::app::acp::resource_not_found(Some(self.context.acp_id.0.to_string()));
                self.end(Attachment::Deleted, error);
            }
            // `Unknown`, or a reason a newer daemon sends: attach again on
            // the next request.
            _ => self.end(Attachment::Stale, daemon::lost()),
        }
    }

    /// Ends the attachment as `attachment`: the prompt waiting on it fails
    /// with `error`, and the host's input prompts are withdrawn. A deleted
    /// session's announced turn ends unseen; a stale one's goes on unseen
    /// ([`Self::lose`]).
    fn end(&mut self, attachment: Attachment, error: AcpError) {
        match attachment {
            Attachment::Stale => self.lose(error),
            Attachment::Deleted | Attachment::Attached(_) => self.fail(error),
        }
        self.inputs.abort_all();
        self.replaying = None;
        self.shared.attachment.send_replace(attachment);
    }

    /// Fails the followed turn, if any, with `error`.
    fn fail(&mut self, error: AcpError) {
        self.settle(Err(error));
    }

    /// The daemon link is gone, so nothing the session does can be seen
    /// until the pump attaches again: the host hears that, instead of an
    /// end of the followed turn, which may run on, and the prompt waiting
    /// on that turn fails with `error`. A host still waiting for the
    /// response that names the session hears nothing.
    fn lose(&mut self, error: AcpError) {
        if !self.replaying.as_ref().is_some_and(Replaying::opening) {
            self.context.port.unobservable(&self.context.acp_id);
            self.observed = Observed::Lost;
        }
        if let Some(Following::Prompt(armed)) = self.following.take() {
            let _ = armed.waiter.outcome.send(Err(error));
        }
    }

    /// Reports the session's state to a host that could not observe it.
    fn report_state(&mut self, foreground: Foreground) {
        self.observed = Observed::Yes;
        let background = lock(&self.shared.background);
        self.context
            .port
            .foreground_changed(&self.context.acp_id, foreground, &background);
    }

    async fn send_mapped(&mut self, event: &SessionEvent) {
        let port = Arc::clone(&self.context.port);
        for update in self.updates.map(event) {
            self.send(&*port, update, Delivery::Live).await;
        }
    }

    async fn send(&self, port: &dyn AcpClientPort, update: SessionUpdate, delivery: Delivery) {
        let notification = updates::notification(&self.context.acp_id, update, delivery);
        if let Err(error) = port.send_session_notification(notification).await {
            tracing::warn!(%error, "could not send an ACP session update");
        }
    }
}

/// Why a followed turn fails whose end the replay no longer held.
fn unreplayed() -> AcpError {
    AcpError::internal_error().data(
        "the prompt's turn ended while this client was behind, and the rho daemon no longer replays how",
    )
}

#[cfg(test)]
#[path = "pump_tests.rs"]
mod tests;
