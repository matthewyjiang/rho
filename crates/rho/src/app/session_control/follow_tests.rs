use pretty_assertions::assert_eq;
use rho_protocol::{AnswerStop, Ask, InputRequestId, Question, QuestionAsk, Resolution, Selection};

use super::*;

/// A watch of a job in `state`, of turn `turn`.
fn watch(state: JobState, turn: &str) -> Watch {
    Watch {
        stream: StreamId::new("stream"),
        seq: Seq(0),
        state,
        turn: Some(TurnId::new(turn)),
        ended: None,
        inputs: Vec::new(),
    }
}

fn turn_ended(turn: &str, outcome: TurnOutcome) -> SessionEvent {
    SessionEvent::TurnEnded {
        turn: TurnId::new(turn),
        outcome,
    }
}

fn state(state: JobState, turn: &str) -> SessionEvent {
    SessionEvent::State {
        state,
        turn: Some(TurnId::new(turn)),
    }
}

fn answered(text: &str) -> TurnOutcome {
    TurnOutcome::Answered {
        text: text.into(),
        stop: AnswerStop::EndTurn,
    }
}

// Covers: a turn's outcome is reported by the turn's id: a reset's replay
// of the turn before's end does not pass for the running turn's, which
// `wait` reports once that turn ends, with a cursor at the last event seen.
// Owner: session-control wait.
#[test]
fn only_the_turn_that_ended_reports_an_outcome() {
    let mut watching = watch(JobState::Running, "t2");
    // The replay: the last turn's end, then the new turn starting.
    watching.push(Seq(0), &turn_ended("t1", answered("old")));
    watching.push(Seq(7), &state(JobState::Running, "t2"));
    watching.finish_replay(JobState::Running, Some(TurnId::new("t2")));
    let mut ended = watch(JobState::Running, "t2");
    ended.seq = watching.seq;
    ended.push(Seq(8), &turn_ended("t2", answered("new")));
    ended.push(Seq(9), &state(JobState::Completed, "t2"));

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

// Covers: a wait that ends between the running turn's end and its terminal
// state does not acknowledge an outcome it has not returned; the end of
// another turn, as a reset replays, holds the cursor back for nothing.
// Owner: session-control wait checkpoint.
#[test]
fn a_wait_boundary_does_not_skip_an_unreturned_outcome() {
    for (ended, checkpoint) in [("t2", "stream:7"), ("t1", "stream:8")] {
        let mut watching = watch(JobState::Running, "t2");
        watching.seq = Seq(7);
        watching.push(Seq(8), &turn_ended(ended, answered("final answer")));
        assert_eq!(
            watching.finish(SessionId::new("s")),
            WaitResult {
                session: SessionId::new("s"),
                state: JobState::Running,
                done: false,
                outcome: None,
                inputs: Vec::new(),
                cursor: Some(checkpoint.into()),
            },
            "the end of turn {ended}"
        );
    }
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
    let mut watching = watch(JobState::Running, "t1");
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
        state(JobState::WaitingForInput, "t1"),
    ] {
        watching.push(Seq(1), &event);
    }

    let waited = watching.finish(SessionId::new("s"));
    assert_eq!(
        (waited.state, waited.done, waited.inputs),
        (JobState::WaitingForInput, true, vec![request(2)])
    );
}
