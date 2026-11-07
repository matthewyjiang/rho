use pretty_assertions::assert_eq;
use rho_protocol::{
    AnswerStop, Cursor, JobState, MessageId, Replay, Seq, SessionAttachResponse, SessionEvent,
    StreamId, TurnError, TurnErrorKind, TurnOutcome,
};

use super::{Ending, Follow};

fn response(replay: Replay, replay_events: u64, state: JobState) -> SessionAttachResponse {
    SessionAttachResponse {
        stream: StreamId::new("stream"),
        replay,
        replay_events,
        state,
        inputs: Vec::new(),
    }
}

fn answered(text: &str) -> TurnOutcome {
    TurnOutcome::Answered {
        text: text.into(),
        stop: AnswerStop::EndTurn,
    }
}

fn turn_ended(outcome: TurnOutcome) -> SessionEvent {
    SessionEvent::TurnEnded { outcome }
}

fn state(state: JobState) -> SessionEvent {
    SessionEvent::State { state }
}

fn chunk(text: &str) -> SessionEvent {
    SessionEvent::AgentMessageChunk {
        message: MessageId::new("message"),
        text: text.into(),
    }
}

// Covers: which event ends an attachment. A reset's replay is history, so
// an earlier turn's `turn_ended` in it ends nothing, and once the replay is
// over the job's state says whether the turn reported already ended; live
// and tail events end it with their `turn_ended`, or an interrupted state.
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
            response: response(Replay::Reset, 3, JobState::Completed),
            events: vec![
                chunk("answer"),
                turn_ended(answered("answer")),
                state(JobState::Completed),
            ],
            ends: Some((3, Ending::Turn(answered("answer")))),
        },
        Case {
            name: "an earlier turn's end in a reset ends nothing; the live one does",
            response: response(Replay::Reset, 3, JobState::Running),
            events: vec![
                turn_ended(answered("earlier")),
                state(JobState::Completed),
                state(JobState::Running),
                chunk("partial"),
                turn_ended(failed.clone()),
            ],
            ends: Some((5, Ending::Turn(failed.clone()))),
        },
        Case {
            name: "an empty reset of an ended job reports it at once",
            response: response(Replay::Reset, 0, JobState::Cancelled),
            events: vec![],
            ends: Some((0, Ending::Unrecorded(JobState::Cancelled))),
        },
        Case {
            name: "an idle session waits for its first turn",
            response: response(Replay::Reset, 0, JobState::Idle),
            events: vec![state(JobState::Running), turn_ended(answered("first"))],
            ends: Some((2, Ending::Turn(answered("first")))),
        },
        Case {
            name: "a turn the daemon's stop cut short ends interrupted",
            response: response(Replay::Reset, 1, JobState::Running),
            events: vec![chunk("partial"), state(JobState::Interrupted)],
            ends: Some((2, Ending::Interrupted)),
        },
        Case {
            name: "a tail replay's turn end is news",
            response: response(Replay::Tail, 2, JobState::Completed),
            events: vec![turn_ended(answered("missed")), state(JobState::Completed)],
            ends: Some((1, Ending::Turn(answered("missed")))),
        },
        Case {
            name: "a turn under way runs on",
            response: response(Replay::Reset, 1, JobState::WaitingForInput),
            events: vec![chunk("partial"), chunk(" more")],
            ends: None,
        },
    ];
    for case in cases {
        let (mut follow, mut ending) = Follow::new(&case.response);
        let mut taken = 0;
        for (seq, event) in (1..).zip(&case.events) {
            if ending.is_some() {
                break;
            }
            ending = follow.take(Seq(seq), event);
            taken += 1;
        }
        let ended = ending.map(|ending| (taken, ending));
        assert_eq!(ended, case.ends, "{}", case.name);
    }
}

// Covers: a reattach resumes after the highest seq seen, never a reset's
// seq-0 transcript events, and a reset drops the cursor it replaces.
// Owner: rho attach follow logic (unit).
#[test]
fn the_cursor_is_the_highest_seq_seen_since_the_last_reset() {
    let (mut follow, _) = Follow::new(&response(Replay::Reset, 3, JobState::Running));
    follow.take(Seq(0), &chunk("saved"));
    assert_eq!(follow.cursor(), None);
    follow.take(Seq(4), &chunk("saved, last"));
    follow.take(Seq(5), &chunk("live"));
    let cursor = Cursor {
        stream: StreamId::new("stream"),
        seq: Seq(5),
    };
    assert_eq!(follow.cursor(), Some(&cursor));

    follow.reattached(&response(Replay::Tail, 0, JobState::Running));
    assert_eq!(follow.cursor(), Some(&cursor));
    follow.reattached(&response(Replay::Reset, 1, JobState::Running));
    assert_eq!(follow.cursor(), None);
}
