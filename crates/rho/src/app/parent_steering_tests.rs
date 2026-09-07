use super::*;
use pretty_assertions::assert_eq;

// Covers: a sender polling an accepted receipt must not make applied miss its
// one-shot event because Shared reports Pending during another clone's poll.
// Owner: parent steering correlation. Barriers expose the in-poll window that
// the SDK's short oneshot poll cannot reliably reproduce through an E2E run.
#[test]
fn sender_receipt_poll_excludes_application_poll() {
    use std::{future::Future, sync::Barrier, task::Poll};

    let slot = SteeringSlot::default();
    let id = rho_sdk::SteeringId::new();
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let receipt = {
        let entered = Arc::clone(&entered);
        let release = Arc::clone(&release);
        let id = id.clone();
        futures_util::future::poll_fn(move |_| {
            entered.wait();
            release.wait();
            Poll::Ready(Ok(id.clone()))
        })
        .boxed()
        .shared()
    };
    let message = ValidatedMessage::parse("contended").unwrap();
    slot.state
        .lock()
        .unwrap()
        .pending
        .push(Arc::new(PendingMessage {
            receipt: receipt.clone(),
            message: message.clone(),
        }));
    let sender = {
        let slot = slot.clone();
        std::thread::spawn(move || {
            let mut wait = std::pin::pin!(slot.wait_receipt(receipt));
            wait.as_mut().poll(&mut std::task::Context::from_waker(
                futures_util::task::noop_waker_ref(),
            ))
        })
    };
    entered.wait();
    let poll_holds_lock = matches!(
        slot.state.try_lock(),
        Err(std::sync::TryLockError::WouldBlock)
    );
    // Always release the worker, including when testing the broken version.
    release.wait();
    let applied = slot.applied(std::slice::from_ref(&id));
    assert_eq!(sender.join().unwrap(), Poll::Ready(Ok(id)));
    assert!(
        poll_holds_lock,
        "sender must serialize Shared polling with applied"
    );
    assert_eq!(applied, vec![message]);
    assert!(slot.state.lock().unwrap().pending.is_empty());
}

// Covers: application reaches the event pump before the sending task polls its
// receipt, while a newer message is still awaiting acceptance. Waiting for all
// send receipts here would deadlock the event pump behind that newer command.
// Owner: parent steering receipt correlation, not card rendering.
#[test]
fn application_resolves_only_ready_registered_receipts_in_sdk_order() {
    let messages = SteeringSlot::default();
    let first = rho_sdk::SteeringId::new();
    let second = rho_sdk::SteeringId::new();
    let later = rho_sdk::SteeringId::new();
    let mut senders = Vec::new();
    for text in ["second", "later", "first"] {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        senders.push(sender);
        messages
            .state
            .lock()
            .unwrap()
            .pending
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
    let applied = messages.applied(&[first, rho_sdk::SteeringId::new(), second]);
    assert_eq!(
        applied,
        vec![
            ValidatedMessage::parse("first").unwrap(),
            ValidatedMessage::parse("second").unwrap()
        ]
    );
    assert!(messages.applied(&[rho_sdk::SteeringId::new()]).is_empty());
    later_sender.send(Ok(later.clone())).unwrap();
    assert_eq!(
        messages.applied(&[later]),
        vec![ValidatedMessage::parse("later").unwrap()]
    );
    assert!(messages.state.lock().unwrap().pending.is_empty());
}
