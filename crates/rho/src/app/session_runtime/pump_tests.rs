use pretty_assertions::assert_eq;
use rho_sdk::{
    ApprovalDecision, ApprovalRequest, CancellationToken, CapabilityRequest, CapabilitySource,
    HostChoice, HostInputId, HostInputRequest, HostQuestion, PathScope, PendingApproval, RunEvent,
    SelectionMode, ToolCallId,
};
use tokio::{
    sync::{
        mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
        oneshot,
    },
    task::JoinHandle,
};

use super::{pump, ApprovalSource, EventSource, PumpSources};
use crate::{
    app::session_runtime::{
        events::{
            cancelled_approval, ApprovalId, PlanApprovalId, RunEnd, SessionEvent, TurnControl,
        },
        turn::TurnIo,
        QuestionSupport,
    },
    permission::PermissionMode,
    tools::plan_exit::{PlanPermissionAnswer, PlanPermissionRequest},
};

/// What a host sees of one event, comparable as a whole.
#[derive(Debug, PartialEq)]
enum Seen {
    RunStarted,
    Run,
    BoundaryApplied,
    ApprovalRequested(ApprovalId),
    ApprovalWithdrawn(ApprovalId),
    PlanRequested(PlanApprovalId),
    PlanWithdrawn(PlanApprovalId),
    QuestionAsked(HostInputId),
    QuestionWithdrawn(HostInputId),
    Notice,
    RunClosed(RunEnd),
    RunFinished,
}

fn seen(event: SessionEvent) -> Seen {
    match event {
        SessionEvent::RunStarted => Seen::RunStarted,
        SessionEvent::Run(_) => Seen::Run,
        SessionEvent::BoundaryApplied { .. } => Seen::BoundaryApplied,
        SessionEvent::ApprovalRequested { id, .. } => Seen::ApprovalRequested(id),
        SessionEvent::ApprovalWithdrawn { id } => Seen::ApprovalWithdrawn(id),
        SessionEvent::PlanApprovalRequested { id, .. } => Seen::PlanRequested(id),
        SessionEvent::PlanApprovalWithdrawn { id } => Seen::PlanWithdrawn(id),
        SessionEvent::QuestionAsked { id, .. } => Seen::QuestionAsked(id),
        SessionEvent::QuestionWithdrawn { id } => Seen::QuestionWithdrawn(id),
        SessionEvent::Notice { .. } => Seen::Notice,
        SessionEvent::RunClosed { ending } => Seen::RunClosed(ending),
        SessionEvent::RunFinished { .. } => Seen::RunFinished,
    }
}

/// A pump on test channels, run as its own task; the test plays both the
/// run and the host.
struct Harness {
    events: UnboundedReceiver<SessionEvent>,
    control: UnboundedSender<TurnControl>,
    /// `None` once the run's event stream ended.
    run_events: Option<UnboundedSender<RunEvent>>,
    approvals: UnboundedSender<PendingApproval>,
    plans: UnboundedSender<PlanPermissionRequest>,
    cancel: CancellationToken,
    pump: JoinHandle<anyhow::Result<()>>,
}

impl Harness {
    fn start(questions: QuestionSupport) -> Self {
        let (run_events, mut run_rx) = unbounded_channel();
        let (approvals, mut approval_rx) = unbounded_channel();
        let (plans, mut plan_rx) = unbounded_channel();
        let (events_tx, events) = unbounded_channel();
        let (control, control_rx) = unbounded_channel();
        let cancel = CancellationToken::new();
        let pump_cancel = cancel.clone();
        let pump = tokio::spawn(async move {
            let mut io = TurnIo {
                events: events_tx,
                control: control_rx,
                questions,
            };
            pump(
                PumpSources {
                    cancel: pump_cancel,
                    events: EventSource::Channel(&mut run_rx),
                    approvals: ApprovalSource::Channel(&mut approval_rx),
                    plan_approvals: Some(&mut plan_rx),
                    displays: None,
                },
                &mut io,
            )
            .await
        });
        Self {
            events,
            control,
            run_events: Some(run_events),
            approvals,
            plans,
            cancel,
            pump,
        }
    }

    async fn next(&mut self) -> Seen {
        seen(self.events.recv().await.expect("the pump reports an event"))
    }

    fn emit(&self, event: RunEvent) {
        let events = self.run_events.as_ref().expect("the run is still going");
        events.send(event).expect("the pump reads run events");
    }

    fn end_run(&mut self) {
        self.run_events = None;
    }

    fn answer(&self, control: TurnControl) {
        self.control.send(control).expect("the pump reads answers");
    }
}

fn approval() -> (PendingApproval, oneshot::Receiver<ApprovalDecision>) {
    PendingApproval::new(ApprovalRequest::new(
        CapabilityRequest::write_path(
            "/workspace/file.rs",
            PathScope::PrimaryWorkspace,
            CapabilitySource::built_in_tool("write"),
        ),
        "edit file",
    ))
}

fn plan() -> (
    PlanPermissionRequest,
    oneshot::Receiver<PlanPermissionAnswer>,
) {
    let (reply, answer) = oneshot::channel();
    (
        PlanPermissionRequest {
            call_id: ToolCallId::new(),
            plan: "1. edit".into(),
            targets: vec![PermissionMode::AllowEdits],
            reply,
        },
        answer,
    )
}

fn question() -> HostInputRequest {
    let question = HostQuestion::new(
        "go",
        "Continue?",
        vec![HostChoice::new("yes", "Yes")],
        SelectionMode::One,
    )
    .unwrap();
    HostInputRequest::questionnaire("Confirm", vec![question]).unwrap()
}

// Covers: a cancelled turn must answer every approval still waiting on the
// host as cancelled, drop plan approvals so their calls report cancelled,
// withdraw open questions, and never put a later approval to the host. A
// pending approval must not hold back the run's events either.
// Owner: session runtime event pump
#[tokio::test]
async fn cancel_settles_every_request_waiting_on_the_host() {
    let mut harness = Harness::start(QuestionSupport::Available);
    let (waiting, waiting_decision) = approval();
    harness.approvals.send(waiting).unwrap();
    assert_eq!(harness.next().await, Seen::ApprovalRequested(ApprovalId(0)));
    let (plan, plan_answer) = plan();
    harness.plans.send(plan).unwrap();
    assert_eq!(harness.next().await, Seen::PlanRequested(PlanApprovalId(0)));
    let request = question();
    let question_id = request.id().clone();
    harness.emit(RunEvent::HostInputRequested { request });
    assert_eq!(
        [harness.next().await, harness.next().await],
        [Seen::Run, Seen::QuestionAsked(question_id.clone())]
    );

    harness.cancel.cancel();
    assert_eq!(
        [
            harness.next().await,
            harness.next().await,
            harness.next().await
        ],
        [
            Seen::ApprovalWithdrawn(ApprovalId(0)),
            Seen::PlanWithdrawn(PlanApprovalId(0)),
            Seen::QuestionWithdrawn(question_id),
        ]
    );
    assert_eq!(waiting_decision.await.unwrap(), cancelled_approval());
    assert!(
        plan_answer.await.is_err(),
        "a cancelled plan approval must drop its reply"
    );
    let (late, late_decision) = approval();
    harness.approvals.send(late).unwrap();
    assert_eq!(late_decision.await.unwrap(), cancelled_approval());

    harness.end_run();
    assert_eq!(harness.next().await, Seen::RunClosed(RunEnd::WithoutResult));
    harness.pump.await.unwrap().unwrap();
    assert!(
        harness.events.recv().await.is_none(),
        "the late approval must not reach the host"
    );
}

// Covers: host answers must reach the request they name, in any order, with
// unknown ids ignored; a plan approval whose tool stopped waiting is
// withdrawn; and a host that could not deliver fails the turn.
// Owner: session runtime event pump
#[tokio::test]
async fn answers_reach_the_request_they_name() {
    let mut harness = Harness::start(QuestionSupport::Available);
    let (first, first_decision) = approval();
    let (second, second_decision) = approval();
    harness.approvals.send(first).unwrap();
    harness.approvals.send(second).unwrap();
    let (kept, kept_answer) = plan();
    let (abandoned, abandoned_answer) = plan();
    harness.plans.send(kept).unwrap();
    harness.plans.send(abandoned).unwrap();
    let mut requested = Vec::new();
    for _ in 0..4 {
        requested.push(harness.next().await);
    }
    assert_eq!(
        requested,
        [
            Seen::ApprovalRequested(ApprovalId(0)),
            Seen::ApprovalRequested(ApprovalId(1)),
            Seen::PlanRequested(PlanApprovalId(0)),
            Seen::PlanRequested(PlanApprovalId(1)),
        ]
    );

    harness.answer(TurnControl::RespondApproval {
        id: ApprovalId(7),
        decision: ApprovalDecision::AllowOnce,
    });
    harness.answer(TurnControl::RespondApproval {
        id: ApprovalId(1),
        decision: ApprovalDecision::AllowForSession,
    });
    harness.answer(TurnControl::RespondApproval {
        id: ApprovalId(0),
        decision: ApprovalDecision::Deny {
            reason: "no".into(),
        },
    });
    harness.answer(TurnControl::AnswerPlan {
        id: PlanApprovalId(0),
        answer: PlanPermissionAnswer::Approve(PermissionMode::AllowEdits),
    });
    drop(abandoned_answer);
    assert_eq!(harness.next().await, Seen::PlanWithdrawn(PlanApprovalId(1)));
    assert_eq!(
        (
            first_decision.await.unwrap(),
            second_decision.await.unwrap(),
            kept_answer.await.unwrap(),
        ),
        (
            ApprovalDecision::Deny {
                reason: "no".into()
            },
            ApprovalDecision::AllowForSession,
            PlanPermissionAnswer::Approve(PermissionMode::AllowEdits),
        )
    );

    harness.answer(TurnControl::HostFailed {
        message: "connection closed".into(),
    });
    let error = harness.pump.await.unwrap().unwrap_err();
    assert_eq!(error.to_string(), "connection closed");
}
