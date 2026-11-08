//! A session's pump: the one task that relays its attachment's events to
//! the host, in order, with the [`Control`]s its requests hand it.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, PoisonError},
};

use agent_client_protocol::{
    schema::v1::{MessageId, SessionInfoUpdate, SessionUpdate},
    Error as AcpError,
};
use rho_protocol::{
    methods::SessionAttach, ClientEnvironment, Cursor, DetachReason, InputRequest, JobState,
    Replay, Seq, SessionAttachParams, SessionEvent, SessionId, SessionOptions, SessionSetup,
    StreamId,
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};

use super::{lock, Attached, Attachment, Control, Shared, Show, Waiter};
use crate::{
    app::acp::{
        daemon::{self, Inbound},
        inputs::{InputContext, Inputs},
        updates::{self, Updates},
        AcpClientPort,
    },
    herdr::{HerdrReporter, HerdrSession, HerdrState},
};

/// Starts the pump of session `context.session`, which waits for its
/// first [`Control::Attached`] on `inbound`.
pub(super) fn spawn(
    context: InputContext,
    inbound: mpsc::UnboundedReceiver<Inbound>,
    shared: Arc<Mutex<Shared>>,
    setup: SessionSetup,
    job: watch::Sender<JobState>,
    herdr: &HerdrReporter,
) -> JoinHandle<()> {
    let pump = Pump {
        updates: Updates::new(context.features),
        herdr: Herdr::new(herdr, &context.session),
        context,
        inbound,
        buffer: VecDeque::new(),
        shared,
        setup,
        job,
        waiter: None,
        inputs: Inputs::default(),
        replaying: None,
        stream: None,
        last_seq: 0,
    };
    tokio::spawn(pump.run())
}

/// The pane state Herdr shows for a session's job.
fn herdr_state(state: JobState) -> HerdrState {
    match state {
        JobState::Running | JobState::Queued => HerdrState::Working,
        JobState::WaitingForInput => HerdrState::Blocked,
        JobState::Idle
        | JobState::Completed
        | JobState::Failed
        | JobState::Cancelled
        | JobState::TimedOut
        | JobState::Interrupted => HerdrState::Idle,
        // `Unknown`, or a state a newer daemon sends.
        _ => HerdrState::Idle,
    }
}

/// Reports a session's job state to Herdr when it changes.
struct Herdr {
    reporter: HerdrReporter,
    session: HerdrSession,
    last: Option<HerdrState>,
}

impl Herdr {
    fn new(reporter: &HerdrReporter, id: &SessionId) -> Self {
        Self {
            reporter: reporter.clone(),
            session: HerdrSession::without_resume(id.as_str()),
            last: None,
        }
    }

    async fn report(&mut self, state: JobState) {
        let state = herdr_state(state);
        if !self.reporter.is_enabled() || self.last == Some(state) {
            return;
        }
        self.last = Some(state);
        let _ = self
            .reporter
            .report_state(state, None, Some(&self.session))
            .await;
    }
}

/// An attachment's replay in progress.
struct Replaying {
    remaining: u64,
    show: Show,
    titled: bool,
    title: Option<String>,
    inputs: Vec<InputRequest>,
    done: Option<oneshot::Sender<()>>,
    gate: Option<oneshot::Receiver<()>>,
}

/// How an attachment ended.
enum Ended {
    /// Wait for the next attachment.
    Stale,
    /// The session is gone, or no request can reach the pump any more.
    Gone,
}

struct Pump {
    context: InputContext,
    inbound: mpsc::UnboundedReceiver<Inbound>,
    /// Events read before their attachment's start.
    buffer: VecDeque<Inbound>,
    updates: Updates,
    shared: Arc<Mutex<Shared>>,
    setup: SessionSetup,
    job: watch::Sender<JobState>,
    herdr: Herdr,
    waiter: Option<Armed>,
    inputs: Inputs,
    replaying: Option<Replaying>,
    stream: Option<StreamId>,
    last_seq: u64,
}

struct Armed {
    waiter: Waiter,
    /// The prompt's user message arrived.
    started: bool,
}

impl Pump {
    async fn run(mut self) {
        loop {
            let Some(attached) = self.next_attachment().await else {
                return;
            };
            self.begin(attached).await;
            match self.relay().await {
                Ended::Stale => {}
                Ended::Gone => return,
            }
        }
    }

    /// Waits for an attachment to start, keeping the events that arrive
    /// before its start.
    async fn next_attachment(&mut self) -> Option<Attached> {
        self.buffer.clear();
        loop {
            match self.inbound.recv().await? {
                Inbound::Session(Control::Attached(attached)) => return Some(attached),
                Inbound::Session(Control::Arm(waiter)) => {
                    let _ = waiter.outcome.send(Err(daemon::lost()));
                }
                Inbound::Session(Control::Options(options, told)) => {
                    self.options_changed(options).await;
                    let _ = told.send(());
                }
                event @ Inbound::Event { .. } => self.buffer.push_back(event),
                Inbound::Detached(_) | Inbound::Lost => self.buffer.clear(),
            }
        }
    }

    async fn begin(&mut self, attached: Attached) {
        let reset = matches!(attached.replay, Replay::Reset);
        if reset || self.stream.as_ref() != Some(&attached.stream) {
            self.last_seq = 0;
        }
        self.stream = Some(attached.stream);
        {
            let mut shared = lock(&self.shared);
            shared.attachment = Attachment::Attached;
            shared.options = attached.options;
        }
        self.job.send_replace(attached.state);
        self.herdr.report(attached.state).await;
        self.replaying = Some(Replaying {
            remaining: attached.replay_events,
            show: attached.show,
            titled: false,
            title: attached.title,
            inputs: attached.inputs,
            done: attached.done,
            gate: attached.gate,
        });
        if attached.replay_events == 0 {
            self.end_replay().await;
        }
    }

    /// Relays the attachment's events until it ends.
    async fn relay(&mut self) -> Ended {
        loop {
            let next = match self.buffer.pop_front() {
                Some(next) => next,
                None => match self.inbound.recv().await {
                    Some(next) => next,
                    None => return Ended::Gone,
                },
            };
            match next {
                Inbound::Event { seq, event } => {
                    self.track(seq);
                    self.event(event).await;
                }
                Inbound::Session(Control::Arm(waiter)) => {
                    self.waiter = Some(Armed {
                        waiter,
                        started: false,
                    });
                }
                Inbound::Session(Control::Options(options, told)) => {
                    self.options_changed(options).await;
                    let _ = told.send(());
                }
                // A request attached again; the old attachment is over.
                Inbound::Session(Control::Attached(attached)) => self.begin(attached).await,
                Inbound::Detached(reason) => {
                    if let Some(ended) = self.detached(reason).await {
                        return ended;
                    }
                }
                Inbound::Lost => return self.stale(daemon::lost()),
            }
        }
    }

    fn track(&mut self, seq: u64) {
        if seq > self.last_seq {
            self.last_seq = seq;
        }
        if let (Some(stream), true) = (&self.stream, self.last_seq > 0) {
            lock(&self.shared).cursor = Some(Cursor {
                stream: stream.clone(),
                seq: Seq(self.last_seq),
            });
        }
    }

    async fn event(&mut self, event: SessionEvent) {
        let Some(replaying) = self.replaying.as_mut() else {
            return self.live(event).await;
        };
        replaying.remaining = replaying.remaining.saturating_sub(1);
        match &replaying.show {
            Show::History(port) => {
                if matches!(event, SessionEvent::TitleChanged { .. }) {
                    replaying.titled = true;
                }
                let port = Arc::clone(port);
                for update in self.updates.map(&event) {
                    self.send(&*port, update, /*replayed*/ true).await;
                }
            }
            Show::Missed => self.live(event).await,
            Show::Skip => {}
        }
        if self.replaying.as_ref().is_some_and(|replay| replay.remaining == 0) {
            self.end_replay().await;
        }
    }

    /// Sends the title a reset's history may lack, answers the request,
    /// waits for the host's response, then puts the open input requests to
    /// the host.
    async fn end_replay(&mut self) {
        let Some(replaying) = self.replaying.take() else {
            return;
        };
        if let (Show::History(port), false, Some(title)) =
            (&replaying.show, replaying.titled, replaying.title)
        {
            let update = SessionUpdate::SessionInfoUpdate(SessionInfoUpdate::new().title(title));
            self.send(&**port, update, /*replayed*/ true).await;
        }
        if let Some(done) = replaying.done {
            let _ = done.send(());
        }
        if let Some(gate) = replaying.gate {
            // A request that failed to respond drops the gate; the events
            // still belong to whoever attaches next.
            let _ = gate.await;
        }
        for request in replaying.inputs {
            self.inputs.open(request, &self.context);
        }
    }

    async fn live(&mut self, event: SessionEvent) {
        match event {
            SessionEvent::UserMessage { message, content } => {
                if let Some(armed) = self.armed().filter(|armed| !armed.started) {
                    armed.started = true;
                    let accept = armed
                        .waiter
                        .accept
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .take();
                    if let Some(accept) = accept {
                        accept(MessageId::new(message.as_str()));
                    }
                    return;
                }
                // Another client's prompt.
                let event = SessionEvent::UserMessage { message, content };
                self.send_mapped(&event).await;
            }
            SessionEvent::TurnEnded { outcome } => {
                if self.armed().is_some_and(|armed| armed.started) {
                    if let Some(armed) = self.waiter.take() {
                        let _ = armed.waiter.outcome.send(Ok(outcome));
                    }
                }
            }
            SessionEvent::State { state } => {
                self.job.send_replace(state);
                self.herdr.report(state).await;
            }
            SessionEvent::InputRequested { request } => self.inputs.open(*request, &self.context),
            SessionEvent::InputResolved { id, .. } => self.inputs.resolve(id),
            SessionEvent::OptionsChanged { options } => self.options_changed(options).await,
            event => self.send_mapped(&event).await,
        }
    }

    /// The armed waiter, unless its prompt gave up.
    fn armed(&mut self) -> Option<&mut Armed> {
        if self
            .waiter
            .as_ref()
            .is_some_and(|armed| armed.waiter.outcome.is_closed())
        {
            self.waiter = None;
        }
        self.waiter.as_mut()
    }

    async fn options_changed(&mut self, options: SessionOptions) {
        let old = std::mem::replace(&mut lock(&self.shared).options, options.clone());
        let port = Arc::clone(&self.context.port);
        for update in updates::options_changed(&old, &options, self.context.features) {
            self.send(&*port, update, /*replayed*/ false).await;
        }
    }

    /// Returns how the attachment ended, or `None` when it goes on.
    async fn detached(&mut self, reason: DetachReason) -> Option<Ended> {
        match reason {
            // Fell behind: resume after the cursor at once.
            DetachReason::Lagged => match self.attach_again().await {
                Ok(()) => None,
                Err(error) => Some(self.stale(error)),
            },
            DetachReason::DaemonStopping => Some(
                self.stale(AcpError::internal_error().data("rho daemon is stopping")),
            ),
            DetachReason::Deleted => {
                self.stale(AcpError::resource_not_found(Some(
                    self.context.acp_id.0.to_string(),
                )));
                lock(&self.shared).attachment = Attachment::Deleted;
                Some(Ended::Gone)
            }
            // `Unknown`, or a reason a newer daemon sends: attach again on
            // the next request.
            _ => Some(self.stale(daemon::lost())),
        }
    }

    async fn attach_again(&mut self) -> Result<(), AcpError> {
        let cursor = lock(&self.shared).cursor.clone();
        let response = self
            .context
            .daemon
            .request::<SessionAttach>(&SessionAttachParams {
                session: self.context.session.clone(),
                after: cursor,
                environment: ClientEnvironment::of_this_process(),
                setup: self.setup.clone(),
            })
            .await?;
        let show = match response.replay {
            Replay::Tail => Show::Missed,
            Replay::Reset => Show::Skip,
        };
        let (attached, _) = Attached::new(response, show, /*gate*/ None);
        self.begin(attached).await;
        Ok(())
    }

    /// Ends the attachment: the prompt waiting on it fails with `error`,
    /// and the host's input prompts are withdrawn.
    fn stale(&mut self, error: AcpError) -> Ended {
        if let Some(armed) = self.waiter.take() {
            let _ = armed.waiter.outcome.send(Err(error));
        }
        self.inputs.abort_all();
        self.replaying = None;
        lock(&self.shared).attachment = Attachment::Stale;
        Ended::Stale
    }

    async fn send_mapped(&mut self, event: &SessionEvent) {
        let port = Arc::clone(&self.context.port);
        for update in self.updates.map(event) {
            self.send(&*port, update, /*replayed*/ false).await;
        }
    }

    async fn send(&self, port: &dyn AcpClientPort, update: SessionUpdate, replayed: bool) {
        let notification = updates::notification(&self.context.acp_id, update, replayed);
        if let Err(error) = port.send_session_notification(notification).await {
            tracing::warn!(%error, "could not send an ACP session update");
        }
    }
}

