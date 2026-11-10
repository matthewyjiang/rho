use std::{
    collections::BTreeMap,
    time::{Duration, UNIX_EPOCH},
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
    tools::plan_exit::PlanExitDecision,
};

/// Opens the request `event` asks in `inputs`, returning its id and the
/// channel its answer reaches.
fn open(
    inputs: &mut Inputs,
    event: &SessionEvent,
) -> (InputRequestId, mpsc::UnboundedReceiver<TurnControl>) {
    let (control, answers) = mpsc::unbounded_channel();
    inputs.note(&control, event, UNIX_EPOCH);
    (opened(inputs).id, answers)
}

/// The request the last change opened, its only one.
fn opened(inputs: &mut Inputs) -> wire::InputRequest {
    match inputs.take_events().as_slice() {
        [wire::SessionEvent::InputRequested { request }] => (**request).clone(),
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
            InputAnswer::KeepPlanning { feedback: None },
            Outcome::Refused(ErrorCode::InvalidParams),
        ),
        (
            plan(0),
            InputAnswer::ApprovePlan {
                mode: wire::PermissionMode::Auto,
                feedback: None,
            },
            Outcome::Turn(TurnControl::AnswerPlan {
                id: PlanApprovalId::nth_for_tests(0),
                answer: PlanExitDecision::approve(PermissionMode::Auto),
            }),
        ),
        (
            plan(0),
            InputAnswer::KeepPlanning {
                feedback: Some("smaller steps".into()),
            },
            Outcome::Turn(TurnControl::AnswerPlan {
                id: PlanApprovalId::nth_for_tests(0),
                answer: PlanExitDecision::keep_planning()
                    .with_feedback(Some("smaller steps".into())),
            }),
        ),
        (
            plan(0),
            InputAnswer::ApprovePlan {
                mode: wire::PermissionMode::Bypass,
                feedback: None,
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
        let mut inputs = Inputs::new(/*question_timeout*/ None);
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
    let mut inputs = Inputs::new(/*question_timeout*/ None);
    let (approved, mut turn) = open(&mut inputs, &approval(0));
    let (asked, _asking) = open(&mut inputs, &question("q"));
    let (abandoned, ended) = open(&mut inputs, &approval(1));
    drop(ended);
    // The model turn that asked `asked` withdraws it; another model turn's
    // withdrawal of a request with the same runtime id closes nothing.
    let (other_turn, _) = mpsc::unbounded_channel();
    let (asking_turn, _) = mpsc::unbounded_channel();
    inputs.note(&asking_turn, &question("q"), UNIX_EPOCH);
    let reasked = opened(&mut inputs);

    let first = inputs
        .answer(approved, InputAnswer::AllowOnce)
        .map_err(code);
    let second = inputs
        .answer(approved, InputAnswer::Deny { reason: None })
        .map_err(code);
    inputs.note(&other_turn, &approval_withdrawn(0), UNIX_EPOCH);
    inputs.note(&other_turn, &question_withdrawn("q"), UNIX_EPOCH);
    inputs.note(&asking_turn, &question_withdrawn("q"), UNIX_EPOCH);
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

/// A session's question timeout in these tests; every request opens at
/// `UNIX_EPOCH`, so one that times out has its deadline this far past it.
const QUESTION_TIMEOUT: Duration = Duration::from_secs(60);

// Covers: only a question whose agent offered fallback answers, in a
// session whose questions time out, gets a timeout: clients see when and
// with what the daemon answers it, and why, and the caller is handed the
// timeout to run.
// Owner: daemon input requests.
#[test]
fn only_a_question_with_fallback_answers_times_out() {
    let times_out = (
        Some(wire::InputTimeout {
            deadline_ms: 60_000,
            answer: answers(&[("answer", &["yes"])]),
            reason: "safe default".into(),
        }),
        vec![(InputRequestId(0), QUESTION_TIMEOUT)],
    );
    let cases = [
        (
            "question with fallback answers",
            Some(QUESTION_TIMEOUT),
            question_with_fallback("q", "safe default"),
            times_out,
        ),
        (
            "question without",
            Some(QUESTION_TIMEOUT),
            question("q"),
            (None, Vec::new()),
        ),
        (
            "approval",
            Some(QUESTION_TIMEOUT),
            approval(0),
            (None, Vec::new()),
        ),
        (
            "questions do not time out",
            None,
            question_with_fallback("q", "safe default"),
            (None, Vec::new()),
        ),
    ];
    for (case, question_timeout, event, expected) in cases {
        let mut inputs = Inputs::new(question_timeout);
        let (control, _turn) = mpsc::unbounded_channel();
        inputs.note(&control, &event, UNIX_EPOCH);
        let request = opened(&mut inputs);

        assert_eq!(
            (request.timeout, inputs.take_timeouts()),
            expected,
            "{case}"
        );
    }
}

/// Something that happens to an open question with a timeout.
#[derive(Clone, Copy, Debug)]
enum Step {
    Answer,
    Hold,
    EndTurn,
    TimeOut,
}

/// What came of a question with a timeout after some steps.
#[derive(Debug, PartialEq)]
struct Played {
    /// The codes of the steps refused.
    refused: Vec<ErrorCode>,
    events: Vec<wire::SessionEvent>,
    turn: Option<TurnControl>,
    open: bool,
}

/// Opens the question `event` asks in a session whose questions time out,
/// plays `steps` on it, and reports what came of it.
fn play(event: &SessionEvent, steps: &[Step]) -> Played {
    let mut inputs = Inputs::new(Some(QUESTION_TIMEOUT));
    let (id, mut turn) = open(&mut inputs, event);
    let mut refused = Vec::new();
    for step in steps {
        let result = match step {
            Step::Answer => inputs.answer(id, InputAnswer::Decline),
            Step::Hold => inputs.hold(id),
            Step::EndTurn => {
                turn.close();
                Ok(())
            }
            Step::TimeOut => {
                inputs.time_out(id);
                Ok(())
            }
        };
        refused.extend(result.err().and_then(|error| error.error_code()));
    }
    inputs.send_answers();
    Played {
        refused,
        events: inputs.take_events(),
        turn: turn.try_recv().ok(),
        open: inputs.waiting(),
    }
}

// Covers: a question still open and not held when its timeout fires hands
// the turn its fallback answers, marked as the fallback, and tells clients
// it timed out; one answered or held first, or whose model turn ended,
// does not time out.
// Owner: daemon input requests.
#[test]
fn a_question_times_out_only_while_open_and_not_held() {
    let id = InputRequestId(0);
    let cases = [
        (
            "nobody answered",
            &[Step::TimeOut][..],
            vec![resolved(id, Resolution::TimedOut)],
            Some(TurnControl::AnswerQuestion {
                id: question_id("q"),
                answer: Ok(HostInputResponse::new()
                    .answer("answer", ["yes"])
                    .with_source(HostInputSource::TimeoutFallback)),
            }),
            false,
        ),
        (
            "answered first",
            &[Step::Answer, Step::TimeOut],
            vec![resolved(id, Resolution::Answered)],
            Some(TurnControl::AnswerQuestion {
                id: question_id("q"),
                answer: Err(HostInputRefusal::Declined),
            }),
            false,
        ),
        (
            "held first",
            &[Step::Hold, Step::TimeOut],
            vec![wire::SessionEvent::InputHeld { id }],
            None,
            true,
        ),
        (
            "model turn ended",
            &[Step::EndTurn, Step::TimeOut],
            vec![resolved(id, Resolution::Withdrawn)],
            None,
            false,
        ),
    ];
    for (case, steps, events, turn, open) in cases {
        let expected = Played {
            refused: Vec::new(),
            events,
            turn,
            open,
        };
        let timed = question_with_fallback("q", "safe");
        assert_eq!(play(&timed, steps), expected, "{case}");
    }
}

// Covers: holding a question stops its timeout once and tells clients;
// holding it again, or one without a timeout, tells nobody; a closed
// question cannot be held.
// Owner: daemon input requests.
#[test]
fn holding_tells_clients_only_when_it_stops_a_timeout() {
    let id = InputRequestId(0);
    let held = wire::SessionEvent::InputHeld { id };
    let timed = question_with_fallback("q", "safe");
    let cases = [
        (
            "held",
            &timed,
            &[Step::Hold][..],
            Vec::new(),
            vec![held.clone()],
        ),
        (
            "held twice",
            &timed,
            &[Step::Hold, Step::Hold],
            Vec::new(),
            vec![held],
        ),
        (
            "without a timeout",
            &question("q"),
            &[Step::Hold],
            Vec::new(),
            Vec::new(),
        ),
        (
            "held after it timed out",
            &timed,
            &[Step::TimeOut, Step::Hold],
            vec![ErrorCode::InputNotOpen],
            vec![resolved(id, Resolution::TimedOut)],
        ),
    ];
    for (case, event, steps, refused, events) in cases {
        let played = play(event, steps);
        assert_eq!((played.refused, played.events), (refused, events), "{case}");
    }
}
