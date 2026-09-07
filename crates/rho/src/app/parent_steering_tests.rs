use super::*;
use pretty_assertions::assert_eq;

// Covers: application reaches the event pump before the sending task polls its
// receipt, while a newer message is still awaiting acceptance. Waiting for all
// send receipts here would deadlock the event pump behind that newer command.
// Owner: parent steering receipt correlation, not card rendering.
#[tokio::test]
async fn application_resolves_only_its_receipts_in_sdk_order() {
    let messages = ParentSteering::default();
    let first = rho_sdk::SteeringId::new();
    let second = rho_sdk::SteeringId::new();
    let later = rho_sdk::SteeringId::new();
    let mut senders = Vec::new();
    for text in ["second", "later", "first"] {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        senders.push(sender);
        messages
            .pending
            .lock()
            .unwrap()
            .push(Arc::new(PendingMessage {
                receipt: async move { receiver.await.unwrap() }.boxed().shared(),
                message: ValidatedMessage::parse(text).unwrap(),
            }));
    }
    let mut senders = senders.into_iter();
    senders.next().unwrap().send(Ok(second.clone())).unwrap();
    let later_sender = senders.next().unwrap();
    senders.next().unwrap().send(Ok(first.clone())).unwrap();
    // Both acknowledgements are ready, but no sending task has polled them.
    let applied = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        messages.applied(&[first, second]),
    )
    .await
    .expect("applied input must not wait for a later send");
    assert_eq!(
        applied,
        vec![
            ValidatedMessage::parse("first").unwrap(),
            ValidatedMessage::parse("second").unwrap()
        ]
    );
    later_sender.send(Ok(later.clone())).unwrap();
    assert_eq!(
        messages.applied(&[later]).await,
        vec![ValidatedMessage::parse("later").unwrap()]
    );
    assert!(messages.pending.lock().unwrap().is_empty());
}
