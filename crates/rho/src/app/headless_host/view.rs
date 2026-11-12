//! How a headless prompt shows: the [`TurnView`] every headless host lends
//! [`SessionRuntime::run_prompt_turn`]. It writes the session's events as
//! the daemon protocol's, to `rho run`'s JSONL stream (#1669) or to the host
//! that follows a child run (#1668), and its run events to the status file,
//! and answers the turn's requests.
//! Nobody is there to approve, so an approval or plan that reaches it
//! anyway (an unattended session sends none) is refused. A delegated child
//! whose parent listens forwards its questions there; any other question
//! was declined by the turn already.

use std::{collections::HashMap, future::Future, pin::Pin};

use futures_util::{stream::FuturesUnordered, StreamExt};
use rho_protocol::{SessionEvent as WireEvent, TurnOutcome};
use rho_sdk::{
    CancellationToken, HostInputId, HostInputRefusal, HostInputRequest, HostInputResponse,
    SessionId, UserInput,
};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use super::RunReporter;
use crate::{
    app::{
        automation_protocol::JsonlStream,
        session_commands::{CommandSet, Hosting},
        session_events::LiveEvents,
        session_options,
        session_runtime::{
            cancelled_approval, PlanReport, SessionEvent, SessionRuntime, TurnControl, TurnIo,
            TurnView,
        },
        subagent_host_input::SubagentHostInputBridge,
        subagent_pool::{ChildEvent, ChildEventSender},
    },
    config::Config,
    tools::plan_exit::PlanExitDecision,
};

/// A child run's prompt is its parent's task, which runs no command.
const CHILD_COMMANDS: CommandSet = CommandSet {
    built_ins: &[],
    workflows: Hosting::Refused,
    agents: Hosting::Refused,
    sessions_label: "subagent runs",
};

/// Keeps the status file fresh while a provider or tool call emits no events.
const REPORT_HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(10);

/// Where one headless prompt's events go.
pub(crate) struct HeadlessView<'a> {
    reporter: Option<&'a mut RunReporter>,
    wire: Option<WireView<'a>>,
    /// The parent that answers a delegated child's questions.
    parent: Option<ParentQuestions>,
}

impl<'a> HeadlessView<'a> {
    /// A view that shows nothing until told where to.
    pub(crate) fn new() -> Self {
        Self {
            reporter: None,
            wire: None,
            parent: None,
        }
    }

    /// Also keeps `reporter`'s status file and event log.
    pub(crate) fn reporting_to(mut self, reporter: &'a mut RunReporter) -> Self {
        self.reporter = Some(reporter);
        self
    }

    /// Also writes the session's events to `stream`.
    pub(crate) fn streaming_jsonl(mut self, stream: &'a mut JsonlStream) -> Self {
        self.wire = Some(WireView::to(WireSink::Jsonl(stream)));
        self
    }

    /// Also sends the session's events to `host`, which follows the run.
    pub(crate) fn streaming_to(mut self, host: ChildEventSender) -> Self {
        self.wire = Some(WireView::to(WireSink::Host(host)));
        self
    }

    /// Forwards the session's questions to `parent`.
    pub(crate) fn with_parent_questions(mut self, parent: Option<ParentQuestions>) -> Self {
        self.parent = parent;
        self
    }

    /// Names `runtime`'s session and tools in what the view writes, and
    /// tells a host that follows the run the options it opened with, which
    /// `process_config` gives their choices. [`super::run_prompt`] binds the
    /// view once the session is open, before its prompt begins; a prompt
    /// that rebinds the session's agent binds it again.
    pub(crate) fn bind(&mut self, runtime: &SessionRuntime, process_config: &Config) {
        if let Some(wire) = &mut self.wire {
            wire.bind(runtime, process_config);
        }
        if let Some(reporter) = self.reporter.as_deref_mut() {
            reporter.set_mcp_tools(runtime.tools().mcp_report().tool_names());
        }
    }

    /// Notes `input`, the display input of the turn about to begin, which
    /// the session's events show as the user message the turn saves.
    pub(crate) fn prompt(&mut self, input: &UserInput) {
        if let Some(wire) = &mut self.wire {
            wire.prompt = Some(input.clone());
        }
    }

    /// Ends the JSONL stream's turn with `outcome`, as the daemon does: the
    /// prompt of every session the stream opened ends with a `turn_ended`.
    /// A host that follows a child run ends the run's turn itself.
    pub(crate) fn end_turn(&mut self, outcome: TurnOutcome) {
        if let Some(WireView {
            sink: WireSink::Jsonl(stream),
            live: Some(_),
            ..
        }) = &mut self.wire
        {
            // The stream holds one turn, which nothing else names.
            let turn = rho_protocol::TurnId::new(uuid::Uuid::new_v4().hyphenated().to_string());
            stream.event(&WireEvent::TurnEnded { turn, outcome });
        }
    }

    /// Why the JSONL stream stopped, once a write to it failed; that
    /// failed the turn.
    pub(crate) fn output_failure(&self) -> Option<String> {
        self.wire.as_ref()?.sink.failure().map(str::to_owned)
    }

    /// Shows one turn's `events` until the turn drops its sender, answering
    /// on `control`.
    async fn show(
        &mut self,
        mut events: UnboundedReceiver<SessionEvent>,
        control: UnboundedSender<TurnControl>,
    ) {
        if let Some(wire) = &mut self.wire {
            wire.open_turn(&control);
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
        if let Some(wire) = &mut self.wire {
            wire.show(&event, control);
        }
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
                    answer: PlanExitDecision::keep_planning(),
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
            | SessionEvent::RunClosed
            | SessionEvent::RunFinished { .. } => {}
        }
    }
}

/// Where a view's wire events go.
enum WireSink<'a> {
    /// `rho run`'s JSONL stream. A write that fails it fails the turn.
    Jsonl(&'a mut JsonlStream),
    /// The host that follows a child run. A host that stopped listening
    /// misses the rest; the run goes on.
    Host(ChildEventSender),
}

impl WireSink<'_> {
    fn send(&mut self, event: WireEvent) {
        match self {
            Self::Jsonl(stream) => stream.event(&event),
            Self::Host(host) => {
                let _ = host.send(ChildEvent::Session(event));
            }
        }
    }

    /// Why the sink takes no more events, once a write to it failed.
    fn failure(&self) -> Option<&str> {
        match self {
            Self::Jsonl(stream) => stream.failure(),
            Self::Host(_) => None,
        }
    }
}

/// The sink a view writes the session's events to, and the mapper that
/// turns them into the wire events it carries.
struct WireView<'a> {
    sink: WireSink<'a>,
    /// The session's mapper, once [`Self::bind`] bound the session.
    live: Option<LiveEvents>,
    /// The display input of the turn about to begin.
    prompt: Option<UserInput>,
}

impl<'a> WireView<'a> {
    fn to(sink: WireSink<'a>) -> Self {
        Self {
            sink,
            live: None,
            prompt: None,
        }
    }

    /// Opens the sink on `runtime`'s session the first time; a rebind,
    /// whose agent has tools of its own, maps with them from then on.
    fn bind(&mut self, runtime: &SessionRuntime, process_config: &Config) {
        let workspace = runtime.workspace_root();
        let live = LiveEvents::new(
            workspace,
            runtime.tools(),
            runtime.configured_context_window(),
        );
        if self.live.replace(live).is_some() {
            return;
        }
        match &mut self.sink {
            WireSink::Jsonl(stream) => {
                stream.run_started(&runtime.session_id().to_string(), workspace);
            }
            WireSink::Host(host) => {
                // No auths: a child's model is its agent's, which nobody
                // switches.
                let options = session_options::of(
                    runtime,
                    process_config,
                    /*available_auths*/ &[],
                    CHILD_COMMANDS,
                );
                let _ = host.send(ChildEvent::Opened(Box::new(options)));
            }
        }
    }

    /// Readies the mapper for the turn opening, which a sink that already
    /// failed fails at once.
    fn open_turn(&mut self, control: &UnboundedSender<TurnControl>) {
        if let (Some(live), Some(input)) = (&mut self.live, self.prompt.take()) {
            live.prompt(&input);
        }
        self.fail_turn_if_broken(control);
    }

    /// Writes `event` as its wire events.
    fn show(&mut self, event: &SessionEvent, control: &UnboundedSender<TurnControl>) {
        let Some(live) = &mut self.live else {
            return;
        };
        if self.sink.failure().is_some() {
            return;
        }
        for wire in live.map(event) {
            self.sink.send(wire);
        }
        self.fail_turn_if_broken(control);
    }

    fn fail_turn_if_broken(&self, control: &UnboundedSender<TurnControl>) {
        if let Some(message) = self.sink.failure() {
            let _ = control.send(TurnControl::HostFailed {
                message: message.to_owned(),
            });
        }
    }
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
