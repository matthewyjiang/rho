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
