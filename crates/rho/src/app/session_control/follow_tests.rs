use pretty_assertions::assert_eq;
use rho_protocol::{
    AnswerStop, Ask, InputAsker, InputRequestId, Question, QuestionAsk, Resolution, Selection,
};

use super::*;

/// A watch of a job in `state`, of turn `turn`.
fn watch(state: JobState, turn: &str) -> Watch {
    Watch {
        session: SessionId::new("s"),
        stream: StreamId::new("stream"),
        seq: Seq(0),
        state,
        turn: Some(TurnId::new(turn)),
        ended: None,
        inputs: Vec::new(),
        activity: None,
        reported: None,
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
        (watching.result(), ended.result()),
        (
            WaitResult {
                session: SessionId::new("s"),
                state: JobState::Running,
                done: false,
                outcome: None,
                turn: Some(TurnId::new("t2")),
                inputs: Vec::new(),
                cursor: Some("stream:7".into()),
            },
            WaitResult {
                session: SessionId::new("s"),
                state: JobState::Completed,
                done: true,
                outcome: Some(answered("new")),
                turn: Some(TurnId::new("t2")),
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
            watching.result(),
            WaitResult {
                session: SessionId::new("s"),
                state: JobState::Running,
                done: false,
                outcome: None,
                turn: Some(TurnId::new("t2")),
                inputs: Vec::new(),
                cursor: Some(checkpoint.into()),
            },
            "the end of turn {ended}"
        );
    }
}

/// Question `id`, asked by `asker`.
fn question(id: u64, asker: InputAsker) -> InputRequest {
    InputRequest {
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
        asker,
    }
}

fn activity(state: SessionActivity) -> SessionEvent {
    SessionEvent::Activity { state }
}

// Covers: a turn waiting for input stops `wait` with the requests still
// open, and one a person answered drops out.
// Owner: session-control wait.
#[test]
fn a_turn_waiting_for_input_reports_its_open_requests() {
    let request = |id| question(id, InputAsker::Turn);
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

    let waited = watching.result();
    assert_eq!(
        (waited.state, waited.done, waited.inputs),
        (JobState::WaitingForInput, true, vec![request(2)])
    );
}

/// What `wait` returns of a watch.
#[derive(Debug, PartialEq)]
struct Waited {
    done: bool,
    outcome: Option<TurnOutcome>,
    turn: Option<TurnId>,
    inputs: Vec<InputRequest>,
    cursor: Option<String>,
}

// Covers: `wait` is done once the session settles, not once a turn ends: a
// turn that ended while background work is still due (a result to deliver,
// or a task left running that may never let it settle) keeps it waiting
// but still returns the turn's outcome, with the cursor held before the
// turn's end so the next wait reads it again; the delivery turn's outcome
// then replaces it; a subagent question open at rest stops it with the
// question listed; one open while a turn runs does not stop it, but is
// listed, so the check of an answer finds it; a stream that never reports activity, as a subagent
// run's, is done once its turn ends.
// Owner: session-control wait.
#[test]
fn wait_is_done_once_the_session_settles() {
    let first_turn_ended = || {
        vec![
            activity(SessionActivity::Active),
            state(JobState::Running, "t1"),
            turn_ended("t1", answered("first")),
            state(JobState::Completed, "t1"),
        ]
    };
    let delivered = || {
        let mut events = first_turn_ended();
        events.extend([
            state(JobState::Running, "t2"),
            turn_ended("t2", answered("delivered")),
            state(JobState::Completed, "t2"),
            activity(SessionActivity::Settled),
        ]);
        events
    };
    let asked = || {
        let mut events = first_turn_ended();
        events.push(SessionEvent::InputRequested {
            request: Box::new(question(1, InputAsker::Subagent)),
        });
        events
    };
    let asked_while_running = || {
        vec![
            activity(SessionActivity::Active),
            state(JobState::Running, "t1"),
            SessionEvent::InputRequested {
                request: Box::new(question(1, InputAsker::Subagent)),
            },
        ]
    };
    let unreported = || {
        vec![
            state(JobState::Running, "t1"),
            turn_ended("t1", answered("first")),
            state(JobState::Completed, "t1"),
        ]
    };
    let cases = [
        (
            "a turn ended, background work still due",
            first_turn_ended(),
            Waited {
                done: false,
                outcome: Some(answered("first")),
                turn: Some(TurnId::new("t1")),
                inputs: Vec::new(),
                cursor: Some("stream:2".into()),
            },
        ),
        (
            "the delivery turn ended and the session settled",
            delivered(),
            Waited {
                done: true,
                outcome: Some(answered("delivered")),
                turn: Some(TurnId::new("t2")),
                inputs: Vec::new(),
                cursor: Some("stream:8".into()),
            },
        ),
        (
            "a subagent asks at rest",
            asked(),
            Waited {
                done: true,
                outcome: Some(answered("first")),
                turn: Some(TurnId::new("t1")),
                inputs: vec![question(1, InputAsker::Subagent)],
                cursor: Some("stream:5".into()),
            },
        ),
        (
            "a subagent asks while a turn runs",
            asked_while_running(),
            Waited {
                done: false,
                outcome: None,
                turn: Some(TurnId::new("t1")),
                inputs: vec![question(1, InputAsker::Subagent)],
                cursor: Some("stream:3".into()),
            },
        ),
        (
            "a stream that reports no activity",
            unreported(),
            Waited {
                done: true,
                outcome: Some(answered("first")),
                turn: Some(TurnId::new("t1")),
                inputs: Vec::new(),
                cursor: Some("stream:3".into()),
            },
        ),
    ];
    for (name, events, expected) in cases {
        let mut watching = watch(JobState::Idle, "t0");
        for (seq, event) in (1..).zip(&events) {
            watching.push(Seq(seq), event);
        }
        let WaitResult {
            done,
            outcome,
            turn,
            inputs,
            cursor,
            ..
        } = watching.result();
        assert_eq!(
            Waited {
                done,
                outcome,
                turn,
                inputs,
                cursor,
            },
            expected,
            "{name}"
        );
    }
}

// Covers: a follow reports each time the session is done with something
// new, and only then: the first look at a session done already, a request
// for a person, the session settling after a person answered it, and a
// second request while the first is open; events while it stays done with
// the same thing, as a title or a repeated activity, are not news again.
// A webhook watch POSTs each piece of news, so a repeat is a billed run.
// Owner: session-control follow news.
#[test]
fn a_follow_reports_each_new_reason_it_is_done_once() {
    let request = |id| SessionEvent::InputRequested {
        request: Box::new(question(id, InputAsker::Turn)),
    };
    let resolved = |id| SessionEvent::InputResolved {
        id: InputRequestId(id),
        resolution: Resolution::Answered,
    };
    let cases = [
        (
            "settled already, then quiet",
            JobState::Completed,
            vec![
                activity(SessionActivity::Settled),
                SessionEvent::TitleChanged {
                    title: "named".into(),
                },
                activity(SessionActivity::Settled),
            ],
            vec![(JobState::Completed, Vec::new())],
        ),
        (
            "asks, is answered, settles",
            JobState::Running,
            vec![
                activity(SessionActivity::Active),
                request(3),
                state(JobState::WaitingForInput, "t1"),
                resolved(3),
                state(JobState::Running, "t1"),
                turn_ended("t1", answered("done")),
                state(JobState::Completed, "t1"),
                activity(SessionActivity::Settled),
            ],
            vec![
                (
                    JobState::WaitingForInput,
                    vec![question(3, InputAsker::Turn)],
                ),
                // Answered, before the turn runs again: still done, with
                // nothing open, which a watch does not deliver.
                (JobState::WaitingForInput, Vec::new()),
                (JobState::Completed, Vec::new()),
            ],
        ),
        (
            "a second request while the first is open",
            JobState::Running,
            vec![
                request(3),
                state(JobState::WaitingForInput, "t1"),
                request(4),
            ],
            vec![
                (
                    JobState::WaitingForInput,
                    vec![question(3, InputAsker::Turn)],
                ),
                (
                    JobState::WaitingForInput,
                    vec![question(3, InputAsker::Turn), question(4, InputAsker::Turn)],
                ),
            ],
        ),
    ];
    for (name, start, events, expected) in cases {
        let mut watching = watch(start, "t1");
        let mut news = Vec::new();
        let mut look = |watching: &mut Watch| {
            if let Some(result) = watching.news() {
                news.push((result.state, result.inputs));
            }
        };
        look(&mut watching);
        for (seq, event) in (1..).zip(&events) {
            watching.push(Seq(seq), event);
            look(&mut watching);
        }
        assert_eq!(news, expected, "{name}");
    }
}
