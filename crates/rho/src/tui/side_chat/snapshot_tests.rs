use rho_sdk::model::{ContentBlock, Message};

use super::frozen_parent_snapshot;

fn user(text: &str) -> Message {
    Message::User(vec![ContentBlock::Text(text.into())])
}

fn assistant(text: &str) -> Message {
    Message::Assistant(vec![ContentBlock::Text(text.into())])
}

// Covers: the aside must see the parent transcript that existed at freeze
// time, not later parent messages.
// Owner: side-chat snapshot
#[test]
fn frozen_snapshot_keeps_parent_turn_text_and_stays_frozen() {
    let mut messages = vec![user("remember zebra-context"), assistant("ok, noted")];
    let snapshot = frozen_parent_snapshot(&messages);
    pretty_assertions::assert_eq!(
        (
            snapshot.contains("zebra-context"),
            snapshot.contains("ok, noted")
        ),
        (true, true)
    );

    messages.push(user("later-only"));
    pretty_assertions::assert_eq!(snapshot.contains("later-only"), false);
    assert_ne!(snapshot, frozen_parent_snapshot(&messages));
}

// Covers: an empty parent session still produces a snapshot so the first
// aside can start without special-casing None.
// Owner: side-chat snapshot
#[test]
fn frozen_snapshot_from_empty_history_is_stable() {
    let first = frozen_parent_snapshot(&[]);
    let second = frozen_parent_snapshot(&[]);
    pretty_assertions::assert_eq!(first, second);
    assert!(!first.is_empty());
}
