use std::num::NonZeroUsize;

use pretty_assertions::assert_eq;
use rho_sdk::tool::tool_progress_channel;
use rmcp::model::{NumberOrString, ProgressNotificationParam, ProgressToken};

use super::McpProgressRouter;

fn token(value: i64) -> ProgressToken {
    ProgressToken(NumberOrString::Number(value))
}

// Covers: progress must reach only the subscribed call, must carry counts only
// when the server supplied a usable total, and must stop when the call ends.
// Owner: MCP progress routing.
#[tokio::test]
async fn progress_reaches_the_subscribed_call_only() {
    let router = McpProgressRouter::new();
    let (sender, mut receiver) = tool_progress_channel(NonZeroUsize::new(8).unwrap());
    let subscription = router.subscribe(token(1), sender);

    // Unsubscribed token: dropped rather than misrouted.
    router
        .dispatch(ProgressNotificationParam::new(token(2), 1.0))
        .await;
    router
        .dispatch(
            ProgressNotificationParam::new(token(1), 3.0)
                .with_total(10.0)
                .with_message("indexing"),
        )
        .await;
    // No total: nothing to render a count against.
    router
        .dispatch(ProgressNotificationParam::new(token(1), 4.0))
        .await;

    let counted = receiver.recv().await.unwrap();
    assert_eq!(
        (
            counted.text(),
            counted.completed_units(),
            counted.total_units()
        ),
        ("indexing", Some(3), Some(10))
    );
    let uncounted = receiver.recv().await.unwrap();
    assert_eq!(
        (uncounted.completed_units(), uncounted.total_units()),
        (None, None)
    );

    drop(subscription);
    router
        .dispatch(ProgressNotificationParam::new(token(1), 5.0))
        .await;
    assert!(receiver.recv().await.is_none());
}
