use std::collections::BTreeMap;

use pretty_assertions::assert_eq;
use rho_protocol::{self as wire, ErrorCode, InputAnswer, InputRequestId, Resolution};
use rho_sdk::{ApprovalDecision, HostInputRefusal, HostInputResponse};
use tokio::sync::mpsc;

use super::Inputs;
use crate::{
    app::{
        daemon_host::sessions::test_events::{
            approval, approval_withdrawn, plan, question, question_id, question_withdrawn,
        },
        session_runtime::{ApprovalId, PlanApprovalId, SessionEvent, TurnControl},
    },
    permission::PermissionMode,
    tools::plan_exit::PlanPermissionAnswer,
};

/// Opens the request `event` asks in `inputs`, returning its id and the
/// channel its answer reaches.
fn open(
    inputs: &mut Inputs,
    event: &SessionEvent,
) -> (InputRequestId, mpsc::UnboundedReceiver<TurnControl>) {
    let (control, answers) = mpsc::unbounded_channel();
    let Some(wire::SessionEvent::InputRequested { request }) = inputs.note(&control, event) else {
        panic!("the event opens a request");
    };
    (request.id, answers)
}

fn answers(pairs: &[(&str, &[&str])]) -> InputAnswer {
    InputAnswer::Answers {
        answers: pairs
            .iter()
            .map(|(question, values)| {
                (
                    (*question).to_owned(),
                    values.iter().map(|value| (*value).to_owned()).collect(),
                )
            })
            .collect::<BTreeMap<_, _>>(),
    }
}

/// What the turn got for an answer, or the error code the client did.
#[derive(Debug, PartialEq)]
enum Outcome {
    Turn(TurnControl),
    Refused(ErrorCode),
}

// Covers: each kind of request hands the turn the answer a client gave in
// the runtime's terms (a deny without a reason says the user denied it, a
// plan approval only into a mode it offers, answers only when they fit the
// questions), and refuses an answer meant for another kind, leaving the
// request open.
// Owner: daemon input requests.
#[test]
fn a_request_hands_the_turn_only_the_answers_it_takes() {
    let question_answered = HostInputResponse::new().answer("answer", ["yes"]);
    let cases = [
        (
            approval(0),
            InputAnswer::AllowOnce,
            Outcome::Turn(TurnControl::RespondApproval {
                id: ApprovalId::nth_for_tests(0),
                decision: ApprovalDecision::AllowOnce,
            }),
        ),
        (
            approval(0),
            InputAnswer::Deny { reason: None },
            Outcome::Turn(TurnControl::RespondApproval {
                id: ApprovalId::nth_for_tests(0),
                decision: ApprovalDecision::Deny {
                    reason: "denied by user".into(),
                },
            }),
        ),
        (
            approval(0),
            InputAnswer::KeepPlanning,
            Outcome::Refused(ErrorCode::InvalidParams),
        ),
        (
            plan(0),
            InputAnswer::ApprovePlan {
                mode: wire::PermissionMode::Auto,
            },
            Outcome::Turn(TurnControl::AnswerPlan {
                id: PlanApprovalId::nth_for_tests(0),
                answer: PlanPermissionAnswer::Approve(PermissionMode::Auto),
            }),
        ),
        (
            plan(0),
            InputAnswer::ApprovePlan {
                mode: wire::PermissionMode::Bypass,
            },
            Outcome::Refused(ErrorCode::InvalidParams),
        ),
        (
            plan(0),
            InputAnswer::AllowOnce,
            Outcome::Refused(ErrorCode::InvalidParams),
        ),
        (
            question("q"),
            answers(&[("answer", &["yes"])]),
            Outcome::Turn(TurnControl::AnswerQuestion {
                id: question_id("q"),
                answer: Ok(question_answered),
            }),
        ),
        (
            question("q"),
            InputAnswer::Decline,
            Outcome::Turn(TurnControl::AnswerQuestion {
                id: question_id("q"),
                answer: Err(HostInputRefusal::Declined),
            }),
        ),
        (
            question("q"),
            answers(&[("answer", &["no"])]),
            Outcome::Refused(ErrorCode::InvalidParams),
        ),
        (
            question("q"),
            answers(&[("other", &["yes"])]),
            Outcome::Refused(ErrorCode::InvalidParams),
        ),
        (
            question("q"),
            InputAnswer::AllowOnce,
            Outcome::Refused(ErrorCode::InvalidParams),
        ),
    ];
    for (index, (event, answer, expected)) in cases.into_iter().enumerate() {
        let mut inputs = Inputs::default();
        let (id, mut turn) = open(&mut inputs, &event);
        let refused = matches!(expected, Outcome::Refused(_));

        let outcome = match inputs.answer(id, answer.clone()) {
            Ok(resolved) => {
                assert_eq!(
                    resolved,
                    wire::SessionEvent::InputResolved {
                        id,
                        resolution: Resolution::Answered
                    }
                );
                Outcome::Turn(turn.try_recv().expect("the turn got the answer"))
            }
            Err(error) => Outcome::Refused(error.error_code().expect("a known code")),
        };

        assert_eq!(outcome, expected, "case {index}: answered {answer:?}");
        assert_eq!(
            inputs.waiting(),
            refused,
            "case {index}: a refused answer leaves the request open"
        );
    }
}

// Covers: the first answer takes the request, so a second client's answer,
// or one to a request the turn withdrew, is refused as not open and reaches
// no turn; a withdrawal closes the request it names, and none an answer
// already closed; ids stay unique after requests close.
// Owner: daemon input requests.
#[test]
fn only_the_first_answer_to_an_open_request_wins() {
    let code = |error: rho_protocol::RpcError| error.error_code();
    let mut inputs = Inputs::default();
    let (approved, mut turn) = open(&mut inputs, &approval(0));
    let (asked, _) = open(&mut inputs, &question("q"));
    let (control, _) = mpsc::unbounded_channel();

    let first = inputs.answer(approved, InputAnswer::AllowOnce).map(drop);
    let second = inputs
        .answer(approved, InputAnswer::Deny { reason: None })
        .map(drop)
        .map_err(code);
    let answered_withdrawal = inputs.note(&control, &approval_withdrawn(0));
    let withdrawal = inputs.note(&control, &question_withdrawn("q"));
    let to_withdrawn = inputs
        .answer(asked, InputAnswer::Decline)
        .map(drop)
        .map_err(code);
    let (next, _) = open(&mut inputs, &approval(1));
    let delivered = [turn.try_recv().ok(), turn.try_recv().ok()];

    assert_eq!(
        (
            first.map_err(code),
            second,
            answered_withdrawal,
            withdrawal,
            to_withdrawn,
            next,
            delivered
        ),
        (
            Ok(()),
            Err(Some(ErrorCode::InputNotOpen)),
            None,
            Some(wire::SessionEvent::InputResolved {
                id: asked,
                resolution: Resolution::Withdrawn,
            }),
            Err(Some(ErrorCode::InputNotOpen)),
            InputRequestId(2),
            [
                Some(TurnControl::RespondApproval {
                    id: ApprovalId::nth_for_tests(0),
                    decision: ApprovalDecision::AllowOnce,
                }),
                None,
            ],
        )
    );
}
