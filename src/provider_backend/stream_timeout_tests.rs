use std::time::Duration;

use super::wait_for_stream_activity_for;

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
