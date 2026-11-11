//! A session's pump: the one task that relays its attachments' events to
//! the host, in order, with the [`Control`]s its requests hand it.
//!
//! The pump owns the session's place in the stream ([`StreamPosition`]),
//! so it alone attaches again: when a request finds the session stale
//! ([`Control::Resume`]), and at once when the daemon drops an attachment
//! that fell behind (`Lagged`).
//!
//! A prompt of this host's is matched to its turn by the turn's id, which
//! the daemon answers the prompt with ([`Armed`]): the pump reads nothing
//! past [`Control::Arm`] until it has that id, so it never guesses whose
//! turn an event is of.

use std::sync::{Arc, PoisonError};

use agent_client_protocol::{
    v1::schema::{MessageId, SessionInfoUpdate, SessionUpdate},
    Error as AcpError,
};
use rho_protocol::{
    DetachReason, InputRequest, JobState, Replay, Seq, SessionAttachResponse, SessionEvent,
    SessionOptions, SessionSetup, TurnId,
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

use super::{
    busy, lock, Attacher, Attachment, Control, Inbound, Opening, Replayed, Shared, Show, Waiter,
};
use crate::{
    app::{
        acp::{
            daemon,
            inputs::{InputContext, Inputs},
            updates::{self, Delivery, Updates},
            AcpClientPort,
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
    inbound: mpsc::UnboundedReceiver<Inbound>,
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
        inbound,
        shared,
        setup,
        turn: None,
        waiter: None,
        inputs: Inputs::default(),
        replaying: None,
    };
    tokio::spawn(pump.run(opening))
}

/// An attachment's replay in progress.
struct Replaying {
    show: Show,
    /// The title to send a host shown history once it is through; cleared
    /// when the history carried one.
    title: Option<String>,
    /// The input requests open when the attachment began.
    inputs: Vec<InputRequest>,
    /// The job state the replay ends in, and the turn it is of, as the
    /// attach answered.
    state: JobState,
    turn: Option<TurnId>,
    done: Replayed,
    /// Released once the host has the response that names the session;
    /// only the attach that opened it has one.
    gate: Option<oneshot::Receiver<()>>,
}

/// The prompt of this host's that the pump waits on, by its turn.
struct Armed {
    /// The turn the daemon answered the prompt with.
    turn: TurnId,
    waiter: Waiter,
    /// Whether the turn's user message, the prompt's own, went by.
    shown: bool,
}

struct Pump {
    context: InputContext,
    attacher: Arc<dyn Attacher>,
    inbound: mpsc::UnboundedReceiver<Inbound>,
    updates: Updates,
    shared: Arc<Shared>,
    setup: SessionSetup,
    herdr: HerdrStatus,
    /// The turn the session's job is in or last ran, as its last state
    /// said.
    turn: Option<TurnId>,
    waiter: Option<Armed>,
    inputs: Inputs,
    position: StreamPosition,
    replaying: Option<Replaying>,
}

impl Pump {
    async fn run(mut self, opening: Opening) {
        let Opening {
            response,
            show,
            replayed,
            gate,
        } = opening;
        self.begin(response, show, replayed, Some(gate)).await;
        while let Some(next) = self.inbound.recv().await {
            match next {
                Inbound::Routed(routed) => self.routed(routed).await,
                Inbound::Control(control) => self.control(control).await,
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
                Ok((response, show)) => self.begin(response, show, replayed, /*gate*/ None).await,
                Err(error) => {
                    let _ = replayed.send(Err(error));
                }
            },
            // Waits for the daemon's answer to the prompt before it reads
            // on: the turn's events, which queue behind this, are then told
            // from every other turn's by its id. A prompt the daemon
            // refused, or one that ended before it began, has no turn to
            // wait on.
            Control::Arm { waiter, turn } if self.attached() => {
                if let Ok(turn) = turn.await {
                    self.waiter = Some(Armed {
                        turn,
                        waiter,
                        shown: false,
                    });
                }
            }
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

    /// Starts relaying the attachment `response` began.
    async fn begin(
        &mut self,
        response: SessionAttachResponse,
        show: Show,
        done: Replayed,
        gate: Option<oneshot::Receiver<()>>,
    ) {
        self.position.reattached(&response);
        self.options_changed(response.options).await;
        self.shared
            .attachment
            .send_replace(Attachment::Attached(response.state));
        self.herdr.report(response.state).await;
        // A reset starts over with the saved conversation, whose messages
        // are of turns its states have yet to name.
        if response.replay == Replay::Reset {
            self.turn = None;
        }
        self.replaying = Some(Replaying {
            show,
            title: response.title,
            inputs: response.inputs,
            state: response.state,
            turn: response.turn,
            done,
            gate,
        });
        if !self.position.replaying() {
            self.end_replay().await;
        }
    }

    async fn event(&mut self, seq: Seq, event: SessionEvent) {
        let left = match self.position.observe(seq) {
            Part::Live => return self.live(event).await,
            Part::Replayed { left, .. } => left,
        };
        let Some(replaying) = self.replaying.as_mut() else {
            return;
        };
        match replaying.show.clone() {
            Show::History(port) => {
                if matches!(event, SessionEvent::TitleChanged { .. }) {
                    replaying.title = None;
                }
                for update in self.updates.map(&event) {
                    self.send(&*port, update, Delivery::Replay).await;
                }
            }
            Show::Missed => self.live(event).await,
            // The host has the history; only the armed prompt reads it.
            Show::Skip => {
                self.follow_prompt(&event);
            }
        }
        if left == 0 {
            self.end_replay().await;
        }
    }

    /// Sends the title a history may lack, fails a prompt whose turn's
    /// end a skipped replay no longer held, answers whoever waits for the
    /// replay, waits for the host's response, then puts the open input
    /// requests to the host.
    async fn end_replay(&mut self) {
        let Some(Replaying {
            show,
            title,
            inputs,
            state,
            turn,
            done,
            gate,
        }) = self.replaying.take()
        else {
            return;
        };
        self.turn = turn;
        match (show, title) {
            (Show::History(port), Some(title)) => {
                let update =
                    SessionUpdate::SessionInfoUpdate(SessionInfoUpdate::new().title(title));
                self.send(&*port, update, Delivery::Replay).await;
            }
            (Show::Skip, _) => self.fail_unreplayed(state),
            (Show::History(_) | Show::Missed, _) => {}
        }
        let _ = done.send(Ok(()));
        if let Some(gate) = gate {
            // A request that failed to respond drops the gate; the events
            // still belong to the session.
            let _ = gate.await;
        }
        for request in inputs {
            self.inputs.open(request, &self.context);
        }
    }

    /// A skipped reset replay is over, ending with the job in `state`, of
    /// the turn the attach named, now `self.turn`. It held the armed prompt's turn's end
    /// unless another turn saved since: a prompt still armed whose turn is
    /// no longer the one under way ended unseen, and fails.
    fn fail_unreplayed(&mut self, state: JobState) {
        let under_way = busy(state).then_some(self.turn.as_ref()).flatten();
        if self
            .waiter
            .as_ref()
            .is_some_and(|armed| under_way != Some(&armed.turn))
        {
            self.fail(AcpError::internal_error().data(
                "the prompt's turn ended while this client was behind, and the rho daemon no longer replays how",
            ));
        }
    }

    async fn live(&mut self, event: SessionEvent) {
        let own = self.follow_prompt(&event);
        match event {
            // The host already shows its own prompt.
            SessionEvent::UserMessage { .. } if own => {}
            // The armed prompt's, or one no prompt of this host waits on.
            SessionEvent::TurnEnded { .. } => {}
            SessionEvent::State { state, .. } => {
                self.shared
                    .attachment
                    .send_replace(Attachment::Attached(state));
                self.herdr.report(state).await;
            }
            SessionEvent::InputRequested { request } => self.inputs.open(*request, &self.context),
            SessionEvent::InputResolved { id, .. } => self.inputs.resolve(id),
            SessionEvent::OptionsChanged { options } => self.options_changed(*options).await,
            // Another client's prompt, or a steer.
            event => self.send_mapped(&event).await,
        }
    }

    /// Reads what `event` says of the armed prompt's turn: the turn's first
    /// user message that is no steer is the prompt's own, which accepts it,
    /// and the turn's end ends it. Returns whether `event` is that user
    /// message. A prompt that gave up (its receiver is gone) is dropped.
    fn follow_prompt(&mut self, event: &SessionEvent) -> bool {
        if let SessionEvent::State { turn, .. } = event {
            self.turn.clone_from(turn);
        }
        if self
            .waiter
            .as_ref()
            .is_some_and(|armed| armed.waiter.outcome.is_closed())
        {
            self.waiter = None;
        }
        let Some(armed) = self.waiter.as_mut() else {
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
            SessionEvent::TurnEnded { turn, outcome } if *turn == armed.turn => {
                if let Some(armed) = self.waiter.take() {
                    let _ = armed.waiter.outcome.send(Ok(outcome.clone()));
                }
                false
            }
            _ => false,
        }
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
            // Fell behind: resume after the cursor at once. Nobody waits
            // for this replay.
            DetachReason::Lagged => match self.reattach().await {
                Ok((response, show)) => {
                    let (replayed, _) = oneshot::channel();
                    self.begin(response, show, replayed, /*gate*/ None).await;
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
    /// with `error`, and the host's input prompts are withdrawn.
    fn end(&mut self, attachment: Attachment, error: AcpError) {
        self.fail(error);
        self.inputs.abort_all();
        self.replaying = None;
        self.shared.attachment.send_replace(attachment);
    }

    /// Fails the armed prompt, if any, with `error`.
    fn fail(&mut self, error: AcpError) {
        if let Some(armed) = self.waiter.take() {
            let _ = armed.waiter.outcome.send(Err(error));
        }
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

#[cfg(test)]
#[path = "pump_tests.rs"]
mod tests;
