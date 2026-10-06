use super::*;
use pretty_assertions::assert_eq;

/// Covers: seal stops new accepts while still delivering bodies already queued.
/// Owner: generic parent-message gate used before terminal stdin close.
#[tokio::test]
async fn seal_rejects_new_sends_and_drains_queued() {
    let (handle, mut inbox) = message_channel();
    handle.send("queued".into()).unwrap();
    inbox.seal();
    assert_eq!(
        handle.send("late".into()),
        Err(ParentMessageSendError::Closed)
    );
    assert_eq!(inbox.recv().await.as_deref(), Some("queued"));
    assert_eq!(inbox.recv().await, None);
}

/// Covers: taking a follow-up keeps the port open for later messages, and only
/// an empty queue closes it; a full queue is a visible error, not a hang.
/// Owner: turn-boundary handoff on the parent-message port.
#[test]
fn take_next_or_close_only_closes_when_empty() {
    let (handle, mut inbox) = message_channel();
    for index in 0..PARENT_MESSAGE_QUEUE_CAPACITY {
        handle.send(format!("m{index}")).unwrap();
    }
    assert_eq!(
        handle.send("overflow".into()),
        Err(ParentMessageSendError::QueueFull)
    );

    assert_eq!(inbox.take_next_or_close().as_deref(), Some("m0"));
    handle.send("after take".into()).unwrap();
    let mut rest = Vec::new();
    while let Some(text) = inbox.take_next_or_close() {
        rest.push(text);
    }
    let mut expected: Vec<String> = (1..PARENT_MESSAGE_QUEUE_CAPACITY)
        .map(|index| format!("m{index}"))
        .collect();
    expected.push("after take".into());
    assert_eq!(rest, expected);
    assert_eq!(
        handle.send("late".into()),
        Err(ParentMessageSendError::Closed)
    );
}
