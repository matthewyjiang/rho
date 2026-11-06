//! How a headless prompt shows: the [`TurnView`] every headless host lends
//! [`SessionRuntime::run_prompt_turn`]. It writes each run event to the
//! JSONL stream and the status file, publishes the run's steering port to a
//! delegated child's parent, and answers the turn's requests. Nobody is
//! there to approve, so an approval or plan that reaches it anyway (an
//! unattended session sends none) is refused. A delegated child whose
//! parent listens forwards its questions there; any other question was
//! declined by the turn already.

use std::{collections::HashMap, future::Future, io, pin::Pin};

use futures_util::{stream::FuturesUnordered, StreamExt};
use rho_sdk::{
    CancellationToken, HostInputId, HostInputRefusal, HostInputRequest, HostInputResponse,
    SessionId,
};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use super::RunReporter;
use crate::{
    app::{
        automation_protocol::{write_event, JsonlAdapter},
        session_runtime::{
            cancelled_approval, PlanReport, SessionEvent, SessionRuntime, TurnControl, TurnIo,
            TurnView,
        },
        subagent_host_input::SubagentHostInputBridge,
        subagent_messaging::SteeringSlot,
    },
    config::Config,
    tools::plan_exit::PlanPermissionAnswer,
};

/// Keeps the status file fresh while a provider or tool call emits no events.
const REPORT_HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(10);

/// Where one headless prompt's events go.
pub(crate) struct HeadlessView<'a> {
    reporter: Option<&'a mut RunReporter>,
    jsonl: Option<&'a mut JsonlAdapter>,
    /// The parent's port for steering a delegated child.
    // Temporary(handle-steering): goes once the parent steers through the
    // session's handle.
    steering: Option<SteeringSlot>,
    /// The parent that answers a delegated child's questions.
    parent: Option<ParentQuestions>,
    /// Why the JSONL stream stopped, once a write to it failed.
    output_failure: Option<String>,
}

impl<'a> HeadlessView<'a> {
    pub(crate) fn new(
        reporter: Option<&'a mut RunReporter>,
        jsonl: Option<&'a mut JsonlAdapter>,
    ) -> Self {
        Self {
            reporter,
            jsonl,
            steering: None,
            parent: None,
            output_failure: None,
        }
    }

    /// Publishes each run's steering port to `slot`.
    // Temporary(handle-steering): goes with `steering`.
    pub(crate) fn with_steering(mut self, slot: Option<SteeringSlot>) -> Self {
        self.steering = slot;
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
        if let Some(adapter) = self.jsonl.as_deref_mut() {
            adapter.set_run_context(runtime.session_id(), runtime.workspace_root());
        }
        if let Some(reporter) = self.reporter.as_deref_mut() {
            reporter.set_mcp_tools(runtime.tools().mcp_report().tool_names());
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
        match event {
            SessionEvent::RunStarted { steering } => {
                if let Some(slot) = &self.steering {
                    slot.publish(steering);
                }
            }
            SessionEvent::Run(event) => self.show_run_event(&event, control),
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
            | SessionEvent::BoundaryApplied { .. }
            | SessionEvent::ApprovalWithdrawn { .. }
            | SessionEvent::PlanApprovalWithdrawn { .. }
            | SessionEvent::Notice { .. }
            | SessionEvent::RunClosed { .. }
            | SessionEvent::RunFinished { .. } => {}
        }
    }

    /// Writes `event` to the status file and the JSONL stream. A failed
    /// JSONL write fails the turn and ends the stream.
    fn show_run_event(
        &mut self,
        event: &rho_sdk::RunEvent,
        control: &UnboundedSender<TurnControl>,
    ) {
        if let Some(reporter) = self.reporter.as_deref_mut() {
            reporter.on_event(event);
        }
        if self.output_failure.is_some() {
            return;
        }
        let Some(wire) = self
            .jsonl
            .as_deref_mut()
            .and_then(|adapter| adapter.event(event))
        else {
            return;
        };
        if let Err(error) = write_event(&mut io::stdout().lock(), &wire) {
            let message = format!("could not write JSONL output: {error}");
            let _ = control.send(TurnControl::HostFailed {
                message: message.clone(),
            });
            self.output_failure = Some(message);
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
