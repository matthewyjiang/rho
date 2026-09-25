use pretty_assertions::assert_eq;

use super::*;
use crate::model::ImageContent;

// Covers: summaries stay recognizable by trigger after a session round trip,
// legacy single-block summaries still load as summaries, and ordinary user
// input (including a pasted header without a body) is never mistaken for one.
// Owner: SDK compatibility encoding.
#[test]
fn recognize_compaction_summaries_by_encoding_not_role() {
    let header_only = || match Message::compaction_summary(CompactionTrigger::Manual, "x") {
        Message::User(mut blocks) => {
            blocks.pop();
            Message::User(blocks)
        }
        _ => unreachable!(),
    };
    let header_with_image = || match Message::compaction_summary(CompactionTrigger::Manual, "x") {
        Message::User(mut blocks) => {
            blocks[1] = ContentBlock::Image(ImageContent {
                data: "aW1hZ2U=".into(),
                mime_type: "image/png".into(),
            });
            Message::User(blocks)
        }
        _ => unreachable!(),
    };
    let cases = [
        (
            Message::compaction_summary(CompactionTrigger::Automatic, "auto"),
            Some((Some(CompactionTrigger::Automatic), "auto")),
        ),
        (
            Message::compaction_summary(CompactionTrigger::Manual, "manual"),
            Some((Some(CompactionTrigger::Manual), "manual")),
        ),
        (
            Message::compaction_summary(CompactionTrigger::ContextOverflow, "overflow"),
            Some((Some(CompactionTrigger::ContextOverflow), "overflow")),
        ),
        (
            Message::user_text(format!("{LEGACY_PREFIX}legacy")),
            Some((None, "legacy")),
        ),
        (Message::user_text("summarize this for me"), None),
        (
            Message::User(vec![
                ContentBlock::Text("please compact".into()),
                ContentBlock::Text("summary".into()),
            ]),
            None,
        ),
        (header_only(), None),
        (header_with_image(), None),
        (Message::assistant_text(format!("{LEGACY_PREFIX}x")), None),
    ];

    for (message, expected) in cases {
        let restored =
            serde_json::from_str::<Message>(&serde_json::to_string(&message).unwrap()).unwrap();
        let recognized = restored
            .as_compaction_summary()
            .map(|summary| (summary.trigger(), summary.text()));
        assert_eq!(recognized, expected, "{message:?}");
    }
}

// Covers: the persisted encoding is frozen, so sessions saved by any release
// keep loading as summaries. Edit these literals only with a migration.
// Owner: SDK compatibility encoding.
#[test]
fn compaction_summary_wire_format_is_frozen() {
    let cases = [
        (
            CompactionTrigger::Automatic,
            r#"{"User":[{"Text":"Automatic compaction summary of earlier conversation, for model context only. It is not a new user message."},{"Text":"s"}]}"#,
        ),
        (
            CompactionTrigger::Manual,
            r#"{"User":[{"Text":"Manual compaction summary of earlier conversation, for model context only. It is not a new user message."},{"Text":"s"}]}"#,
        ),
        (
            CompactionTrigger::ContextOverflow,
            r#"{"User":[{"Text":"Compaction summary of earlier conversation after the context window overflowed, for model context only. It is not a new user message."},{"Text":"s"}]}"#,
        ),
    ];
    for (trigger, json) in cases {
        assert_eq!(
            serde_json::to_string(&Message::compaction_summary(trigger, "s")).unwrap(),
            json,
            "{trigger:?}"
        );
    }

    let legacy: Message = serde_json::from_str(
        r#"{"User":[{"Text":"Automatic compaction summary of earlier conversation for model context only:\n\nold"}]}"#,
    )
    .unwrap();
    let summary = legacy.as_compaction_summary().unwrap();
    assert_eq!((summary.trigger(), summary.text()), (None, "old"));
}
