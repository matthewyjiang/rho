use crate::{model::ToolResult, tool::tool_progress_channel};

use super::*;

#[tokio::test]
async fn cancellation_cleanup_keeps_result_completed_during_trailing_progress() {
    let cancellation = CancellationToken::new();
    let (progress, progress_receiver) = tool_progress_channel(NonZeroUsize::new(2).unwrap());
    assert!(progress.send(ToolProgress::new("first")).await);
    assert!(progress.send(ToolProgress::new("second")).await);
    let (_host_input, host_input_receiver) = mpsc::channel(1);
    let (worker_tx, mut worker_rx) = mpsc::channel(1);
    worker_tx
        .send(WorkerEvent::Progress {
            index: 1,
            progress: ToolProgress::new("channel full"),
        })
        .await
        .unwrap();
    let (completed, completion_observed) = tokio::sync::oneshot::channel();
    let execution: ToolFuture<'static> = Box::pin(async move {
        let _ = completed.send(());
        Ok(ToolOutput::text("completed side effect").failed())
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
        call: ToolCall::new("completed".into(), "test".into(), serde_json::json!({})),
        id: ToolCallId::from_string("completed").unwrap(),
        state: CallState::Running(forwarded),
        progress: None,
        host_input: None,
        dependencies: Vec::new(),
        queued_at: Instant::now(),
        execution_started: Some(Instant::now()),
        result: None,
        first_capability: None,
    }];

    settle_running_after_cancellation(&cancellation, &mut batch, &mut worker_rx).await;

    let CallState::Finishing(Some(Ok(result))) = &batch[0].state else {
        panic!("completed execution was not preserved");
    };
    assert_eq!(result, &ToolOutput::text("completed side effect").failed());
    let mut history = Vec::new();
    interrupt_batch(&cancellation, &mut batch, &mut history);
    pretty_assertions::assert_eq!(
        history,
        vec![Message::ToolResult(
            ToolResult::failed("completed", "completed side effect").into()
        )]
    );
}

// Covers: a cancel that lands after a running tool last checked for it, and
// before the coordinator does, still lets the tool return its own result,
// such as the `ui` tool saying it stopped waiting on a surface the user saw,
// instead of the call reading as interrupted before it ran.
// Owner: SDK tool batch coordinator.
#[tokio::test]
async fn a_cancel_after_the_tools_last_check_keeps_its_own_result() {
    let cancellation = CancellationToken::new();
    let (_progress, progress_receiver) = tool_progress_channel(NonZeroUsize::new(1).unwrap());
    let (_host_input, host_input_receiver) = mpsc::channel(1);
    let (worker_tx, _worker_rx) = mpsc::channel(1);
    let cancel = cancellation.clone();
    let mut checked = false;
    // Checks for the cancel, which lands just after; sees it on its next poll.
    let execution: ToolFuture<'static> = Box::pin(std::future::poll_fn(move |_| {
        if checked {
            return Poll::Ready(Err(ToolError::new(
                crate::tool::ToolErrorKind::Execution,
                "stopped waiting",
            )));
        }
        checked = true;
        cancel.cancel();
        Poll::Pending
    }));

    let result = forward_execution(
        0,
        execution,
        progress_receiver,
        host_input_receiver,
        worker_tx,
        cancellation,
    )
    .await;

    assert_eq!(result.unwrap_err().message(), "stopped waiting");
}
