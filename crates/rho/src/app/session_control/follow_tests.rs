use pretty_assertions::assert_eq;
use rho_protocol::{AnswerStop, Ask, InputRequestId, Question, QuestionAsk, Resolution, Selection};

use super::*;

fn watch(state: JobState) -> Watch {
    Watch {
        stream: StreamId::new("stream"),
        seq: Seq(0),
        state,
        outcome: None,
        outcome_checkpoint: None,
        inputs: Vec::new(),
    }
}

fn answered(text: &str) -> TurnOutcome {
    TurnOutcome::Answered {
        text: text.into(),
        stop: AnswerStop::EndTurn,
    }
}

// Covers: a reset's replay of the last turn's end does not pass for the
// running turn's, which `wait` reports once that turn ends, with a cursor at
// the last event seen.
// Owner: session-control wait.
#[test]
fn only_the_turn_that_ended_reports_an_outcome() {
    let mut watching = watch(JobState::Running);
    // The replay: the last turn's end, then the new turn starting.
    watching.push(
        Seq(0),
        &SessionEvent::TurnEnded {
            outcome: answered("old"),
        },
    );
    watching.push(
        Seq(7),
        &SessionEvent::State {
            state: JobState::Running,
        },
    );
    let mut ended = watch(JobState::Running);
    ended.seq = watching.seq;
    ended.push(
        Seq(8),
        &SessionEvent::TurnEnded {
            outcome: answered("new"),
        },
    );
    ended.push(
        Seq(9),
        &SessionEvent::State {
            state: JobState::Completed,
        },
    );

    assert_eq!(
        (
            watching.finish(SessionId::new("s")),
            ended.finish(SessionId::new("s"))
        ),
        (
            WaitResult {
                session: SessionId::new("s"),
                state: JobState::Running,
                done: false,
                outcome: None,
                inputs: Vec::new(),
                cursor: Some("stream:7".into()),
            },
            WaitResult {
                session: SessionId::new("s"),
                state: JobState::Completed,
                done: true,
                outcome: Some(answered("new")),
                inputs: Vec::new(),
                cursor: Some("stream:9".into()),
            },
        )
    );
}

// Covers: a wait ending between TurnEnded and State must not acknowledge
// an outcome it has not returned; the next wait must replay that outcome.
// Owner: session-control wait checkpoint.
#[test]
fn a_wait_boundary_does_not_skip_an_unreturned_outcome() {
    let mut watching = watch(JobState::Running);
    watching.seq = Seq(7);
    watching.push(
        Seq(8),
        &SessionEvent::TurnEnded {
            outcome: answered("final answer"),
        },
    );
    assert_eq!(
        watching.finish(SessionId::new("s")),
        WaitResult {
            session: SessionId::new("s"),
            state: JobState::Running,
            done: false,
            outcome: None,
            inputs: Vec::new(),
            cursor: Some("stream:7".into()),
        }
    );
}

// Covers: a turn waiting for input stops `wait` with the requests still
// open, and one a person answered drops out.
// Owner: session-control wait.
#[test]
fn a_turn_waiting_for_input_reports_its_open_requests() {
    let request = |id| InputRequest {
        id: InputRequestId(id),
        ask: Ask::Question(QuestionAsk {
            title: "Color".into(),
            questions: vec![Question {
                id: "color".into(),
                prompt: "Which color?".into(),
                header: None,
                choices: Vec::new(),
                selection: Selection::One,
                allow_other: true,
                help: None,
                default: None,
                required: true,
            }],
        }),
        tool_call: None,
        timeout: None,
    };
    let mut watching = watch(JobState::Running);
    for event in [
        SessionEvent::InputRequested {
            request: Box::new(request(1)),
        },
        SessionEvent::InputRequested {
            request: Box::new(request(2)),
        },
        SessionEvent::InputResolved {
            id: InputRequestId(1),
            resolution: Resolution::Answered,
        },
        SessionEvent::State {
            state: JobState::WaitingForInput,
        },
    ] {
        watching.push(Seq(1), &event);
    }

    let waited = watching.finish(SessionId::new("s"));
    assert_eq!(
        (waited.state, waited.done, waited.inputs),
        (JobState::WaitingForInput, true, vec![request(2)])
    );
}
