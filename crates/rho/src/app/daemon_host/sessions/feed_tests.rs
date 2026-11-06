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

fn cursor(feed: &Feed, seq: u64) -> Option<Cursor> {
    Some(Cursor {
        stream: feed.state().stream.clone(),
        seq: Seq(seq),
    })
}

#[test]
fn attach_resumes_a_buffered_cursor_and_resets_anything_else() {
    // One saved turn, then a turn in progress.
    let feed = Feed::new();
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
    let feed = Feed::new();
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
    let feed = Feed::new();
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
    let feed = Feed::new();
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
fn an_attach_reports_the_job_state_its_replay_ends_in() {
    let feed = Feed::new();
    assert_eq!(feed.attach(None).job, wire::JobState::Idle);
    feed.publish([wire::SessionEvent::State {
        state: wire::JobState::Running,
        error: None,
    }]);
    feed.publish(notices(&["a reply"]));
    assert_eq!(feed.attach(None).job, wire::JobState::Running);
}

// Covers: an event JSON cannot hold, live or saved, never reaches a reset or
// a subscriber, whose forwarder relies on every event encoding.
#[cfg(unix)]
#[test]
fn events_json_cannot_hold_are_dropped_live_and_saved() {
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt, path::PathBuf};

    let unencodable = || {
        wire::SessionEvent::ToolCall(wire::ToolCallUpdate {
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
        })
    };
    let feed = Feed::new();
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
