use std::{num::NonZeroUsize, time::Duration};

use tokio::sync::oneshot;

use super::*;

struct NotifyOnDrop(Option<oneshot::Sender<()>>);

impl Drop for NotifyOnDrop {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

// Covers: a complete-policy deadline must abort rather than detach an unfinished worker.
// Owner: SDK async-job orchestration
#[tokio::test]
async fn complete_policy_timeout_aborts_worker() {
    let (started_tx, started_rx) = oneshot::channel();
    let (dropped_tx, mut dropped_rx) = oneshot::channel();
    let worker = tokio::spawn(async move {
        let _notify_on_drop = NotifyOnDrop(Some(dropped_tx));
        let _ = started_tx.send(());
        std::future::pending::<Result<ToolOutput, ToolError>>().await
    });
    started_rx.await.expect("worker started");

    let call = ToolCall {
        id: "call-a".into(),
        name: "slow".into(),
        arguments: serde_json::json!({}),
    };
    let (_progress, progress) = tool_progress_channel(NonZeroUsize::MIN);
    let result = settle_job(AsyncJob {
        call: call.clone(),
        name: call.name.clone(),
        cancellation: CancellationToken::new(),
        cancellation_policy: ToolCancellationPolicy::Complete {
            timeout: Duration::ZERO,
        },
        progress,
        worker,
        started: Instant::now(),
        first_capability: FirstCapability::default(),
    })
    .await;

    assert_eq!(result, interrupted_result(&call));
    dropped_rx
        .try_recv()
        .expect("settle must await the aborted worker");
}
