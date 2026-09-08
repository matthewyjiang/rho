use std::{num::NonZeroUsize, time::Duration};

use pretty_assertions::assert_eq;
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

// Covers: cancellation forwarding one ready completion must not lose the other calls' results.
// Owner: SDK async-job orchestration; spawn/await cancellation tests do not cover batched harvesting.
#[tokio::test]
async fn cancelled_harvest_preserves_all_ready_tool_results() {
    let runtime = Rho::builder()
        .provider(crate::provider::ScriptedProvider::new(
            crate::model::ModelIdentity::new("scripted", "test", "async"),
            [],
        ))
        .build()
        .unwrap();
    let hooks = RunHooks::new(&runtime, crate::SessionId::new(), crate::RunId::new());
    let cancellation = CancellationToken::new();
    let mut jobs = AsyncJobSet::new(NonZeroUsize::MIN);
    let mut expected = Vec::new();
    for id in ["call-a", "call-b"] {
        let call = ToolCall {
            id: id.into(),
            name: "ready".into(),
            arguments: serde_json::json!({}),
        };
        let output = ToolOutput::text(id);
        let result = Ok(output.clone());
        let worker = tokio::spawn(async move { Ok(output) });
        // Await the actual finished state so interrupt preserves the successful output.
        while !worker.is_finished() {
            tokio::task::yield_now().await;
        }
        let (_progress, progress) = tool_progress_channel(NonZeroUsize::MIN);
        let call_id = ToolCallId::from_string(id).unwrap();
        jobs.jobs.insert(
            call_id.clone(),
            AsyncJob {
                call,
                name: "ready".into(),
                cancellation: CancellationToken::new(),
                cancellation_policy: ToolCancellationPolicy::Abort,
                progress,
                worker,
                started: Instant::now(),
                first_capability: FirstCapability::default(),
            },
        );
        jobs.completions_tx
            .send(JobCompletion { call_id, result })
            .unwrap();
        expected.push(Message::ToolResult(ToolResult {
            id: id.into(),
            ok: true,
            content: id.into(),
        }));
    }
    // One terminal event per ready call, with enough capacity for cleanup too.
    let (events, _receiver) = mpsc::channel(expected.len());
    let (_commands_tx, mut commands) = mpsc::channel(1);
    let mut steering = super::super::SteeringQueue::new();
    cancellation.cancel();
    let result = harvest_ready_jobs(&mut RunControl {
        hooks: &hooks,
        cancellation: &cancellation,
        events: &events,
        commands: &mut commands,
        steering: &mut steering,
        async_jobs: &mut jobs,
    })
    .await;
    assert!(matches!(result, Err(Error::Cancelled)));
    let mut history = Vec::new();
    jobs.interrupt(&mut history, &hooks, &events).await;
    assert_eq!(history, expected);
}
