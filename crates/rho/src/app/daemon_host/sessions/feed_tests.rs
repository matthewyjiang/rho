use pretty_assertions::assert_eq;
use rho_protocol::{self as wire, Cursor, Replay, Seq};

use super::{Feed, REPLAY_BUFFER_BYTES, TURN_START_EVICTED};

fn notice(text: &str) -> wire::SessionEvent {
    wire::SessionEvent::Notice { text: text.into() }
}

fn notices(texts: &[&str]) -> Vec<wire::SessionEvent> {
    texts.iter().map(|text| notice(text)).collect()
}

/// The events an attach from `after` replays, and whether it resets.
fn replay(feed: &Feed, after: Option<Cursor>) -> (Replay, Vec<(u64, wire::SessionEvent)>) {
    let (replay, events) = feed.state().replay_from(after.as_ref());
    let events = events
        .into_iter()
        .map(|(seq, event)| (seq.0, event))
        .collect();
    (replay, events)
}

/// Options that differ only in their permission mode.
fn options(mode: wire::PermissionMode) -> wire::SessionOptions {
    wire::SessionOptions {
        model: wire::ModelOptions {
            auth: "api-key".into(),
            current: wire::ModelRef {
                provider: "test".into(),
                model: "model".into(),
            },
            available: Vec::new(),
        },
        reasoning: None,
        fast_mode: None,
        permission_mode: wire::PermissionModeOptions {
            current: mode,
            available: vec![wire::PermissionMode::Bypass, wire::PermissionMode::Plan],
        },
        commands: Vec::new(),
        goal: None,
    }
}

fn idle_feed() -> Feed {
    Feed::new(
        wire::JobState::Idle,
        /*turn*/ None,
        /*title*/ None,
        options(wire::PermissionMode::Bypass),
    )
}

fn cursor(feed: &Feed, seq: u64) -> Option<Cursor> {
    Some(Cursor {
        stream: feed.state().stream.clone(),
        seq: Seq(seq),
    })
}

#[test]
fn attach_resumes_a_buffered_cursor_and_resets_anything_else() {
    // One saved turn, then a turn in progress.
    let feed = idle_feed();
    feed.publish(notices(&["live"]));
    feed.end_turn(notices(&["saved"]));
    feed.publish(notices(&["a", "b", "c"]));

    let other_stream = Some(Cursor {
        stream: rho_protocol::StreamId::new("an earlier daemon"),
        seq: Seq(1),
    });
    let reset = (
        Replay::Reset,
        vec![
            (1, notice("saved")),
            (2, notice("a")),
            (3, notice("b")),
            (4, notice("c")),
        ],
    );
    for (name, after, expected) in [
        ("no cursor", None, reset.clone()),
        ("another stream's cursor", other_stream, reset.clone()),
        ("a cursor past the stream", cursor(&feed, 5), reset.clone()),
        // What a client holds that left partway through a transcript.
        ("a cursor at 0", cursor(&feed, 0), reset.clone()),
        (
            "the transcript's end",
            cursor(&feed, 1),
            (
                Replay::Tail,
                vec![(2, notice("a")), (3, notice("b")), (4, notice("c"))],
            ),
        ),
        (
            "a buffered cursor",
            cursor(&feed, 2),
            (Replay::Tail, vec![(3, notice("b")), (4, notice("c"))]),
        ),
        (
            "the latest cursor",
            cursor(&feed, 4),
            (Replay::Tail, vec![]),
        ),
    ] {
        assert_eq!(replay(&feed, after), expected, "{name}");
    }
}

#[test]
fn a_saved_turn_moves_into_the_transcript_without_repeating_or_dropping_events() {
    let feed = idle_feed();
    feed.publish(notices(&["first live"]));
    feed.end_turn(notices(&["first turn"]));
    feed.publish(notices(&["live prompt", "live reply"]));
    // The turn's replay from its saved rows joins the transcript, and its
    // live events stop being part of a reset.
    feed.end_turn(notices(&["saved prompt", "saved reply"]));
    feed.publish(notices(&["next prompt"]));

    assert_eq!(
        replay(&feed, None),
        (
            Replay::Reset,
            vec![
                (0, notice("first turn")),
                (0, notice("saved prompt")),
                (3, notice("saved reply")),
                (4, notice("next prompt")),
            ]
        ),
    );
    // A client that saw the live turn still resumes from the buffer.
    assert_eq!(
        replay(&feed, cursor(&feed, 2)),
        (
            Replay::Tail,
            vec![(3, notice("live reply")), (4, notice("next prompt"))]
        ),
    );
}

// Covers: a turn whose saved rows could not be read must not vanish from
// resets once the next turn starts.
#[test]
fn a_turn_that_did_not_save_replays_live_until_a_later_turn_saves_it() {
    let feed = idle_feed();
    feed.publish(notices(&["unsaved turn"]));
    feed.publish(notices(&["next turn"]));
    assert_eq!(
        replay(&feed, None),
        (
            Replay::Reset,
            vec![(1, notice("unsaved turn")), (2, notice("next turn"))]
        ),
    );

    feed.end_turn(notices(&["both turns saved"]));
    assert_eq!(
        replay(&feed, None),
        (Replay::Reset, vec![(2, notice("both turns saved"))]),
    );
}

#[test]
fn a_turn_larger_than_the_buffer_resets_with_a_notice_instead_of_its_start() {
    let feed = idle_feed();
    let large = "x".repeat(REPLAY_BUFFER_BYTES / 4);
    feed.publish(std::iter::repeat_with(|| notice(&large)).take(5));
    feed.publish(notices(&["latest"]));

    let (kind, events) = replay(&feed, None);
    let seqs: Vec<_> = events.iter().map(|(seq, _)| *seq).collect();
    assert_eq!(kind, Replay::Reset);
    assert_eq!(events[0], (0, notice(TURN_START_EVICTED)));
    assert_eq!(events.last(), Some(&(6, notice("latest"))));
    assert!(
        seqs[1] > 1,
        "the turn's first events were evicted: {seqs:?}"
    );
    // The evicted start can no longer be resumed from.
    assert_eq!(replay(&feed, cursor(&feed, 1)).0, Replay::Reset);
}

#[test]
fn an_attach_reports_the_job_state_its_replay_ends_in_and_its_turn() {
    let feed = idle_feed();
    let idle = feed.attach(None);
    assert_eq!((idle.job, idle.turn), (wire::JobState::Idle, None));
    let turn = wire::TurnId::new("t1");
    feed.publish([wire::SessionEvent::State {
        state: wire::JobState::Running,
        turn: Some(turn.clone()),
    }]);
    feed.publish(notices(&["a reply"]));
    let running = feed.attach(None);
    assert_eq!(
        (running.job, running.turn),
        (wire::JobState::Running, Some(turn))
    );
}

// Covers: an attach reports the session's title, the saved one a reopened
// session starts with until a title change replaces it, since no saved
// transcript holds a title (#1670).
// Owner: daemon session feed.
#[test]
fn an_attach_reports_the_saved_title_until_a_title_change() {
    let feed = Feed::new(
        wire::JobState::Idle,
        /*turn*/ None,
        Some("Saved title".into()),
        options(wire::PermissionMode::Bypass),
    );
    assert_eq!(feed.attach(None).title, Some("Saved title".into()));
    feed.publish([wire::SessionEvent::TitleChanged {
        title: "New title".into(),
    }]);
    assert_eq!(feed.attach(None).title, Some("New title".into()));
}

// Covers: an attach reports the input requests open as they stand after
// its replay: resolved ones gone, and a held one without the timeout it
// opened with, so a late client neither shows a countdown nor misses one.
// Owner: daemon session feed.
#[test]
fn an_attach_reports_open_requests_as_they_stand() {
    let request = |id: u64| wire::InputRequest {
        id: wire::InputRequestId(id),
        ask: wire::Ask::PlanApproval(wire::PlanApprovalAsk {
            plan: "the plan".into(),
            modes: vec![wire::PermissionMode::Auto],
        }),
        tool_call: None,
        timeout: Some(wire::InputTimeout {
            deadline_ms: 1,
            answer: wire::InputAnswer::KeepPlanning,
            reason: "safe".into(),
        }),
    };
    let feed = idle_feed();
    feed.publish([
        wire::SessionEvent::InputRequested {
            request: Box::new(request(0)),
        },
        wire::SessionEvent::InputRequested {
            request: Box::new(request(1)),
        },
        wire::SessionEvent::InputRequested {
            request: Box::new(request(2)),
        },
        wire::SessionEvent::InputHeld {
            id: wire::InputRequestId(0),
        },
        wire::SessionEvent::InputResolved {
            id: wire::InputRequestId(1),
            resolution: wire::Resolution::TimedOut,
        },
    ]);
    let held = wire::InputRequest {
        timeout: None,
        ..request(0)
    };
    assert_eq!(feed.attach(None).inputs, vec![held, request(2)]);
}

// Covers: clients hear of an options change once, and an attach reports the
// options as they stand, so a configure that changes nothing sends nothing.
// Owner: daemon session feed
#[test]
fn options_reach_clients_only_when_they_change() {
    let feed = idle_feed();
    feed.update_options(options(wire::PermissionMode::Bypass));
    feed.update_options(options(wire::PermissionMode::Plan));
    feed.update_options(options(wire::PermissionMode::Plan));

    let changed = wire::SessionEvent::OptionsChanged {
        options: Box::new(options(wire::PermissionMode::Plan)),
    };
    assert_eq!(replay(&feed, None), (Replay::Reset, vec![(1, changed)]));
    assert_eq!(
        feed.attach(None).options,
        options(wire::PermissionMode::Plan)
    );
}

// Covers: an event JSON cannot hold, live or saved, never reaches a reset or
// a subscriber, whose forwarder relies on every event encoding.
#[cfg(unix)]
#[test]
fn events_json_cannot_hold_are_dropped_live_and_saved() {
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt, path::PathBuf};

    let unencodable = || {
        wire::SessionEvent::tool_call(wire::ToolCallUpdate {
            id: wire::ToolCallId::new("call"),
            title: None,
            kind: None,
            status: None,
            raw_input: None,
            content: None,
            locations: Some(vec![wire::Location {
                path: PathBuf::from(OsStr::from_bytes(b"/work/\xff")),
                line: None,
            }]),
            display: None,
        })
    };
    let feed = idle_feed();
    feed.publish([unencodable(), notice("live")]);
    assert_eq!(
        replay(&feed, None),
        (Replay::Reset, vec![(1, notice("live"))])
    );

    feed.end_turn(vec![unencodable(), notice("saved")]);
    assert_eq!(
        replay(&feed, None),
        (Replay::Reset, vec![(1, notice("saved"))])
    );
}
