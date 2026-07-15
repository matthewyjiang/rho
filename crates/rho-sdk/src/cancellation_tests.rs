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

#[tokio::test]
async fn dropping_a_clone_does_not_cancel_remaining_owners() {
    let cancellation = CancellationToken::new();
    let remaining = cancellation.clone();

    drop(cancellation);

    assert!(!remaining.is_cancelled());
}
