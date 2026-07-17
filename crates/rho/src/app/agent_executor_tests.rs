use std::sync::Arc;

use super::*;

#[tokio::test]
async fn cancellation_interrupts_concurrency_queue() {
    let permits = Arc::new(tokio::sync::Semaphore::new(0));
    let cancellation = RunCancellation::new();
    let queued = tokio::spawn({
        let permits = Arc::clone(&permits);
        let cancellation = cancellation.clone();
        async move { acquire_permit_or_cancel(permits, &cancellation).await }
    });

    cancellation.cancel();

    let permit = tokio::time::timeout(std::time::Duration::from_secs(1), queued)
        .await
        .expect("queued acquisition should observe cancellation")
        .unwrap()
        .unwrap();
    assert!(permit.is_none());
}

#[tokio::test]
async fn cancellation_wins_when_a_permit_is_already_available() {
    let permits = Arc::new(tokio::sync::Semaphore::new(1));
    let cancellation = RunCancellation::new();
    cancellation.cancel();

    let permit = acquire_permit_or_cancel(permits, &cancellation)
        .await
        .unwrap();

    assert!(permit.is_none());
}
