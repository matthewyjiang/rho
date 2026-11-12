use std::sync::Arc;

use pretty_assertions::assert_eq;
use rho_sdk::{
    hooks::{
        testing::{recording_dispatcher, RecordingObserver},
        HookInputKind, HookPayload,
    },
    ApprovalDecision, ApprovalRequest, CancellationToken, CapabilityRequest, CapabilitySource,
    HostChoice, HostInputId, HostInputRequest, HostQuestion, PathScope, PendingApproval, RunEvent,
    SelectionMode, SessionId, ToolCallId,
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
        events::{cancelled_approval, ApprovalId, PlanApprovalId, SessionEvent, TurnControl},
        turn::TurnIo,
        QuestionSupport,
    },
    app::subagent_host_input::SubagentHostInputRequest,
    permission::PermissionMode,
    tools::plan_exit::{PlanExitDecision, PlanPermissionRequest},
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
    RunClosed,
    RunFinished,
}

fn seen(event: SessionEvent) -> Seen {
    match event {
        SessionEvent::MessageInserted(_) => {
            unreachable!("the turn inserts the message, not the pump")
        }
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
        SessionEvent::RunClosed => Seen::RunClosed,
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
    /// Where the session's subagents put their questions.
    child_questions: tokio::sync::mpsc::Sender<SubagentHostInputRequest>,
    cancel: CancellationToken,
    /// What the session's hooks were told.
    hooks: Arc<RecordingObserver>,
    pump: JoinHandle<anyhow::Result<()>>,
}

impl Harness {
    fn start(questions: QuestionSupport) -> Self {
        let (run_events, mut run_rx) = unbounded_channel();
        let (approvals, mut approval_rx) = unbounded_channel();
        let (plans, mut plan_rx) = unbounded_channel();
        let (child_questions, mut child_rx) = tokio::sync::mpsc::channel(4);
        let (events_tx, events) = unbounded_channel();
        let (control, control_rx) = unbounded_channel();
        let cancel = CancellationToken::new();
        let pump_cancel = cancel.clone();
        let (dispatcher, hooks) = recording_dispatcher();
        let pump = tokio::spawn(async move {
            let session = SessionId::new();
            let diagnostics = crate::diagnostics::test_diagnostics("test", "test");
            let mut io = TurnIo {
                events: events_tx,
                control: control_rx,
            };
            pump(
                PumpSources {
                    cancel: pump_cancel,
                    questions,
                    events: EventSource::Channel(&mut run_rx),
                    approvals: ApprovalSource::Channel(&mut approval_rx),
                    plan_approvals: Some(&mut plan_rx),
                    displays: None,
                    hooks: dispatcher,
                    session: &session,
                    child_questions: Some(&mut child_rx),
                    diagnostics: &diagnostics,
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
            child_questions,
            cancel,
            hooks,
            pump,
        }
    }

    /// What the hooks were told of requests, once the pump has finished.
    fn asked(&self) -> Vec<(HookInputKind, String)> {
        assert!(
            self.pump.is_finished(),
            "read the hooks after the pump ends"
        );
        self.hooks
            .envelopes()
            .iter()
            .map(|envelope| match envelope.payload() {
                HookPayload::InputRequested(payload) => (payload.kind, payload.summary.clone()),
                other => panic!("unexpected hook payload: {other:?}"),
            })
            .collect()
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

fn plan() -> (PlanPermissionRequest, oneshot::Receiver<PlanExitDecision>) {
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
    assert_eq!(harness.next().await, Seen::RunClosed);
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
        answer: PlanExitDecision::approve(PermissionMode::AllowEdits),
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
            PlanExitDecision::approve(PermissionMode::AllowEdits),
        )
    );

    harness.answer(TurnControl::HostFailed {
        message: "connection closed".into(),
    });
    let error = harness.pump.await.unwrap().unwrap_err();
    assert_eq!(error.to_string(), "connection closed");
}

// Covers: the session's hooks hear of every request put to the host, once
// each and with its kind, and of nothing the pump settles itself: requests
// after a cancel, and questions a host cannot ask.
// Owner: session runtime event pump
#[tokio::test]
async fn hooks_hear_of_each_request_put_to_the_host() {
    let mut harness = Harness::start(QuestionSupport::Available);
    let (waiting, _decision) = approval();
    harness.approvals.send(waiting).unwrap();
    assert_eq!(harness.next().await, Seen::ApprovalRequested(ApprovalId(0)));
    let (plan, _answer) = plan();
    harness.plans.send(plan).unwrap();
    assert_eq!(harness.next().await, Seen::PlanRequested(PlanApprovalId(0)));
    let request = question();
    let question_id = request.id().clone();
    harness.emit(RunEvent::HostInputRequested { request });
    assert_eq!(
        [harness.next().await, harness.next().await],
        [Seen::Run, Seen::QuestionAsked(question_id)]
    );
    harness.cancel.cancel();
    let (late, late_decision) = approval();
    harness.approvals.send(late).unwrap();
    assert_eq!(late_decision.await.unwrap(), cancelled_approval());
    harness.emit(RunEvent::HostInputRequested {
        request: question(),
    });
    harness.end_run();
    (&mut harness.pump).await.unwrap().unwrap();

    let mut unasked = Harness::start(QuestionSupport::Unavailable);
    unasked.emit(RunEvent::HostInputRequested {
        request: question(),
    });
    unasked.end_run();
    (&mut unasked.pump).await.unwrap().unwrap();

    assert_eq!(
        (harness.asked(), unasked.asked()),
        (
            vec![
                (
                    HookInputKind::Approval,
                    "write /workspace/file.rs (edit file)".into()
                ),
                (HookInputKind::PlanApproval, "1. edit".into()),
                (HookInputKind::Question, "Confirm".into()),
            ],
            Vec::new(),
        )
    );
}

/// A question a subagent asks, and where its answer arrives.
fn child_question() -> (
    SubagentHostInputRequest,
    HostInputId,
    oneshot::Receiver<Result<rho_sdk::HostInputResponse, rho_sdk::Error>>,
) {
    let request = question();
    let id = request.id().clone();
    let (response, answered) = oneshot::channel();
    let asked = SubagentHostInputRequest {
        run_id: "ab12cd".into(),
        agent_id: "worker".into(),
        request,
        response,
    };
    (asked, id, answered)
}

// Covers: a subagent's question reaches the host as a question of the
// turn, named by a notice, and the host's answer reaches the subagent, not
// the parent's run; one whose subagent stops waiting is withdrawn, and one
// still open when the turn is cancelled is withdrawn
// and its subagent told nobody will answer, as is one asked afterwards.
// Owner: session runtime turn pump.
#[tokio::test]
async fn a_subagents_question_is_answered_through_the_turn() {
    let mut harness = Harness::start(QuestionSupport::Available);
    let (asked, answered_id, answered) = child_question();
    harness.child_questions.send(asked).await.unwrap();
    let first = [harness.next().await, harness.next().await];
    harness.answer(TurnControl::AnswerQuestion {
        id: answered_id.clone(),
        answer: Ok(rho_sdk::HostInputResponse::new().answer("go", ["yes"])),
    });
    let answer = answered.await.expect("the answer reaches the subagent");
    let answer_told = harness.next().await;

    let (asked, abandoned_id, abandoned) = child_question();
    harness.child_questions.send(asked).await.unwrap();
    let abandoned_ask = [harness.next().await, harness.next().await];
    drop(abandoned);
    let given_up = harness.next().await;

    let (asked, open_id, open) = child_question();
    harness.child_questions.send(asked).await.unwrap();
    let second = [harness.next().await, harness.next().await];
    harness.cancel.cancel();
    let withdrawn = harness.next().await;
    let (late, _, late_reply) = child_question();
    harness.child_questions.send(late).await.unwrap();

    assert_eq!(
        (
            first,
            answer
                .ok()
                .and_then(|answer| answer.answers().get("go").cloned()),
            answer_told,
            abandoned_ask,
            given_up,
            second,
            withdrawn,
            open.await.is_err(),
            late_reply.await.is_err(),
        ),
        (
            [Seen::Notice, Seen::QuestionAsked(answered_id)],
            Some(vec!["yes".to_owned()]),
            Seen::Notice,
            [Seen::Notice, Seen::QuestionAsked(abandoned_id.clone())],
            Seen::QuestionWithdrawn(abandoned_id),
            [Seen::Notice, Seen::QuestionAsked(open_id.clone())],
            Seen::QuestionWithdrawn(open_id),
            true,
            true,
        )
    );
}
