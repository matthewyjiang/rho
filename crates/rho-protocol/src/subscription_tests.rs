use pretty_assertions::assert_eq;

use super::*;

fn notice(seq: u64) -> SessionEvent {
    SessionEvent::Notice {
        text: format!("event {seq}"),
    }
}

// Covers: a full backlog reports `Lagged` without waiting and keeps
// reporting it after the subscriber drains, so the client sees no seq gap
// before its detached notification; a dropped subscription reports `Gone`.
// Owner: protocol subscription.
#[tokio::test]
async fn try_publish_reports_lag_and_departure_without_waiting() {
    let (publisher, mut events) = subscription(1, Vec::new());
    assert_eq!(publisher.try_publish(Seq(1), notice(1)), Ok(()));
    assert_eq!(
        publisher.try_publish(Seq(2), notice(2)),
        Err(Published::Lagged)
    );
    assert_eq!(events.next().await, Some((Seq(1), notice(1))));
    assert_eq!(
        publisher.try_publish(Seq(3), notice(3)),
        Err(Published::Lagged)
    );
    drop(publisher);
    assert_eq!(events.next().await, None);
    assert_eq!(events.reason(), Some(DetachReason::Lagged));

    let (publisher, events) = subscription(1, Vec::new());
    drop(events);
    assert_eq!(
        publisher.try_publish(Seq(1), notice(1)),
        Err(Published::Gone)
    );
}

// Covers: a replay longer than the backlog arrives whole and before every
// live event, and does not count against the backlog.
// Owner: protocol subscription.
#[tokio::test]
async fn the_replay_precedes_live_events_without_using_the_backlog() {
    let replay: Vec<_> = (1..=3).map(|seq| (Seq(seq), notice(seq))).collect();
    let (publisher, mut events) = subscription(1, replay.clone());
    assert_eq!(publisher.try_publish(Seq(4), notice(4)), Ok(()));
    drop(publisher);

    let mut received = Vec::new();
    while let Some(event) = events.next().await {
        received.push(event);
    }
    let mut expected = replay;
    expected.push((Seq(4), notice(4)));
    assert_eq!(received, expected);
}
