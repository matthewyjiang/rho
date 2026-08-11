use pretty_assertions::assert_eq;
use tokio::sync::oneshot;

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
    let (handle, mut inbox) = message_channel();
    handle.send("one".into()).await.unwrap();
    assert_eq!(inbox.recv().await.as_deref(), Some("one"));
    drop(inbox);
    assert_eq!(
        handle.send("two".into()).await,
        Err(ClaudeMessageSendError::Closed)
    );
}

/// Covers: seal stops new accepts while still delivering bodies already queued.
/// Owner: Claude parent-message gate used before terminal stdin close.
#[tokio::test]
async fn seal_rejects_new_sends_and_drains_queued() {
    let (handle, mut inbox) = message_channel();
    handle.send("queued".into()).await.unwrap();
    inbox.seal();
    assert_eq!(
        handle.send("late".into()).await,
        Err(ClaudeMessageSendError::Closed)
    );
    assert_eq!(inbox.recv().await.as_deref(), Some("queued"));
    assert_eq!(inbox.recv().await, None);
}

/// Covers: a send that cloned the sender before seal still delivers, and seal
/// waits for that clone to drop before disconnecting the receiver.
/// Owner: terminal close/send interleaving on the Claude message port.
#[tokio::test(flavor = "current_thread")]
async fn seal_preserves_in_flight_sender_clone() {
    let (handle, mut inbox) = message_channel();
    let (cloned_ready_tx, cloned_ready_rx) = oneshot::channel::<()>();
    let (release_tx, release_rx) = oneshot::channel::<()>();

    let sender = handle
        .clone_sender_for_test()
        .expect("port open for in-flight clone");
    let send_task = tokio::spawn(async move {
        let _ = cloned_ready_tx.send(());
        release_rx.await.expect("release in-flight send");
        sender
            .send("in-flight".into())
            .await
            .expect("in-flight clone still delivers after seal");
    });

    cloned_ready_rx.await.expect("sender cloned");
    inbox.seal();
    assert_eq!(
        handle.send("after-seal".into()).await,
        Err(ClaudeMessageSendError::Closed)
    );

    let drain = tokio::spawn(async move {
        let first = inbox.recv().await;
        let second = inbox.recv().await;
        (first, second)
    });

    // No body yet: the in-flight task still holds the only sender clone.
    tokio::task::yield_now().await;
    release_tx.send(()).expect("release send");
    send_task.await.expect("in-flight send task");
    let (first, second) = drain.await.expect("drain task");
    assert_eq!(first.as_deref(), Some("in-flight"));
    assert_eq!(second, None);
}
