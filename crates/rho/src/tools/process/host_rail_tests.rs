use super::*;
use std::time::Duration;

/// Waits for exit through the host view, which, unlike `poll`, leaves the
/// result undelivered.
async fn exited_undelivered(manager: &ProcessManager, id: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        if terminal(manager.host_view(id).unwrap().snapshot.state) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "process did not become terminal"
        );
        tokio::task::yield_now().await;
    }
}

fn rail_ids(manager: &ProcessManager) -> Vec<String> {
    manager
        .live_summaries()
        .into_iter()
        .map(|summary| summary.process_id)
        .collect()
}

// Covers: host peek must not mark a terminal process observed.
// Owner: process manager host view
#[tokio::test]
async fn host_view_does_not_mark_terminal_observed() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let started = manager
        .start(SUCCESS_COMMAND.into(), std::path::Path::new("."), None)
        .await
        .unwrap();
    exited_undelivered(&manager, &started.process_id).await;
    assert!(manager.has_pending_notification());
    let view = manager.host_view(&started.process_id).unwrap();
    pretty_assertions::assert_eq!(view.snapshot.state, State::Exited);
    pretty_assertions::assert_eq!(view.snapshot.exit_code, Some(0));
    pretty_assertions::assert_eq!(view.quiet_seconds, None);
    assert!(manager.has_pending_notification());
}

// Covers: the host rail must see live jobs.
// Owner: process manager
#[tokio::test]
async fn live_summaries_lists_running() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let started = manager
        .start(LONG_RUNNING_COMMAND.into(), std::path::Path::new("."), None)
        .await
        .unwrap();

    let summaries = manager.live_summaries();
    assert_eq!(summaries.len(), 1);
    pretty_assertions::assert_eq!(
        (
            summaries[0].process_id.as_str(),
            summaries[0].command.as_str(),
            terminal(summaries[0].state),
            summaries[0].quiet_seconds,
            summaries[0].exit_code
        ),
        (
            started.process_id.as_str(),
            LONG_RUNNING_COMMAND,
            false,
            None,
            None
        )
    );

    manager
        .stop(&started.process_id, Duration::ZERO)
        .await
        .unwrap();
    eventually(&manager, &started.process_id).await;
}

// Covers: overflow rows must keep the oldest live process, not the newest.
// Owner: process manager
#[tokio::test]
async fn live_summaries_orders_oldest_first() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let first = manager
        .start(LONG_RUNNING_COMMAND.into(), std::path::Path::new("."), None)
        .await
        .unwrap();
    let second = manager
        .start(LONG_RUNNING_COMMAND.into(), std::path::Path::new("."), None)
        .await
        .unwrap();

    let ids = manager
        .live_summaries()
        .into_iter()
        .map(|summary| summary.process_id)
        .collect::<Vec<_>>();
    pretty_assertions::assert_eq!(
        ids,
        vec![first.process_id.clone(), second.process_id.clone()]
    );

    manager
        .stop(&first.process_id, Duration::ZERO)
        .await
        .unwrap();
    manager
        .stop(&second.process_id, Duration::ZERO)
        .await
        .unwrap();
    eventually(&manager, &first.process_id).await;
    eventually(&manager, &second.process_id).await;
}

// Covers: a finished, undelivered process stays on the rail with a frozen
// elapsed duration and its exit code, not disappear or keep ticking.
// Owner: process manager
#[tokio::test]
async fn live_summaries_keeps_terminal_rows_with_frozen_elapsed_and_exit_code() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let started = manager
        .start(SUCCESS_COMMAND.into(), std::path::Path::new("."), None)
        .await
        .unwrap();
    exited_undelivered(&manager, &started.process_id).await;

    let summaries = manager.live_summaries();
    assert_eq!(summaries.len(), 1);
    pretty_assertions::assert_eq!(
        (
            summaries[0].process_id.as_str(),
            summaries[0].command.as_str(),
            summaries[0].state,
            summaries[0].quiet_seconds,
            summaries[0].exit_code
        ),
        (
            started.process_id.as_str(),
            SUCCESS_COMMAND,
            State::Exited,
            None,
            Some(0)
        )
    );
    // `true` finishes immediately; frozen elapsed is completed-started, not wall
    // time since start, so it stays at a truncated 0s (or 1s if the spawn
    // straddled a second).
    assert!(
        summaries[0].elapsed_seconds <= 1,
        "elapsed_seconds={}",
        summaries[0].elapsed_seconds
    );
}

// Covers: a finished row leaves the rail when its result is delivered, either
// as a notification or through an agent poll, not on a timer; a rolled-back
// notification puts it back.
// Owner: process manager host list
#[tokio::test]
async fn live_summaries_keep_terminal_rows_until_delivered() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let notified = manager
        .start(SUCCESS_COMMAND.into(), std::path::Path::new("."), None)
        .await
        .unwrap();
    let polled = manager
        .start(SUCCESS_COMMAND.into(), std::path::Path::new("."), None)
        .await
        .unwrap();
    exited_undelivered(&manager, &notified.process_id).await;
    exited_undelivered(&manager, &polled.process_id).await;
    pretty_assertions::assert_eq!(
        rail_ids(&manager),
        vec![notified.process_id.clone(), polled.process_id.clone()]
    );

    manager
        .poll(&polled.process_id, None, Duration::ZERO)
        .await
        .unwrap();
    pretty_assertions::assert_eq!(rail_ids(&manager), vec![notified.process_id.clone()]);

    let delivered = manager.take_notifications();
    pretty_assertions::assert_eq!(rail_ids(&manager), Vec::<String>::new());

    manager.restore_notifications(&delivered);
    pretty_assertions::assert_eq!(rail_ids(&manager), vec![notified.process_id]);
}

// Covers: the rail reports seconds-since-output for live jobs, and None when
// nothing has been written yet.
// Owner: process manager
#[tokio::test]
async fn live_summaries_reports_quiet_seconds_only_after_output() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let silent = manager
        .start(LONG_RUNNING_COMMAND.into(), std::path::Path::new("."), None)
        .await
        .unwrap();
    let noisy = manager
        .start(
            OUTPUT_THEN_SLEEP_COMMAND.into(),
            std::path::Path::new("."),
            None,
        )
        .await
        .unwrap();

    loop {
        let snapshot = manager
            .poll(&noisy.process_id, Some(0), Duration::from_secs(2))
            .await
            .unwrap();
        if snapshot
            .chunks
            .iter()
            .any(|chunk| chunk.text.contains("hello"))
        {
            break;
        }
        assert!(
            !terminal(snapshot.state),
            "noisy process exited before producing output"
        );
    }

    let summaries = manager.live_summaries();
    pretty_assertions::assert_eq!(
        summaries
            .iter()
            .map(|summary| summary.process_id.as_str())
            .collect::<Vec<_>>(),
        vec![silent.process_id.as_str(), noisy.process_id.as_str()]
    );
    pretty_assertions::assert_eq!(summaries[0].quiet_seconds, None);
    pretty_assertions::assert_eq!(summaries[0].exit_code, None);
    match summaries[1].quiet_seconds {
        Some(seconds) => assert!(seconds <= 2, "quiet_seconds={seconds}"),
        None => panic!("expected quiet_seconds after output"),
    }
    pretty_assertions::assert_eq!(summaries[1].exit_code, None);

    manager
        .stop(&silent.process_id, Duration::ZERO)
        .await
        .unwrap();
    manager
        .stop(&noisy.process_id, Duration::ZERO)
        .await
        .unwrap();
    eventually(&manager, &silent.process_id).await;
    eventually(&manager, &noisy.process_id).await;
}
