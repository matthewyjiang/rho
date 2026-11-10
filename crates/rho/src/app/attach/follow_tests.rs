use pretty_assertions::assert_eq;
use rho_protocol::{
    AnswerStop, JobState, MessageId, Replay, Seq, SessionAttachResponse, SessionEvent, TurnError,
    TurnErrorKind, TurnId, TurnOutcome,
};

use super::{Ending, Follow};
use crate::app::daemon_client::test_support::attach_response;

fn answered(text: &str) -> TurnOutcome {
    TurnOutcome::Answered {
        text: text.into(),
        stop: AnswerStop::EndTurn,
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

fn user(id: &str) -> SessionEvent {
    SessionEvent::UserMessage {
        message: MessageId::new(id),
        content: Vec::new(),
        steer: None,
    }
}

fn chunk(text: &str) -> SessionEvent {
    SessionEvent::AgentMessageChunk {
        message: MessageId::new("message"),
        text: text.into(),
    }
}

// Covers: which event ends an attachment: the `turn_ended` of the turn it
// follows, by id, and no other turn's, wherever it arrives. A reset's
// replay is history, so the end it holds counts once the replay is over.
// Owner: rho attach follow logic (unit).
#[test]
fn an_attachment_ends_with_the_turn_it_follows() {
    let failed = TurnOutcome::Failed {
        error: TurnError {
            kind: TurnErrorKind::Provider,
            message: "provider failed".into(),
        },
    };
    struct Case {
        name: &'static str,
        response: SessionAttachResponse,
        events: Vec<SessionEvent>,
        /// How it ends, and after how many events.
        ends: Option<(usize, Ending)>,
    }
    let cases = [
        Case {
            name: "a reset after the turn ended reports it once the replay is over",
            response: attach_response(Replay::Reset, 3, JobState::Completed, Some("t1")),
            events: vec![
                chunk("answer"),
                turn_ended("t1", answered("answer")),
                state(JobState::Completed, "t1"),
            ],
            ends: Some((3, Ending::Turn(answered("answer")))),
        },
        Case {
            name: "an earlier turn's end in a reset ends nothing; the followed turn's does",
            response: attach_response(Replay::Reset, 2, JobState::Running, Some("t2")),
            events: vec![
                turn_ended("t1", answered("earlier")),
                state(JobState::Running, "t2"),
                chunk("partial"),
                turn_ended("t2", failed.clone()),
            ],
            ends: Some((4, Ending::Turn(failed.clone()))),
        },
        Case {
            name: "a reset of an ended job that holds no turn end is a daemon bug",
            response: attach_response(Replay::Reset, 0, JobState::Cancelled, Some("t1")),
            events: vec![],
            ends: Some((0, Ending::Unreported)),
        },
        Case {
            name: "a reset that holds only an earlier turn's end does not report it",
            response: attach_response(Replay::Reset, 2, JobState::Completed, Some("t2")),
            events: vec![turn_ended("t1", answered("earlier")), user("next")],
            ends: Some((2, Ending::Unreported)),
        },
        Case {
            name: "an idle session waits for its first turn",
            response: attach_response(Replay::Reset, 0, JobState::Idle, None),
            events: vec![
                state(JobState::Running, "t1"),
                turn_ended("t1", answered("first")),
            ],
            ends: Some((2, Ending::Turn(answered("first")))),
        },
        Case {
            name: "a turn the daemon's stop cut short ends interrupted",
            response: attach_response(Replay::Reset, 1, JobState::Running, Some("t1")),
            events: vec![chunk("partial"), state(JobState::Interrupted, "t1")],
            ends: Some((2, Ending::Interrupted)),
        },
        Case {
            name: "a tail replay's turn end is news",
            response: attach_response(Replay::Tail, 2, JobState::Completed, Some("t1")),
            events: vec![
                turn_ended("t1", answered("missed")),
                state(JobState::Completed, "t1"),
            ],
            ends: Some((1, Ending::Turn(answered("missed")))),
        },
        Case {
            name: "a turn under way runs on",
            response: attach_response(Replay::Reset, 1, JobState::WaitingForInput, Some("t1")),
            events: vec![chunk("partial"), chunk(" more")],
            ends: None,
        },
    ];
    for case in cases {
        let (mut follow, mut ending) = Follow::attached(&case.response);
        let mut taken = 0;
        for (seq, event) in (1..).zip(&case.events) {
            if ending.is_some() {
                break;
            }
            ending = follow.observe(Seq(seq), event);
            taken += 1;
        }
        let ended = ending.map(|ending| (taken, ending));
        assert_eq!(ended, case.ends, "{}", case.name);
    }
}
