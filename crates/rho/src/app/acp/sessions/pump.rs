//! A session's pump: the one task that relays its attachments' events to
//! the host, in order, with the [`Control`]s its requests hand it.
//!
//! The pump owns the session's place in the stream ([`StreamPosition`]),
//! so it alone attaches again: when a request finds the session stale
//! ([`Control::Resume`]), and at once when the daemon drops an attachment
//! that fell behind (`Lagged`).

use std::sync::{Arc, PoisonError};

use agent_client_protocol::{
    schema::v1::{MessageId, SessionInfoUpdate, SessionUpdate},
    Error as AcpError,
};
use rho_protocol::{
    self as wire, DetachReason, InputRequest, Seq, SessionAttachResponse, SessionEvent,
    SessionOptions, SessionSetup, TurnOutcome,
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
        daemon_client::{HerdrStatus, Part, ReplayedTurn, ResetReplay, Routed, StreamPosition},
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
    /// What a skipped reset replay holds of the last turn.
    replay: ResetReplay,
    done: Replayed,
    /// Released once the host has the response that names the session;
    /// only the attach that opened it has one.
    gate: Option<oneshot::Receiver<()>>,
}

/// The prompt of this host's that the pump waits on.
enum Armed {
    /// Its user message has not arrived.
    Waiting(Waiter),
    /// Its turn is under way.
    Started(Waiter),
}

impl Armed {
    fn into_waiter(self) -> Waiter {
        match self {
            Self::Waiting(waiter) | Self::Started(waiter) => waiter,
        }
    }
}

struct Pump {
    context: InputContext,
    attacher: Arc<dyn Attacher>,
    inbound: mpsc::UnboundedReceiver<Inbound>,
    updates: Updates,
    shared: Arc<Shared>,
    setup: SessionSetup,
    herdr: HerdrStatus,
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
            Control::Arm(waiter) if self.attached() => self.waiter = Some(Armed::Waiting(waiter)),
            // The attachment ended between the prompt's reattach and now.
            Control::Arm(waiter) => {
                let _ = waiter.outcome.send(Err(daemon::lost()));
            }
            Control::Options(options, told) => {
                self.options_changed(options).await;
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
        self.replaying = Some(Replaying {
            show,
            title: response.title,
            inputs: response.inputs,
            replay: ResetReplay::new(response.state),
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
            Show::Skip => replaying.replay.observe(&event),
        }
        if left == 0 {
            self.end_replay().await;
        }
    }

    /// Sends the title a history may lack, settles a prompt the skipped
    /// replay ended or started, answers whoever waits for the replay, waits
    /// for the host's response, then puts the open input requests to the
    /// host.
    async fn end_replay(&mut self) {
        let Some(Replaying {
            show,
            title,
            inputs,
            replay,
            done,
            gate,
        }) = self.replaying.take()
        else {
            return;
        };
        match (show, title) {
            (Show::History(port), Some(title)) => {
                let update =
                    SessionUpdate::SessionInfoUpdate(SessionInfoUpdate::new().title(title));
                self.send(&*port, update, Delivery::Replay).await;
            }
            (Show::Skip, _) => self.settle_replayed(replay),
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

    /// Settles the armed prompt by a reset replay that was skipped, which
    /// holds the whole saved conversation, and the last turn's end until the
    /// next turn saves: a turn that ended with the job idle ends the prompt,
    /// and a user message the running job began with starts it. Otherwise
    /// the prompt's turn shows as live events. A prompt the daemon has not
    /// saved yet, while the job is idle, is taken to be the turn that ended:
    /// the replay cannot tell them apart.
    fn settle_replayed(&mut self, ResetReplay { state, turn }: ResetReplay) {
        match (turn, busy(state)) {
            (ReplayedTurn::Ended(outcome), false) => {
                if let Some(armed) = self.waiter.take() {
                    let _ = armed.into_waiter().outcome.send(Ok(outcome));
                }
            }
            (ReplayedTurn::Began(message), true) => {
                self.start_turn(&message);
            }
            (ReplayedTurn::Unseen | ReplayedTurn::Began(_), _) | (ReplayedTurn::Ended(_), true) => {
            }
        }
    }

    async fn live(&mut self, event: SessionEvent) {
        match event {
            SessionEvent::UserMessage {
                message,
                content,
                steer,
            } => {
                // A steer applies within the turn under way; it never
                // starts one.
                if steer.is_some() || !self.start_turn(&message) {
                    // Another client's prompt or steer.
                    let event = SessionEvent::UserMessage {
                        message,
                        content,
                        steer,
                    };
                    self.send_mapped(&event).await;
                }
            }
            SessionEvent::TurnEnded { outcome } => self.end_turn(outcome),
            SessionEvent::State { state } => {
                self.shared
                    .attachment
                    .send_replace(Attachment::Attached(state));
                self.herdr.report(state).await;
            }
            SessionEvent::InputRequested { request } => self.inputs.open(*request, &self.context),
            SessionEvent::InputResolved { id, .. } => self.inputs.resolve(id),
            SessionEvent::OptionsChanged { options } => self.options_changed(options).await,
            event => self.send_mapped(&event).await,
        }
    }

    /// The user message `message` arrived: the armed prompt's when it
    /// waits for one, which starts its turn and accepts it. A waiting
    /// prompt that gave up is dropped. Returns whether it was the prompt's.
    fn start_turn(&mut self, message: &wire::MessageId) -> bool {
        match self.waiter.take() {
            Some(Armed::Waiting(waiter)) if !waiter.outcome.is_closed() => {
                let accept = waiter
                    .accept
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take();
                if let Some(accept) = accept {
                    accept(MessageId::new(message.as_str()));
                }
                self.waiter = Some(Armed::Started(waiter));
                true
            }
            Some(Armed::Waiting(_)) => false,
            armed @ (Some(Armed::Started(_)) | None) => {
                self.waiter = armed;
                false
            }
        }
    }

    /// A turn ended `outcome`: the armed prompt's, once its turn started.
    fn end_turn(&mut self, outcome: TurnOutcome) {
        match self.waiter.take() {
            Some(Armed::Started(waiter)) => {
                let _ = waiter.outcome.send(Ok(outcome));
            }
            armed @ (Some(Armed::Waiting(_)) | None) => self.waiter = armed,
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
            DetachReason::Deleted => {
                let error = AcpError::resource_not_found(Some(self.context.acp_id.0.to_string()));
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
        if let Some(armed) = self.waiter.take() {
            let _ = armed.into_waiter().outcome.send(Err(error));
        }
        self.inputs.abort_all();
        self.replaying = None;
        self.shared.attachment.send_replace(attachment);
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
