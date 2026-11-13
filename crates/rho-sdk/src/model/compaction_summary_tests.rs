use pretty_assertions::assert_eq;

use super::*;
use crate::model::{Message, WireMessage};

// Covers: providers receive summaries under the labels they always have, and
// the legacy loader keys on the same strings. Edit only with a migration.
// Owner: SDK compaction-summary wire rendering.
#[test]
fn compaction_summary_wire_rendering_is_frozen() {
    let cases = [
        (
            Some(CompactionTrigger::Automatic),
            vec!["Automatic compaction summary of earlier conversation, for model context only. It is not a new user message.", "s"],
        ),
        (
            Some(CompactionTrigger::Manual),
            vec!["Manual compaction summary of earlier conversation, for model context only. It is not a new user message.", "s"],
        ),
        (
            Some(CompactionTrigger::ContextOverflow),
            vec!["Compaction summary of earlier conversation after the context window overflowed, for model context only. It is not a new user message.", "s"],
        ),
        (
            None,
            vec!["Automatic compaction summary of earlier conversation for model context only:\n\ns"],
        ),
    ];
    for (trigger, expected) in cases {
        let message = Message::CompactionSummary(CompactionSummary {
            trigger,
            text: "s".into(),
        });
        let WireMessage::User(content) = message.wire() else {
            panic!("summaries are sent as user-role content");
        };
        let expected = expected
            .into_iter()
            .map(|text| ContentBlock::Text(text.into()))
            .collect::<Vec<_>>();
        assert_eq!(content.into_owned(), expected, "{trigger:?}");
    }
}
