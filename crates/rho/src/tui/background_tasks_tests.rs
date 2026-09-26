use std::sync::{mpsc, Arc};

use pretty_assertions::assert_eq;

use super::*;

fn update_notice(result: Result<Option<String>, JoinError>) -> TaskOutput {
    SessionOutput::UpdateNotice(result).into()
}

async fn wait_finished(tasks: &BackgroundTasks) {
    while !tasks.has_finished() {
        tokio::task::yield_now().await;
    }
}

// Covers: shutdown must not stall on blocking work that ignores abort, must
// not abort work that has to finish (cache writes), and must still stop and
// wait for ordinary async tasks.
// Owner: background task registry (unit seam; PTY cannot observe task lifetime)
#[tokio::test]
async fn cancel_all_honors_each_task_cancel_policy() {
    let mut tasks = BackgroundTasks::default();

    let async_marker = Arc::new(());
    let captured = Arc::clone(&async_marker);
    tasks.spawn(
        TaskId::UpdateNotice,
        async move {
            let _marker = captured;
            std::future::pending::<Option<String>>().await
        },
        update_notice,
    );

    let (release_blocking, blocked) = mpsc::channel::<()>();
    tasks.spawn_blocking(
        TaskId::SyntaxWarmup,
        move || blocked.recv().unwrap(),
        |_| SessionOutput::SyntaxWarmup.into(),
    );

    let (release_cache, cache_gate) = tokio::sync::oneshot::channel::<()>();
    let (cache_done, cache_written) = tokio::sync::oneshot::channel::<()>();
    let cache_write = tokio::spawn(async move {
        cache_gate.await.unwrap();
        cache_done.send(()).unwrap();
    });
    tasks.track(
        TaskId::CustomModels,
        cache_write,
        OnCancel::RunToCompletion,
        |_| SessionOutput::CustomModels.into(),
    );

    // Returns while the blocking task is still parked on its channel.
    tasks.cancel_all().await;

    assert!(!tasks.has_pending());
    assert_eq!(Arc::strong_count(&async_marker), 1);
    release_cache.send(()).unwrap();
    assert_eq!(cache_written.await, Ok(()));
    release_blocking.send(()).unwrap();
}

// Covers: an output held back from dispatch (metadata while the session is
// busy) is dropped when its feature aborts, so a restarted fetch cannot be
// overwritten by the stale result.
// Owner: background task registry (unit seam)
#[tokio::test]
async fn abort_drops_held_outputs() {
    let mut tasks = BackgroundTasks::default();
    tasks.spawn(
        TaskId::UpdateNotice,
        async { Some("stale".to_string()) },
        update_notice,
    );
    wait_finished(&tasks).await;
    assert!(tasks.take_next_finished(Err::<TaskOutput, _>).is_none());
    assert!(tasks.contains(|id| *id == TaskId::UpdateNotice));

    tasks.abort(|id| *id == TaskId::UpdateNotice);

    assert!(!tasks.has_pending());
    assert!(tasks.take_next_finished(Ok::<_, TaskOutput>).is_none());
}

// Covers: outputs are handed out one at a time, so an apply that aborts
// another feature's task (login cancelling `/limits`) also drops that task's
// result when both finished in the same tick.
// Owner: background task registry (unit seam)
#[tokio::test]
async fn abort_between_takes_drops_outputs_that_finished_together() {
    let mut tasks = BackgroundTasks::default();
    tasks.spawn(TaskId::UpdateNotice, async { None }, update_notice);
    tasks.spawn(TaskId::Changelog, async { None }, update_notice);
    while tasks.running.iter().any(|task| !task.abort.is_finished()) {
        tokio::task::yield_now().await;
    }

    assert!(tasks.take_next_finished(Ok::<_, TaskOutput>).is_some());
    tasks.abort(|id| matches!(id, TaskId::UpdateNotice | TaskId::Changelog));

    assert!(tasks.take_next_finished(Ok::<_, TaskOutput>).is_none());
}
