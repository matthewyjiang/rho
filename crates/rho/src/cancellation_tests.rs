use super::RunCancellation;

#[tokio::test]
async fn cancellation_wakes_current_and_future_waiters() {
    let cancellation = RunCancellation::default();
    let waiter = {
        let cancellation = cancellation.clone();
        tokio::spawn(async move { cancellation.cancelled().await })
    };

    cancellation.cancel();

    tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
        .await
        .expect("current waiter was not notified")
        .expect("waiter task failed");
    tokio::time::timeout(std::time::Duration::from_secs(1), cancellation.cancelled())
        .await
        .expect("future waiter did not observe cancellation");
}
