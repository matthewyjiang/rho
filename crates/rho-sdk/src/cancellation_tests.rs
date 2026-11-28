use std::future::Future;

use super::CancellationToken;

#[tokio::test]
async fn cancellation_is_shared_and_idempotent() {
    let cancellation = CancellationToken::new();
    let observer = cancellation.clone();

    assert!(!observer.is_cancelled());
    cancellation.cancel();
    cancellation.cancel();
    observer.cancelled().await;

    assert!(observer.is_cancelled());
}

// Covers: a wait parked before the cancel resolves on the next poll even
// when the task's cooperative budget is spent, as a busy turn's task's can
// be. Otherwise a tool racing its own cancellation against the host's loses:
// the host, checking a fresh wait, drops the tool before it can return its
// own result.
// Owner: SDK cancellation.
#[tokio::test]
async fn a_parked_wait_sees_cancellation_without_budget() {
    let cancellation = CancellationToken::new();
    let wait = cancellation.cancelled();
    tokio::pin!(wait);
    std::future::poll_fn(|cx| {
        assert!(wait.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    cancellation.cancel();

    std::future::poll_fn(|cx| {
        // Tokio's budget is 128 units a poll; more than that spends it.
        for _ in 0..1024 {
            if !tokio::task::coop::has_budget_remaining() {
                break;
            }
            let spend = std::pin::pin!(tokio::task::coop::consume_budget());
            let _ = spend.poll(cx);
        }
        assert!(
            !tokio::task::coop::has_budget_remaining(),
            "the budget is not spent, so the test no longer covers a parked wait without it"
        );
        assert!(wait.as_mut().poll(cx).is_ready());
        std::task::Poll::Ready(())
    })
    .await;
}
