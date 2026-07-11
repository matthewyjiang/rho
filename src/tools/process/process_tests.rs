use super::*;

async fn eventually(manager: &ProcessManager, id: &str) -> Snapshot {
    let mut cursor = 0;
    loop {
        let snapshot = manager
            .poll(id, Some(cursor), Duration::from_secs(2))
            .await
            .unwrap();
        cursor = snapshot.next_cursor;
        if terminal(snapshot.state) {
            return snapshot;
        }
    }
}

#[tokio::test]
async fn captures_streams_and_incremental_cursors() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let started = manager
        .start(
            "printf out; printf err >&2".into(),
            std::path::Path::new("."),
            None,
        )
        .await
        .unwrap();
    let done = eventually(&manager, &started.process_id).await;
    let all = manager
        .poll(&started.process_id, Some(0), Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(done.state, State::Exited);
    assert!(all
        .chunks
        .iter()
        .any(|c| c.stream == Stream::Stdout && c.text.contains("out")));
    assert!(all
        .chunks
        .iter()
        .any(|c| c.stream == Stream::Stderr && c.text.contains("err")));
    let empty = manager
        .poll(&started.process_id, Some(all.next_cursor), Duration::ZERO)
        .await
        .unwrap();
    assert!(empty.chunks.is_empty());
}

#[tokio::test]
async fn stdin_close_drives_exit_and_subsequent_write_is_typed_error() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let started = manager
        .start("cat".into(), std::path::Path::new("."), None)
        .await
        .unwrap();
    manager
        .write(&started.process_id, "hello\n", true)
        .await
        .unwrap();
    eventually(&manager, &started.process_id).await;
    assert_eq!(
        manager
            .write(&started.process_id, "again", false)
            .await
            .unwrap_err(),
        "process has exited"
    );
}

#[tokio::test]
async fn stale_cursor_and_byte_and_chunk_limits_are_explicit() {
    let manager = ProcessManager::new(ProcessLimits {
        max_bytes: 3,
        max_chunks: 1,
        ..ProcessLimits::default()
    });
    let started = manager
        .start(
            "printf abc; printf def".into(),
            std::path::Path::new("."),
            None,
        )
        .await
        .unwrap();
    eventually(&manager, &started.process_id).await;
    let snapshot = manager
        .poll(&started.process_id, Some(0), Duration::ZERO)
        .await
        .unwrap();
    assert!(snapshot.truncated || snapshot.chunks.iter().map(|c| c.text.len()).sum::<usize>() <= 3);
    assert!(snapshot.chunks.len() <= 1);
}

#[tokio::test]
async fn long_poll_observes_output_without_missed_wakeup() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let started = manager
        .start(
            "sleep 0.05; printf wake".into(),
            std::path::Path::new("."),
            None,
        )
        .await
        .unwrap();
    let polling = {
        let manager = manager.clone();
        let id = started.process_id.clone();
        tokio::spawn(async move {
            manager
                .poll(&id, Some(0), Duration::from_secs(5))
                .await
                .unwrap()
        })
    };
    let snapshot = polling.await.unwrap();
    assert!(
        snapshot.chunks.iter().any(|c| c.text.contains("wake")),
        "{snapshot:?}"
    );
    manager.shutdown().await;
}

#[tokio::test]
async fn enforces_live_limit_and_shutdown_is_terminal() {
    let manager = ProcessManager::new(ProcessLimits {
        max_live: 1,
        ..ProcessLimits::default()
    });
    let first = manager
        .start("cat".into(), std::path::Path::new("."), None)
        .await
        .unwrap();
    assert_eq!(
        manager
            .start("cat".into(), std::path::Path::new("."), None)
            .await
            .unwrap_err(),
        "live process limit reached"
    );
    manager.shutdown().await;
    assert_eq!(
        manager
            .poll(&first.process_id, None, Duration::ZERO)
            .await
            .unwrap()
            .state,
        State::Terminated
    );
}

#[tokio::test]
async fn timeout_and_stop_reach_distinct_terminal_states() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let timeout = manager
        .start(
            "cat".into(),
            std::path::Path::new("."),
            Some(Duration::from_millis(20)),
        )
        .await
        .unwrap();
    assert_eq!(
        eventually(&manager, &timeout.process_id).await.state,
        State::TimedOut
    );
    let stopped = manager
        .start("cat".into(), std::path::Path::new("."), None)
        .await
        .unwrap();
    manager
        .stop(&stopped.process_id, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(
        eventually(&manager, &stopped.process_id).await.state,
        State::Terminated
    );
}
