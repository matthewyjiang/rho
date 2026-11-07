use std::{
    collections::BTreeMap,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use pretty_assertions::assert_eq;
use rho_protocol::{self as wire, ErrorCode, InputAnswer, InputRequestId, Resolution};
use rho_sdk::{ApprovalDecision, HostInputRefusal, HostInputResponse, HostInputSource};
use tokio::sync::mpsc;

use super::Inputs;
use crate::{
    app::{
        daemon_host::sessions::test_events::{
            approval, approval_withdrawn, plan, question, question_id, question_with_fallback,
            question_withdrawn,
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
    inputs.note(&control, event);
    (opened(inputs).id, answers)
}

/// The request the last change opened, its only one.
fn opened(inputs: &mut Inputs) -> wire::InputRequest {
    match inputs.take_events().as_slice() {
        [wire::SessionEvent::InputRequested { request }] => request.clone(),
        _ => panic!("the event opens a request"),
    }
}

fn resolved(id: InputRequestId, resolution: Resolution) -> wire::SessionEvent {
    wire::SessionEvent::InputResolved { id, resolution }
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
        let mut inputs = Inputs::new(/*questions_time_out*/ None);
        let (id, mut turn) = open(&mut inputs, &event);
        let refused = matches!(expected, Outcome::Refused(_));

        let outcome = match inputs.answer(id, answer.clone()) {
            Ok(()) => {
                inputs.send_answers();
                Outcome::Turn(turn.try_recv().expect("the turn got the answer"))
            }
            Err(error) => Outcome::Refused(error.error_code().expect("a known code")),
        };

        assert_eq!(outcome, expected, "case {index}: answered {answer:?}");
        // A refused answer leaves the request open; an accepted one tells
        // clients it was answered.
        let told = if refused {
            Vec::new()
        } else {
            vec![resolved(id, Resolution::Answered)]
        };
        assert_eq!(
            (inputs.waiting(), inputs.take_events()),
            (refused, told),
            "case {index}"
        );
    }
}

// Covers: the first answer takes the request, so a second client's answer,
// or one to a request the turn withdrew, is refused as not open and reaches
// no turn; the answer reaches its turn only once it is sent, after clients
// were told; a withdrawal closes only the request of the model turn that
// asked, and none an answer already closed; an answer to a model turn that
// ended without withdrawing its request is refused and tells clients the
// request was withdrawn; ids stay unique after requests close.
// Owner: daemon input requests.
#[test]
fn only_the_first_answer_to_an_open_request_wins() {
    let code = |error: rho_protocol::RpcError| error.error_code();
    let mut inputs = Inputs::new(/*questions_time_out*/ None);
    let (approved, mut turn) = open(&mut inputs, &approval(0));
    let (asked, _asking) = open(&mut inputs, &question("q"));
    let (abandoned, ended) = open(&mut inputs, &approval(1));
    drop(ended);
    // The model turn that asked `asked` withdraws it; another model turn's
    // withdrawal of a request with the same runtime id closes nothing.
    let (other_turn, _) = mpsc::unbounded_channel();
    let (asking_turn, _) = mpsc::unbounded_channel();
    inputs.note(&asking_turn, &question("q"));
    let reasked = opened(&mut inputs);

    let first = inputs
        .answer(approved, InputAnswer::AllowOnce)
        .map_err(code);
    let second = inputs
        .answer(approved, InputAnswer::Deny { reason: None })
        .map_err(code);
    inputs.note(&other_turn, &approval_withdrawn(0));
    inputs.note(&other_turn, &question_withdrawn("q"));
    inputs.note(&asking_turn, &question_withdrawn("q"));
    let to_withdrawn = inputs
        .answer(reasked.id, InputAnswer::Decline)
        .map_err(code);
    let to_ended = inputs
        .answer(abandoned, InputAnswer::AllowOnce)
        .map_err(code);
    let before_sending = turn.try_recv().ok();
    inputs.send_answers();
    let delivered = [before_sending, turn.try_recv().ok(), turn.try_recv().ok()];

    assert_eq!(
        (
            first,
            second,
            to_withdrawn,
            to_ended,
            inputs.take_events(),
            delivered,
            inputs.waiting(),
            [approved, asked, abandoned, reasked.id],
        ),
        (
            Ok(()),
            Err(Some(ErrorCode::InputNotOpen)),
            Err(Some(ErrorCode::InputNotOpen)),
            Err(Some(ErrorCode::InputNotOpen)),
            vec![
                resolved(approved, Resolution::Answered),
                resolved(reasked.id, Resolution::Withdrawn),
                resolved(abandoned, Resolution::Withdrawn),
            ],
            [
                None,
                Some(TurnControl::RespondApproval {
                    id: ApprovalId::nth_for_tests(0),
                    decision: ApprovalDecision::AllowOnce,
                }),
                None,
            ],
            // `asked`, whose model turn withdrew nothing, is still open.
            true,
            [
                InputRequestId(0),
                InputRequestId(1),
                InputRequestId(2),
                InputRequestId(3)
            ],
        )
    );
}

const TIME_OUT: Duration = Duration::from_secs(60);

fn unix_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

// Covers: only a question whose agent offered fallback answers, in a
// session whose questions time out, gets a timeout: clients see its
// deadline, answer, and reason, and the caller is handed it to run.
// Owner: daemon input requests.
#[test]
fn only_a_question_with_fallback_answers_times_out() {
    let cases = [
        (
            Some(TIME_OUT),
            question_with_fallback("q", "safe default"),
            true,
        ),
        (Some(TIME_OUT), question("q"), false),
        (Some(TIME_OUT), approval(0), false),
        (None, question_with_fallback("q", "safe default"), false),
    ];
    for (index, (time_out, event, times_out)) in cases.into_iter().enumerate() {
        let mut inputs = Inputs::new(time_out);
        let before = unix_ms();
        let (control, _turn) = mpsc::unbounded_channel();
        inputs.note(&control, &event);
        let request = opened(&mut inputs);
        let after = unix_ms();

        let timeout = request.timeout.map(|timeout| {
            let window = before + 60_000..=after + 60_000;
            assert!(
                window.contains(&timeout.deadline),
                "case {index}: deadline {} outside {window:?}",
                timeout.deadline
            );
            (timeout.answer, timeout.reason)
        });
        assert_eq!(
            (timeout, inputs.take_timeouts()),
            if times_out {
                (
                    Some((answers(&[("answer", &["yes"])]), "safe default".to_owned())),
                    vec![(request.id, TIME_OUT)],
                )
            } else {
                (None, Vec::new())
            },
            "case {index}"
        );
    }
}

// Covers: a timed-out question hands the turn its fallback answers, marked
// as the fallback, and tells clients it timed out; one a client held,
// answered, or whose turn ended first does not time out, and holding a
// closed request is refused as not open.
// Owner: daemon input requests.
#[test]
fn a_question_times_out_only_while_open_and_not_held() {
    let fallback = || TurnControl::AnswerQuestion {
        id: question_id("q"),
        answer: Ok(HostInputResponse::new()
            .answer("answer", ["yes"])
            .with_source(HostInputSource::TimeoutFallback)),
    };
    let mut inputs = Inputs::new(Some(TIME_OUT));
    let (timed_out, mut turn) = open(&mut inputs, &question_with_fallback("q", "safe"));
    let (held, mut held_turn) = open(&mut inputs, &question_with_fallback("q", "safe"));
    let (answered, _answered_turn) = open(&mut inputs, &question_with_fallback("q", "safe"));
    let (ended, ended_turn) = open(&mut inputs, &question_with_fallback("q", "safe"));
    drop(ended_turn);
    inputs.take_timeouts();

    inputs.time_out(timed_out);
    inputs.send_answers();
    let timed_out_events = inputs.take_events();
    let held_twice = (inputs.hold(held), inputs.hold(held));
    let hold_events = inputs.take_events();
    inputs.time_out(held);
    inputs.answer(answered, InputAnswer::Decline).unwrap();
    inputs.take_events();
    inputs.time_out(answered);
    inputs.time_out(ended);
    inputs.send_answers();
    let later_events = inputs.take_events();
    let hold_closed = inputs.hold(timed_out).map_err(|error| error.error_code());

    assert_eq!(
        (
            timed_out_events,
            turn.try_recv().ok(),
            held_twice,
            hold_events,
            later_events,
            held_turn.try_recv().ok(),
            inputs.waiting(),
            hold_closed,
        ),
        (
            vec![resolved(timed_out, Resolution::TimedOut)],
            Some(fallback()),
            (Ok(()), Ok(())),
            vec![wire::SessionEvent::InputHeld { id: held }],
            vec![resolved(ended, Resolution::Withdrawn)],
            None,
            true,
            Err(Some(ErrorCode::InputNotOpen)),
        )
    );
}
