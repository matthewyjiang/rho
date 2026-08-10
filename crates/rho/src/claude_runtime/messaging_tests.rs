use pretty_assertions::assert_eq;

use super::*;

#[test]
fn encode_user_turn_is_single_ndjson_line() {
    let line = encode_user_turn("hello\nworld");
    assert!(line.ends_with('\n'));
    assert_eq!(line.matches('\n').count(), 1);
    let value: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
    assert_eq!(value["type"], "user");
    assert_eq!(value["message"]["role"], "user");
    assert_eq!(value["message"]["content"], "hello\nworld");
}

#[test]
fn frame_parent_message_marks_course_correction() {
    let framed = frame_parent_message("stop editing tests");
    assert!(framed.contains("Message from the parent session"));
    assert!(framed.contains("stop editing tests"));
}

#[tokio::test]
async fn message_channel_delivers_until_receiver_drops() {
    let (handle, mut receiver) = message_channel();
    handle.send("one".into()).await.unwrap();
    assert_eq!(receiver.recv().await.as_deref(), Some("one"));
    drop(receiver);
    assert_eq!(
        handle.send("two".into()).await,
        Err(ClaudeMessageSendError::Closed)
    );
}
