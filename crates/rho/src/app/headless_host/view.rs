//! How a headless prompt shows: the [`TurnView`] every headless host lends
//! [`SessionRuntime::run_prompt_turn`]. It writes the session's events to
//! the JSONL stream as the daemon protocol's (#1669) and its run events to
//! the status file, and answers the turn's requests.
//! Nobody is there to approve, so an approval or plan that reaches it
//! anyway (an unattended session sends none) is refused. A delegated child
//! whose parent listens forwards its questions there; any other question
//! was declined by the turn already.

use std::{collections::HashMap, future::Future, pin::Pin};

use futures_util::{stream::FuturesUnordered, StreamExt};
use rho_sdk::{
    CancellationToken, HostInputId, HostInputRefusal, HostInputRequest, HostInputResponse,
    SessionId, UserInput,
};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use super::RunReporter;
use crate::{
    app::{
        automation_protocol::JsonlStream,
        session_events::LiveEvents,
        session_runtime::{
            cancelled_approval, PlanReport, SessionEvent, SessionRuntime, TurnControl, TurnIo,
            TurnView,
        },
        subagent_host_input::SubagentHostInputBridge,
    },
    config::Config,
    tools::plan_exit::PlanPermissionAnswer,
};

/// Keeps the status file fresh while a provider or tool call emits no events.
const REPORT_HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(10);

/// Where one headless prompt's events go.
pub(crate) struct HeadlessView<'a> {
    reporter: Option<&'a mut RunReporter>,
    jsonl: Option<JsonlView<'a>>,
    /// The parent that answers a delegated child's questions.
    parent: Option<ParentQuestions>,
    /// Why the JSONL stream stopped, once a write to it failed.
    output_failure: Option<String>,
}

impl<'a> HeadlessView<'a> {
    /// A view that shows nothing until told where to.
    pub(crate) fn new() -> Self {
        Self {
            reporter: None,
            jsonl: None,
            parent: None,
            output_failure: None,
        }
    }

    /// Also keeps `reporter`'s status file and event log.
    pub(crate) fn reporting_to(mut self, reporter: &'a mut RunReporter) -> Self {
        self.reporter = Some(reporter);
        self
    }

    /// Also writes the session's events to `stream`.
    pub(crate) fn streaming_jsonl(mut self, stream: &'a mut JsonlStream) -> Self {
        self.jsonl = Some(JsonlView { stream, live: None });
        self
    }

    /// Forwards the session's questions to `parent`.
    pub(crate) fn with_parent_questions(mut self, parent: Option<ParentQuestions>) -> Self {
        self.parent = parent;
        self
    }

    /// Names `runtime`'s session and tools in what the view writes.
    /// [`super::run_prompt`] binds the view once the session is open, before
    /// its prompt begins; a prompt that rebinds the session's agent binds it
    /// again.
    pub(crate) fn bind(&mut self, runtime: &SessionRuntime) {
        if let Some(jsonl) = &mut self.jsonl {
            let workspace = runtime.workspace_root();
            jsonl.live = Some(LiveEvents::new(workspace, runtime.tools()));
            let session = runtime.session_id().to_string();
            if let Err(error) = jsonl.stream.open(&session, workspace) {
                self.output_failure
                    .get_or_insert_with(|| output_failure(&error));
            }
        }
        if let Some(reporter) = self.reporter.as_deref_mut() {
            reporter.set_mcp_tools(runtime.tools().mcp_report().tool_names());
        }
    }

    /// Notes `input`, the display input of the turn about to begin, which
    /// the JSONL stream shows as the user message the turn saves.
    pub(crate) fn prompt(&mut self, input: &UserInput) {
        if let Some(live) = self.jsonl.as_mut().and_then(|jsonl| jsonl.live.as_mut()) {
            live.prompt(input);
        }
    }

    /// The message of the JSONL write that failed, which failed the turn.
    pub(crate) fn take_output_failure(&mut self) -> Option<String> {
        self.output_failure.take()
    }

    /// Shows one turn's `events` until the turn drops its sender, answering
    /// on `control`.
    async fn show(
        &mut self,
        mut events: UnboundedReceiver<SessionEvent>,
        control: UnboundedSender<TurnControl>,
    ) {
        if let Some(jsonl) = &mut self.jsonl {
            jsonl.stream.begin_turn();
        }
        // The stream broke before the turn began; the turn fails at once.
        if let Some(message) = &self.output_failure {
            let _ = control.send(TurnControl::HostFailed {
                message: message.clone(),
            });
        }
        let mut heartbeat = tokio::time::interval(REPORT_HEARTBEAT);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut asked = ParentAsks::default();
        loop {
            tokio::select! {
                biased;
                Some((id, answer)) = asked.pending.next() => {
                    asked.tokens.remove(&id);
                    let _ = control.send(TurnControl::AnswerQuestion { id, answer });
                }
                event = events.recv() => {
                    let Some(event) = event else {
                        break;
                    };
                    self.show_event(event, &control, &mut asked);
                }
                _ = heartbeat.tick(), if self.reporter.is_some() => {
                    if let Some(reporter) = self.reporter.as_deref_mut() {
                        reporter.write();
                    }
                }
            }
        }
        // The turn withdrew or answered every question before it ended;
        // stop any parent request still in flight.
        for token in asked.tokens.into_values() {
            token.cancel();
        }
    }

    fn show_event(
        &mut self,
        event: SessionEvent,
        control: &UnboundedSender<TurnControl>,
        asked: &mut ParentAsks,
    ) {
        self.stream_jsonl(&event, control);
        match event {
            SessionEvent::Run(event) => {
                if let Some(reporter) = self.reporter.as_deref_mut() {
                    reporter.on_event(&event);
                }
            }
            SessionEvent::ApprovalRequested { id, .. } => {
                let _ = control.send(TurnControl::RespondApproval {
                    id,
                    decision: cancelled_approval(),
                });
            }
            SessionEvent::PlanApprovalRequested { id, .. } => {
                let _ = control.send(TurnControl::AnswerPlan {
                    id,
                    answer: PlanPermissionAnswer::KeepPlanning,
                });
            }
            SessionEvent::QuestionAsked { id, request, .. } => match &self.parent {
                Some(parent) => asked.ask(parent, id, request),
                None => {
                    let _ = control.send(TurnControl::AnswerQuestion {
                        id,
                        answer: Err(HostInputRefusal::Declined),
                    });
                }
            },
            SessionEvent::QuestionWithdrawn { id } => {
                if let Some(token) = asked.tokens.remove(&id) {
                    token.cancel();
                }
            }
            SessionEvent::MessageInserted(_)
            | SessionEvent::RunStarted
            | SessionEvent::BoundaryApplied { .. }
            | SessionEvent::ApprovalWithdrawn { .. }
            | SessionEvent::PlanApprovalWithdrawn { .. }
            | SessionEvent::Notice { .. }
            | SessionEvent::RunClosed { .. }
            | SessionEvent::RunFinished { .. } => {}
        }
    }

    /// Writes `event` to the JSONL stream as its wire events. A failed
    /// write fails the turn and ends the stream.
    fn stream_jsonl(&mut self, event: &SessionEvent, control: &UnboundedSender<TurnControl>) {
        let Some(JsonlView {
            stream,
            live: Some(live),
        }) = &mut self.jsonl
        else {
            return;
        };
        if self.output_failure.is_some() {
            return;
        }
        for wire in live.map(event) {
            if let Err(error) = stream.event(&wire) {
                let message = output_failure(&error);
                let _ = control.send(TurnControl::HostFailed {
                    message: message.clone(),
                });
                self.output_failure = Some(message);
                return;
            }
        }
    }
}

/// The message of a failed JSONL write, which fails the run.
fn output_failure(error: &std::io::Error) -> String {
    format!("could not write JSONL output: {error}")
}

/// The JSONL stream a view writes, and the mapper that turns the session's
/// events into the wire events it carries.
struct JsonlView<'a> {
    stream: &'a mut JsonlStream,
    /// The session's mapper, once [`HeadlessView::bind`] opened it.
    live: Option<LiveEvents>,
}

impl<'a> TurnView for HeadlessView<'a> {
    fn open_turn<'v>(
        &'v mut self,
        _runtime: &SessionRuntime,
    ) -> (TurnIo, impl Future<Output = ()> + Send + use<'v, 'a>) {
        let (events, event_receiver) = mpsc::unbounded_channel();
        let (control_sender, control) = mpsc::unbounded_channel();
        (
            TurnIo { events, control },
            self.show(event_receiver, control_sender),
        )
    }

    /// Unattended sessions offer no plan exit, so there is no handoff to
    /// show.
    fn report_plan(
        &mut self,
        _runtime: &SessionRuntime,
        _process_config: &Config,
        _report: PlanReport,
    ) -> impl Future<Output = ()> + Send {
        std::future::ready(())
    }
}

/// The parent session a delegated child's questions go to, through the
/// bridge its TUI listens on.
pub(crate) struct ParentQuestions {
    pub(crate) bridge: SubagentHostInputBridge,
    pub(crate) run_id: String,
    pub(crate) agent_id: String,
    pub(crate) parent: SessionId,
}

type Answer = Result<HostInputResponse, HostInputRefusal>;
type AskFuture = Pin<Box<dyn Future<Output = (HostInputId, Answer)> + Send>>;

/// The questions waiting on the parent, each with the token that stops its
/// request once the turn withdraws it.
#[derive(Default)]
struct ParentAsks {
    pending: FuturesUnordered<AskFuture>,
    tokens: HashMap<HostInputId, CancellationToken>,
}

impl ParentAsks {
    fn ask(&mut self, parent: &ParentQuestions, id: HostInputId, request: HostInputRequest) {
        let token = CancellationToken::new();
        self.tokens.insert(id.clone(), token.clone());
        let bridge = parent.bridge.clone();
        let (run_id, agent_id, parent) = (
            parent.run_id.clone(),
            parent.agent_id.clone(),
            parent.parent.clone(),
        );
        self.pending.push(Box::pin(async move {
            let answer = bridge
                .request(run_id, agent_id, parent, request, &token)
                .await
                .map_err(|error| match error {
                    rho_sdk::Error::Cancelled => HostInputRefusal::Dismissed,
                    error => {
                        tracing::debug!(%error, "parent did not answer a delegated question");
                        HostInputRefusal::Declined
                    }
                });
            (id, answer)
        }));
    }
}

#[cfg(test)]
#[path = "view_tests.rs"]
mod tests;
