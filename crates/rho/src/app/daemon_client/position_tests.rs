use pretty_assertions::assert_eq;
use rho_protocol::{Cursor, JobState, Replay, Seq, StreamId};

use super::StreamPosition;
use crate::app::daemon_client::test_support::attach_response;

// Covers: a reattach resumes after the highest seq seen, never a reset's
// seq-0 transcript events, and a reset drops the cursor it replaces.
// Owner: daemon client stream position (unit).
#[test]
fn the_cursor_is_the_highest_seq_seen_since_the_last_reset() {
    let mut position =
        StreamPosition::attached(&attach_response(Replay::Reset, 3, JobState::Running));
    position.observe(Seq(0));
    assert_eq!(position.cursor(), None);
    position.observe(Seq(4));
    position.observe(Seq(5));
    let cursor = Cursor {
        stream: StreamId::new("stream"),
        seq: Seq(5),
    };
    assert_eq!(position.cursor(), Some(&cursor));

    position.reattached(&attach_response(Replay::Tail, 0, JobState::Running));
    assert_eq!(position.cursor(), Some(&cursor));
    position.reattached(&attach_response(Replay::Reset, 1, JobState::Running));
    assert_eq!(position.cursor(), None);
}

// Covers: the events either kind of replay holds count as replayed, which
// clients keep from what they treat as news, and the events after them as
// news.
// Owner: daemon client stream position (unit).
#[test]
fn both_kinds_of_replay_count_as_replayed() {
    let mut position =
        StreamPosition::attached(&attach_response(Replay::Reset, 1, JobState::Running));
    let mut replaying = Vec::new();
    for seq in [1, 2] {
        replaying.push(position.replaying());
        position.observe(Seq(seq));
    }
    position.reattached(&attach_response(Replay::Tail, 1, JobState::Running));
    for seq in [3, 4] {
        replaying.push(position.replaying());
        position.observe(Seq(seq));
    }
    assert_eq!(replaying, [true, false, true, false]);
}
