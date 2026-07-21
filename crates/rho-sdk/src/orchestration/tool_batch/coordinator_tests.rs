use crate::tool::tool_progress_channel;

use super::*;

#[tokio::test]
async fn cancellation_cleanup_keeps_result_completed_during_trailing_progress() {
    let cancellation = CancellationToken::new();
    let (progress, progress_receiver) = tool_progress_channel(NonZeroUsize::new(2).unwrap());
    assert!(progress.send(ToolProgress::message("first")).await);
    assert!(progress.send(ToolProgress::message("second")).await);
    let (_host_input, host_input_receiver) = mpsc::channel(1);
    let (worker_tx, mut worker_rx) = mpsc::channel(1);
    worker_tx
        .send(WorkerEvent::Progress {
            index: 1,
            progress: ToolProgress::message("channel full"),
        })
        .await
        .unwrap();
    let (completed, completion_observed) = tokio::sync::oneshot::channel();
    let execution: ToolFuture<'static> = Box::pin(async move {
        let _ = completed.send(());
        Ok(ToolOutput::text("completed side effect"))
    });
    let mut forwarded = forward_execution(
        0,
        execution,
        progress_receiver,
        host_input_receiver,
        worker_tx,
        cancellation.clone(),
    );
    std::future::poll_fn(|cx| {
        assert!(forwarded.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    completion_observed.await.unwrap();
    let mut batch = [BatchCall {
        call: ToolCall {
            id: "completed".into(),
            name: "test".into(),
            arguments: serde_json::json!({}),
        },
        id: ToolCallId::from_string("completed").unwrap(),
        state: CallState::Running(forwarded),
        progress: None,
        host_input: None,
        dependencies: Vec::new(),
        queued_at: Instant::now(),
        execution_started: Some(Instant::now()),
        result: None,
    }];

    settle_running_after_cancellation(&cancellation, &mut batch, &mut worker_rx).await;

    let CallState::Finishing(Some(Ok(result))) = &batch[0].state else {
        panic!("completed execution was not preserved");
    };
    assert_eq!(result.content(), "completed side effect");
}
