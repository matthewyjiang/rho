use crate::cli_runtime::parent_messages::message_channel;
use pretty_assertions::assert_eq;

use super::*;

#[test]
fn encode_user_turn_is_single_ndjson_line() {
    let line = encode_user_turn("hello\nworld");
    assert!(line.ends_with('\n'));
    assert_eq!(line.matches('\n').count(), 1);
    let value: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"type": "user", "message": {"role": "user", "content": "hello\nworld"}})
    );
}

// Covers: receiving the next line must not overwrite an earlier line's receipt.
// Owner: Claude's adapter pairs provider input with its delivery attachment.
#[tokio::test]
async fn follow_ups_keep_their_own_delivery_receipts() {
    let (handle, inbox) = message_channel();
    let mut source = ClaudeFollowUpSource::new(inbox);
    handle.send("first".into()).unwrap();
    handle.send("second".into()).unwrap();
    let first = source.try_recv().unwrap();
    let second = source.recv().await.unwrap();
    for (follow_up, body) in [(first, "first"), (second, "second")] {
        assert_eq!(
            follow_up.line,
            encode_user_turn(&frame_parent_message(body))
        );
        let Some(StreamEffect::Attachment(AttachmentEvent::Message(card))) = follow_up.written
        else {
            panic!("parent follow-up must carry its delivery receipt");
        };
        assert_eq!(
            *card,
            parent_message_card(
                body.into(),
                NotificationDelivery::Queued,
                "written to Claude stdin; awaiting its next turn".into(),
            )
        );
    }
}
