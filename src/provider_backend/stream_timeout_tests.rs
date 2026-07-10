use std::time::Duration;

use super::{wait_for_stream_activity_for, StreamIdleDeadline};

#[tokio::test]
async fn reports_when_a_stream_stops_delivering_activity() {
    let error =
        wait_for_stream_activity_for(std::future::pending::<()>(), Duration::from_millis(1))
            .await
            .unwrap_err();

    assert_eq!(
        error.to_string(),
        "provider stream received no data for 1ms; the connection may be stale"
    );
}

#[tokio::test]
async fn returns_activity_before_the_deadline() {
    let value = wait_for_stream_activity_for(async { 42 }, Duration::from_secs(1))
        .await
        .unwrap();

    assert_eq!(value, 42);
}

#[tokio::test]
async fn keep_alives_do_not_reset_the_idle_deadline() {
    let deadline = StreamIdleDeadline::with_timeout(Duration::from_millis(1));
    tokio::time::sleep(Duration::from_millis(5)).await;

    let error = deadline.wait_for(async {}).await.unwrap_err();

    assert_eq!(
        error.to_string(),
        "provider stream received no data for 1ms; the connection may be stale"
    );
}

#[tokio::test]
async fn meaningful_activity_resets_the_idle_deadline() {
    let mut deadline = StreamIdleDeadline::with_timeout(Duration::from_millis(10));
    tokio::time::sleep(Duration::from_millis(5)).await;
    deadline.record_activity();

    deadline.wait_for(async {}).await.unwrap();
}
