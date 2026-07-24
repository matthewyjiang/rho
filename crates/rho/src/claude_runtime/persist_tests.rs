use std::{
    sync::{atomic::AtomicUsize, Arc, Mutex},
    time::Duration,
};

use tokio::sync::watch;

use crate::{
    subagent::{self, RunState, RunStatus},
    tui::AttachmentEvent,
};

use super::*;
use crate::claude_runtime::stream::{
    StatusPatch, StreamEffect, TerminalClassification, TerminalResult,
};

fn identity() -> crate::claude_runtime::persist::ClaudeRunIdentity {
    crate::claude_runtime::persist::ClaudeRunIdentity {
        agent_id: "claude-planner".into(),
        agent_fingerprint: "fp".into(),
        model: Some("opus".into()),
    }
}

fn success_terminal() -> TerminalResult {
    TerminalResult {
        classification: TerminalClassification::Success {
            subtype: "success".into(),
        },
        ok: true,
        result_text: Some("done".into()),
        error: None,
        session_id: Some("sess".into()),
        num_turns: Some(1),
        usage: None,
        context: None,
        total_cost_usd: None,
        permission_denials: Vec::new(),
        stop_reason: None,
        subtype: Some("success".into()),
        is_error: Some(false),
    }
}

fn read_attachment_events(output: &std::path::Path) -> Vec<AttachmentEvent> {
    let path = output.with_file_name(crate::subagent::ATTACHMENT_FILE_NAME);
    let body = std::fs::read_to_string(path).unwrap_or_default();
    body.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("attachment event json"))
        .collect()
}

#[tokio::test]
async fn stalled_writer_does_not_block_high_volume_effects() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let stall = WriterStall::new_stalled();
    let log = Arc::new(Mutex::new(Vec::new()));
    let hooks = PersistHooks {
        stall: Some(Arc::clone(&stall)),
        log: Arc::clone(&log),
        ..PersistHooks::default()
    };
    let mut sink =
        StatusSink::new_with_hooks(output.clone(), &identity(), "prompt", None, hooks).unwrap();

    let started = std::time::Instant::now();
    for index in 0..2_000 {
        sink.apply_effect(StreamEffect::Attachment(
            AttachmentEvent::AssistantTextDelta(format!("chunk {index}")),
        ))
        .unwrap();
        sink.apply_effect(StreamEffect::Status(StatusPatch {
            state: Some(RunState::Running),
            last_activity: Some(format!("chunk {index}")),
            append_text: Some(format!("chunk {index}")),
            ..StatusPatch::default()
        }))
        .unwrap();
    }
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "stream enqueue stalled on writer: {:?}",
        started.elapsed()
    );
    // Attachment may degrade under saturation; that is intentional.
    let degraded = sink.status.attachment_error.is_some();

    stall.release();
    sink.finalize_success_from_stream(&success_terminal()).await;
    sink.shutdown().await;

    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Ok);
    if degraded {
        assert!(status.attachment_error.is_some());
    } else {
        let events = read_attachment_events(&output);
        assert!(events
            .iter()
            .any(|event| matches!(event, AttachmentEvent::Completed)));
    }
    let log = log.lock().unwrap().clone();
    assert!(
        log.iter()
            .any(|entry| matches!(entry, PersistLogEntry::BarrierDone)),
        "barrier must run: {log:?}"
    );
}

#[tokio::test]
async fn terminal_barrier_preserves_order_and_single_completed() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let log = Arc::new(Mutex::new(Vec::new()));
    let hooks = PersistHooks {
        log: Arc::clone(&log),
        ..PersistHooks::default()
    };
    let mut sink =
        StatusSink::new_with_hooks(output.clone(), &identity(), "prompt", None, hooks).unwrap();

    sink.mark_running().unwrap();
    sink.apply_effect(StreamEffect::Attachment(
        AttachmentEvent::AssistantTextDelta("hello".into()),
    ))
    .unwrap();
    sink.apply_effect(StreamEffect::Status(StatusPatch {
        state: Some(RunState::Running),
        last_activity: Some("assistant text".into()),
        append_text: Some("hello".into()),
        ..StatusPatch::default()
    }))
    .unwrap();
    sink.finalize_success_from_stream(&success_terminal()).await;
    sink.shutdown().await;

    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Ok);
    assert_eq!(status.result.as_deref(), Some("done"));

    let events = read_attachment_events(&output);
    let terminals: Vec<_> = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                AttachmentEvent::Completed
                    | AttachmentEvent::Failed(_)
                    | AttachmentEvent::Cancelled
            )
        })
        .collect();
    assert_eq!(terminals.len(), 1);
    assert!(matches!(terminals[0], AttachmentEvent::Completed));
    assert!(events.iter().any(
        |event| matches!(event, AttachmentEvent::AssistantTextDelta(text) if text == "hello")
    ));

    let log = log.lock().unwrap().clone();
    let completed_pos = log.iter().position(|entry| {
        matches!(
            entry,
            PersistLogEntry::Attachment(AttachmentKind::Completed)
        )
    });
    let barrier_pos = log
        .iter()
        .position(|entry| matches!(entry, PersistLogEntry::BarrierDone));
    assert!(completed_pos.is_some());
    assert!(barrier_pos.is_some());
    assert!(completed_pos.unwrap() < barrier_pos.unwrap());
}

#[tokio::test]
async fn early_status_failure_is_reported_after_later_success() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let hooks = PersistHooks {
        fail_status_writes: Arc::new(AtomicUsize::new(1)),
        ..PersistHooks::default()
    };
    let mut sink =
        StatusSink::new_with_hooks(output.clone(), &identity(), "prompt", None, hooks).unwrap();

    // First forced running write is injected to fail on the worker thread.
    let _ = sink.mark_running();
    // Give the worker time to record sticky status-write feedback.
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Later stream status still enqueues; sticky failure must dominate terminal.
    sink.apply_effect(StreamEffect::Status(StatusPatch {
        state: Some(RunState::Running),
        last_activity: Some("still going".into()),
        ..StatusPatch::default()
    }))
    .ok();

    sink.finalize_success_from_stream(&success_terminal()).await;
    sink.shutdown().await;

    let status = subagent::read_status(&output).expect("status");
    assert_eq!(
        status.state,
        RunState::Error,
        "sticky status failure must not end Completed/Ok"
    );
    assert!(
        status.error.as_deref().is_some_and(
            |text| text.contains("status persistence failed") || text.contains("injected")
        ),
        "unexpected error: {:?}",
        status.error
    );

    let events = read_attachment_events(&output);
    let terminals: Vec<_> = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                AttachmentEvent::Completed
                    | AttachmentEvent::Failed(_)
                    | AttachmentEvent::Cancelled
            )
        })
        .collect();
    assert_eq!(
        terminals.len(),
        1,
        "exactly one terminal attachment: {events:?}"
    );
    assert!(
        matches!(terminals[0], AttachmentEvent::Failed(_)),
        "sticky status failure must emit Failed, not Completed: {terminals:?}"
    );
}

/// Sticky status failure discovered only while draining the barrier must still
/// demote Completed -> Failed so the journal and result.json agree on Error.
#[tokio::test]
async fn barrier_drain_status_failure_demotes_completed_to_failed() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let stall = WriterStall::new_stalled();
    let log = Arc::new(Mutex::new(Vec::new()));
    let hooks = PersistHooks {
        stall: Some(Arc::clone(&stall)),
        fail_status_writes: Arc::new(AtomicUsize::new(1)),
        log: Arc::clone(&log),
        ..PersistHooks::default()
    };
    let mut sink =
        StatusSink::new_with_hooks(output.clone(), &identity(), "prompt", None, hooks).unwrap();

    // Queue a running write that will fail only after the stall is released,
    // i.e. during barrier drain when the sink has already preselected Completed.
    sink.mark_running().ok();
    // Barrier is issued while the worker is still stalled, so StatusSink cannot
    // yet see sticky feedback and still enqueues Completed.
    let finalize = tokio::spawn(async move {
        sink.finalize_success_from_stream(&success_terminal()).await;
        sink.shutdown().await;
    });

    // Let the barrier enqueue behind the stalled running write.
    tokio::time::sleep(Duration::from_millis(20)).await;
    stall.release();
    tokio::time::timeout(Duration::from_secs(2), finalize)
        .await
        .expect("barrier should finish")
        .unwrap();

    let status = subagent::read_status(&output).expect("status");
    assert_eq!(
        status.state,
        RunState::Error,
        "barrier-drain sticky failure must end Error"
    );
    assert!(
        status.error.as_deref().is_some_and(
            |text| text.contains("status persistence failed") || text.contains("injected")
        ),
        "unexpected error: {:?}",
        status.error
    );

    let events = read_attachment_events(&output);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AttachmentEvent::Completed)),
        "journal must not contain Completed after sticky status failure: {events:?}"
    );
    let terminals: Vec<_> = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                AttachmentEvent::Completed
                    | AttachmentEvent::Failed(_)
                    | AttachmentEvent::Cancelled
            )
        })
        .collect();
    assert_eq!(terminals.len(), 1, "exactly one terminal: {events:?}");
    assert!(
        matches!(terminals[0], AttachmentEvent::Failed(_)),
        "expected Failed terminal: {terminals:?}"
    );

    let log = log.lock().unwrap().clone();
    assert!(
        log.iter()
            .any(|entry| matches!(entry, PersistLogEntry::BarrierDone)),
        "barrier must complete: {log:?}"
    );
    assert!(
        !log.iter().any(|entry| {
            matches!(
                entry,
                PersistLogEntry::Attachment(AttachmentKind::Completed)
            )
        }),
        "worker must not write Completed after sticky failure: {log:?}"
    );
}

#[tokio::test]
async fn attachment_failure_degrades_without_killing_run() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let hooks = PersistHooks {
        fail_attachment_writes: Arc::new(AtomicUsize::new(1)),
        ..PersistHooks::default()
    };
    let mut sink =
        StatusSink::new_with_hooks(output.clone(), &identity(), "prompt", None, hooks).unwrap();

    sink.mark_running().unwrap();
    sink.apply_effect(StreamEffect::Attachment(
        AttachmentEvent::AssistantTextDelta("will fail".into()),
    ))
    .unwrap();
    // Let worker record attachment_error.
    tokio::time::sleep(Duration::from_millis(20)).await;
    sink.apply_effect(StreamEffect::Attachment(
        AttachmentEvent::AssistantTextDelta("ignored after degrade".into()),
    ))
    .unwrap();
    sink.finalize_success_from_stream(&success_terminal()).await;
    sink.shutdown().await;

    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Ok);
    assert!(
        status
            .attachment_error
            .as_deref()
            .is_some_and(|error| error.contains("could not record attach output")),
        "expected attachment_error, got {:?}",
        status.attachment_error
    );
    // Run succeeded; no second terminal attachment required after degrade.
}

#[tokio::test]
async fn stalled_writer_cancel_path_stays_responsive() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let stall = WriterStall::new_stalled();
    let hooks = PersistHooks {
        stall: Some(Arc::clone(&stall)),
        ..PersistHooks::default()
    };
    let mut sink =
        StatusSink::new_with_hooks(output.clone(), &identity(), "prompt", None, hooks).unwrap();

    for index in 0..500 {
        let _ = sink.apply_effect(StreamEffect::Attachment(
            AttachmentEvent::AssistantTextDelta(format!("x{index}")),
        ));
    }

    let stop = tokio::spawn(async move {
        sink.stop("cancelled").await;
        sink.shutdown().await;
    });
    // Release shortly after stop is requested so barrier can finish.
    tokio::time::sleep(Duration::from_millis(10)).await;
    stall.release();
    tokio::time::timeout(Duration::from_secs(2), stop)
        .await
        .expect("stop should finish")
        .unwrap();

    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Stopped);
}

#[tokio::test]
async fn abort_detached_does_not_block_and_preserves_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let stall = WriterStall::new_stalled();
    let hooks = PersistHooks {
        stall: Some(Arc::clone(&stall)),
        ..PersistHooks::default()
    };
    let mut sink =
        StatusSink::new_with_hooks(output.clone(), &identity(), "prompt", None, hooks).unwrap();
    sink.mark_running().ok();
    sink.status.state = RunState::Error;
    sink.status.error = Some("session panicked".into());
    sink.status.last_activity = Some("failed".into());

    let started = std::time::Instant::now();
    sink.abort_detached();
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "abort must not wait on stalled worker: {:?}",
        started.elapsed()
    );

    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Error);
    assert_eq!(status.error.as_deref(), Some("session panicked"));

    // Release stall so the detached join helper can finish; Drop already ran abort.
    stall.release();
    tokio::time::sleep(Duration::from_millis(30)).await;
    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Error);
}

#[tokio::test]
async fn drop_aborts_without_blocking_tokio_worker() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let stall = WriterStall::new_stalled();
    let hooks = PersistHooks {
        stall: Some(Arc::clone(&stall)),
        ..PersistHooks::default()
    };
    let sink =
        StatusSink::new_with_hooks(output.clone(), &identity(), "prompt", None, hooks).unwrap();

    let started = std::time::Instant::now();
    drop(sink);
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "Drop must not park the runtime on the persist worker: {:?}",
        started.elapsed()
    );
    stall.release();
    tokio::time::sleep(Duration::from_millis(30)).await;
}

#[tokio::test]
async fn panic_fallback_like_race_keeps_terminal_error() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let stall = WriterStall::new_stalled();
    let log = Arc::new(Mutex::new(Vec::new()));
    let hooks = PersistHooks {
        stall: Some(Arc::clone(&stall)),
        log: Arc::clone(&log),
        ..PersistHooks::default()
    };
    let mut sink =
        StatusSink::new_with_hooks(output.clone(), &identity(), "prompt", None, hooks).unwrap();

    // Queue a nonterminal running update that the stalled worker has not written.
    sink.mark_running().ok();
    sink.apply_effect(StreamEffect::Status(StatusPatch {
        state: Some(RunState::Running),
        last_activity: Some("queued running".into()),
        ..StatusPatch::default()
    }))
    .ok();

    // Executor-style panic fallback writes Error while the detached worker still
    // holds a queued Running snapshot.
    let fallback = crate::subagent::RunStatus {
        state: RunState::Error,
        agent_id: Some("claude-planner".into()),
        agent_fingerprint: Some("fp".into()),
        provider: Some("claude-code".into()),
        model: Some("opus".into()),
        error: Some("delegated agent task panicked".into()),
        last_activity: Some("failed".into()),
        ..crate::subagent::RunStatus::default()
    };
    crate::subagent::write_status(&output, &fallback).unwrap();

    // Session future abort disconnects the worker without joining on this task.
    sink.abort_detached();

    // Worker resumes and attempts the queued Running write; monotonic guard wins.
    stall.release();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Error);
    assert_eq!(
        status.error.as_deref(),
        Some("delegated agent task panicked")
    );
}

/// Collect every distinct watch publish while `body` runs.
async fn collect_watch_states<F, T>(
    mut rx: watch::Receiver<RunStatus>,
    body: F,
) -> (T, Vec<RunState>)
where
    F: std::future::Future<Output = T>,
{
    let observed = Arc::new(Mutex::new(vec![rx.borrow().state]));
    let observed_task = Arc::clone(&observed);
    let collector = tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            observed_task.lock().unwrap().push(rx.borrow().state);
        }
    });
    let result = body.await;
    // Drop senders in body should end the collector; give it a moment.
    let _ = tokio::time::timeout(Duration::from_millis(200), collector).await;
    let states = observed.lock().unwrap().clone();
    (result, states)
}

fn count_terminal(states: &[RunState]) -> usize {
    states.iter().filter(|state| state.is_terminal()).count()
}

/// Sticky failure discovered during barrier drain must never publish Ok on the
/// watch channel. Disk, journal, sink memory, and watch must all end Error/Failed.
#[tokio::test]
async fn barrier_drain_sticky_failure_never_publishes_ok_on_watch() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let stall = WriterStall::new_stalled();
    let log = Arc::new(Mutex::new(Vec::new()));
    let hooks = PersistHooks {
        stall: Some(Arc::clone(&stall)),
        fail_status_writes: Arc::new(AtomicUsize::new(1)),
        log: Arc::clone(&log),
        ..PersistHooks::default()
    };
    let (status_tx, status_rx) = watch::channel(RunStatus::default());
    let mut sink = StatusSink::new_with_hooks(
        output.clone(),
        &identity(),
        "prompt",
        Some(status_tx),
        hooks,
    )
    .unwrap();

    sink.mark_running().ok();
    // Barrier is issued while the worker is still stalled, so StatusSink cannot
    // yet see sticky feedback and still preselects Completed/Ok locally.
    let finalize = tokio::spawn(async move {
        sink.finalize_success_from_stream(&success_terminal()).await;
        let memory = sink.status.clone();
        sink.shutdown().await;
        memory
    });

    // While the writer is stalled the watch channel must stay nonterminal.
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(
        !status_rx.borrow().state.is_terminal(),
        "terminal watch update must wait for barrier: {:?}",
        status_rx.borrow().state
    );

    let (memory, states) = collect_watch_states(status_rx, async {
        stall.release();
        tokio::time::timeout(Duration::from_secs(2), finalize)
            .await
            .expect("barrier should finish")
            .unwrap()
    })
    .await;

    assert!(
        !states.iter().any(|state| *state == RunState::Ok),
        "watch must never observe Ok when sticky failure demotes: {states:?}"
    );
    assert_eq!(
        count_terminal(&states),
        1,
        "exactly one terminal watch publish: {states:?}"
    );
    assert_eq!(states.last().copied(), Some(RunState::Error));
    assert_eq!(memory.state, RunState::Error);

    let disk = subagent::read_status(&output).expect("status");
    assert_eq!(disk.state, RunState::Error);
    assert_eq!(disk.state, memory.state);
    assert!(
        disk.error.as_deref().is_some_and(
            |text| text.contains("status persistence failed") || text.contains("injected")
        ),
        "unexpected error: {:?}",
        disk.error
    );

    let events = read_attachment_events(&output);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AttachmentEvent::Completed)),
        "journal must not contain Completed: {events:?}"
    );
    let terminals: Vec<_> = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                AttachmentEvent::Completed
                    | AttachmentEvent::Failed(_)
                    | AttachmentEvent::Cancelled
            )
        })
        .collect();
    assert_eq!(
        terminals.len(),
        1,
        "exactly one terminal attachment: {events:?}"
    );
    assert!(
        matches!(terminals[0], AttachmentEvent::Failed(_)),
        "expected Failed terminal: {terminals:?}"
    );
}

/// Happy path: nonterminal watch updates stay live; exactly one terminal Ok
/// appears after the barrier, matching disk and Completed attachment.
#[tokio::test]
async fn success_path_publishes_exactly_one_terminal_ok_after_barrier() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let stall = WriterStall::new_stalled();
    let hooks = PersistHooks {
        stall: Some(Arc::clone(&stall)),
        ..PersistHooks::default()
    };
    let (status_tx, status_rx) = watch::channel(RunStatus::default());
    let mut sink = StatusSink::new_with_hooks(
        output.clone(),
        &identity(),
        "prompt",
        Some(status_tx),
        hooks,
    )
    .unwrap();

    sink.mark_running().unwrap();
    sink.apply_effect(StreamEffect::Status(StatusPatch {
        state: Some(RunState::Running),
        last_activity: Some("assistant text".into()),
        append_text: Some("hello".into()),
        ..StatusPatch::default()
    }))
    .unwrap();
    sink.apply_effect(StreamEffect::Attachment(
        AttachmentEvent::AssistantTextDelta("hello".into()),
    ))
    .unwrap();

    // Live nonterminal updates must already be visible while the writer is stalled.
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(status_rx.borrow().state, RunState::Running);
    assert!(
        !status_rx.borrow().state.is_terminal(),
        "no terminal before barrier"
    );

    let finalize = tokio::spawn(async move {
        sink.finalize_success_from_stream(&success_terminal()).await;
        let memory = sink.status.clone();
        sink.shutdown().await;
        memory
    });

    // Finalize is waiting on the barrier; still no terminal Ok on the watch.
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(
        !status_rx.borrow().state.is_terminal(),
        "Ok must not publish before barrier resolves: {:?}",
        status_rx.borrow().state
    );

    let (memory, states) = collect_watch_states(status_rx, async {
        stall.release();
        tokio::time::timeout(Duration::from_secs(2), finalize)
            .await
            .expect("barrier should finish")
            .unwrap()
    })
    .await;

    assert_eq!(memory.state, RunState::Ok);
    assert_eq!(
        count_terminal(&states),
        1,
        "exactly one terminal watch publish: {states:?}"
    );
    assert_eq!(states.last().copied(), Some(RunState::Ok));
    assert!(
        states
            .iter()
            .filter(|state| **state == RunState::Ok)
            .count()
            == 1,
        "Ok must appear once: {states:?}"
    );

    let disk = subagent::read_status(&output).expect("status");
    assert_eq!(disk.state, RunState::Ok);
    assert_eq!(disk.result.as_deref(), Some("done"));
    assert_eq!(disk.state, memory.state);

    let events = read_attachment_events(&output);
    let terminals: Vec<_> = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                AttachmentEvent::Completed
                    | AttachmentEvent::Failed(_)
                    | AttachmentEvent::Cancelled
            )
        })
        .collect();
    assert_eq!(terminals.len(), 1);
    assert!(matches!(terminals[0], AttachmentEvent::Completed));
}

/// Cancellation still ends Stopped with a single terminal watch publish.
#[tokio::test]
async fn stop_path_publishes_exactly_one_terminal_stopped_on_watch() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.json");
    let stall = WriterStall::new_stalled();
    let hooks = PersistHooks {
        stall: Some(Arc::clone(&stall)),
        ..PersistHooks::default()
    };
    let (status_tx, status_rx) = watch::channel(RunStatus::default());
    let mut sink = StatusSink::new_with_hooks(
        output.clone(),
        &identity(),
        "prompt",
        Some(status_tx),
        hooks,
    )
    .unwrap();

    sink.mark_running().ok();
    let stop = tokio::spawn(async move {
        sink.stop("cancelled").await;
        let memory = sink.status.clone();
        sink.shutdown().await;
        memory
    });

    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        !status_rx.borrow().state.is_terminal(),
        "Stopped must wait for barrier: {:?}",
        status_rx.borrow().state
    );

    let (memory, states) = collect_watch_states(status_rx, async {
        stall.release();
        tokio::time::timeout(Duration::from_secs(2), stop)
            .await
            .expect("stop should finish")
            .unwrap()
    })
    .await;

    assert_eq!(memory.state, RunState::Stopped);
    assert_eq!(count_terminal(&states), 1, "states: {states:?}");
    assert_eq!(states.last().copied(), Some(RunState::Stopped));

    let disk = subagent::read_status(&output).expect("status");
    assert_eq!(disk.state, RunState::Stopped);
    assert_eq!(disk.state, memory.state);

    let events = read_attachment_events(&output);
    assert!(events
        .iter()
        .any(|event| matches!(event, AttachmentEvent::Cancelled)));
}
