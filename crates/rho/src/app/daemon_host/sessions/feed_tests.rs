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
    // A saved transcript of one turn, then a turn in progress.
    let feed = Feed::new(notices(&["saved"]), 1);
    feed.begin_turn();
    feed.publish(notices(&["a", "b", "c"]));

    let other_stream = Some(Cursor {
        stream: rho_protocol::StreamId::new("an earlier daemon"),
        seq: Seq(1),
    });
    let reset = (
        Replay::Reset,
        vec![
            (0, notice("saved")),
            (1, notice("a")),
            (2, notice("b")),
            (3, notice("c")),
        ],
    );
    for (name, after, expected) in [
        ("no cursor", None, reset.clone()),
        ("another stream's cursor", other_stream, reset.clone()),
        ("a cursor past the stream", cursor(&feed, 4), reset.clone()),
        (
            "a buffered cursor",
            cursor(&feed, 1),
            (Replay::Tail, vec![(2, notice("b")), (3, notice("c"))]),
        ),
        (
            "the latest cursor",
            cursor(&feed, 3),
            (Replay::Tail, vec![]),
        ),
        (
            "the stream's start",
            cursor(&feed, 0),
            (
                Replay::Tail,
                vec![(1, notice("a")), (2, notice("b")), (3, notice("c"))],
            ),
        ),
    ] {
        assert_eq!(replay(&feed, after), expected, "{name}");
    }
}

#[test]
fn a_saved_turn_moves_into_the_transcript_without_repeating_or_dropping_events() {
    let feed = Feed::new(notices(&["first turn"]), 1);
    feed.begin_turn();
    feed.publish(notices(&["live prompt", "live reply"]));
    // The turn saved two display rows; their replay joins the transcript,
    // and its live events stop being part of a reset.
    feed.end_turn(3, |from| {
        assert_eq!(from, 1, "only the rows saved since the last turn replay");
        notices(&["saved prompt", "saved reply"])
    });
    feed.begin_turn();
    feed.publish(notices(&["next prompt"]));

    assert_eq!(
        replay(&feed, None),
        (
            Replay::Reset,
            vec![
                (2, notice("first turn")),
                (2, notice("saved prompt")),
                (2, notice("saved reply")),
                (3, notice("next prompt")),
            ]
        ),
    );
    // A client that saw the live turn still resumes from the buffer.
    assert_eq!(
        replay(&feed, cursor(&feed, 1)),
        (
            Replay::Tail,
            vec![(2, notice("live reply")), (3, notice("next prompt"))]
        ),
    );
}

#[test]
fn a_turn_larger_than_the_buffer_resets_with_a_notice_instead_of_its_start() {
    let feed = Feed::new(Vec::new(), 0);
    feed.begin_turn();
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
    assert_eq!(replay(&feed, cursor(&feed, 0)).0, Replay::Reset);
}
