use super::*;
use pretty_assertions::assert_eq;

// Covers: another session's workflows must not leak into the rail, and delivery
// hides only the drained terminal rows (restoring failed delivery reopens them).
// Owner: tracker policy; notification tests below do not assert rail visibility.
#[test]
fn rail_rows_are_session_scoped_and_follow_delivery() {
    let tracker = WorkflowRunTracker::new();
    tracker.register_start("unbound", "orphan", "digest", None);
    tracker.bind_parent_session("parent");
    for (id, session) in [
        ("active", None),
        ("finished", None),
        ("other", Some("other")),
    ] {
        tracker.register_start(id, id, "digest", session.map(str::to_owned));
    }
    tracker.mark_failed("finished", "driver failed");
    let ids = |session| {
        tracker
            .rail_summaries(session)
            .into_iter()
            .map(|row| row.run_id)
            .collect::<Vec<_>>()
    };
    for (session, expected) in [
        ("parent", vec!["active", "finished"]),
        ("other", vec!["other"]),
        ("unknown", vec![]),
    ] {
        assert_eq!(ids(session), expected, "{session}");
    }
    let delivery = tracker.take_notifications("parent");
    assert_eq!(ids("parent"), vec!["active"]);
    tracker.restore_notifications(&delivery);
    assert_eq!(ids("parent"), vec!["active", "finished"]);
    tracker.observe("finished");
    tracker.restore_notifications(&delivery);
    assert_eq!(ids("parent"), vec!["active"]);
}

// Covers: runtime events must keep completion counts/current task accurate,
// retain cancelling across launches, and never revive terminal rows.
// Owner: tracker policy; the existing tests cover notifications, not live events.
#[test]
fn rail_runtime_events_preserve_lifecycle_and_terminal_snapshot() {
    use crate::workflow::{AttemptNumber, NodeId, NodeTerminalState, TaskInstanceId};
    let tracker = WorkflowRunTracker::new();
    tracker.bind_parent_session("parent");
    tracker.register_start("run", "review", "digest", None);
    let task = |id| TaskInstanceId::root(NodeId::new(id).unwrap());
    let attempt = AttemptNumber::new(1).unwrap();
    let inspect = task("inspect");
    let build = task("build");
    tracker.record_event(
        "run",
        &RuntimeEvent::StateChanged {
            revision: 1,
            activity: crate::app::workflow_runtime::WorkflowActivitySnapshot {
                lifecycle: RunLifecycle::Cancelling,
                outcome: None,
                tasks: BTreeMap::from([
                    (inspect.to_string(), NodeState::Ready),
                    (build.to_string(), NodeState::Pending),
                ]),
            },
        },
    );
    assert!(!tracker.register_start("run", "replacement", "other digest", None));
    let cases = [
        (
            RuntimeEvent::NodeStarted {
                node: inspect.clone(),
                attempt,
            },
            0,
            Some("inspect"),
        ),
        (
            RuntimeEvent::NodeProgress {
                node: inspect.clone(),
                attempt,
                message: "working".into(),
                detail: None,
                completed: None,
                total: None,
            },
            0,
            Some("inspect"),
        ),
        (
            RuntimeEvent::NodeFinished {
                node: inspect,
                outcome: NodeTerminalState::Success,
            },
            1,
            None,
        ),
        (
            RuntimeEvent::NodeStarted {
                node: build.clone(),
                attempt,
            },
            1,
            Some("build"),
        ),
        (
            RuntimeEvent::NodeFinished {
                node: build.clone(),
                outcome: NodeTerminalState::Cancellation,
            },
            2,
            None,
        ),
    ];
    for (event, completed, active) in cases {
        tracker.record_event("unregistered", &event);
        tracker.record_event("run", &event);
        let row = tracker.rail_summaries("parent").remove(0);
        assert_eq!(
            (
                row.lifecycle,
                row.completed_tasks,
                row.total_tasks,
                row.active_task
            ),
            (
                RunLifecycle::Cancelling,
                completed,
                2,
                active.map(str::to_owned)
            )
        );
    }
    tracker.mark_finished(
        "run",
        WorkflowFinishedSnapshot {
            lifecycle: "completed".into(),
            outcome: Some("cancellation".into()),
            nodes: Vec::new(),
            error: None,
            outputs: Vec::new(),
        },
    );
    // Freeze a known interval without sleeping or depending on scheduler speed.
    {
        let mut inner = tracker.inner.lock().unwrap();
        let entry = inner.runs.get_mut("run").unwrap();
        let end = Instant::now();
        entry.started = end - std::time::Duration::from_secs(60);
        entry.finished_at = Some(end);
    }
    let expected = WorkflowRailSummary {
        run_id: "run".into(),
        workflow_name: "review".into(),
        lifecycle: RunLifecycle::Completed,
        outcome: Some(WorkflowOutcome::Cancellation),
        failed: false,
        completed_tasks: 2,
        total_tasks: 2,
        active_task: None,
        elapsed_seconds: 60,
    };
    tracker.record_event(
        "run",
        &RuntimeEvent::NodeStarted {
            node: build,
            attempt,
        },
    );
    assert_eq!(tracker.rail_summaries("parent"), vec![expected]);
}

// Covers: a driver error is not proof that the durable workflow completed.
// Owner: tracker projection; completion delivery still reports the driver error.
#[test]
fn driver_failure_preserves_last_known_workflow_lifecycle() {
    for lifecycle in [
        RunLifecycle::Planned,
        RunLifecycle::Running,
        RunLifecycle::Cancelling,
    ] {
        let tracker = WorkflowRunTracker::new();
        tracker.bind_parent_session("parent");
        assert!(tracker.register_start("run", "review", "digest", None));
        tracker.record_event(
            "run",
            &RuntimeEvent::StateChanged {
                revision: 1,
                activity: crate::app::workflow_runtime::WorkflowActivitySnapshot {
                    lifecycle,
                    outcome: None,
                    tasks: BTreeMap::new(),
                },
            },
        );
        tracker.mark_failed("run", "driver lost ownership");
        let row = tracker.rail_summaries("parent").remove(0);
        assert_eq!(
            (row.lifecycle, row.failed, row.is_live()),
            (lifecycle, true, false)
        );
        let notification = tracker.take_notifications("parent").remove(0);
        assert_eq!(
            notification.finished.error.as_deref(),
            Some("driver lost ownership")
        );
    }
}

#[test]
fn take_notifications_drains_finished_runs_once() {
    let tracker = WorkflowRunTracker::new();
    tracker.bind_parent_session("session-1");
    tracker.register_start("run-a", "review", "digest-a", None);
    tracker.register_start("run-b", "review", "digest-b", None);
    assert!(tracker.take_notifications("session-1").is_empty());
    assert!(tracker.has_active_or_pending_notification("session-1"));

    tracker.mark_finished(
        "run-a",
        WorkflowFinishedSnapshot {
            lifecycle: "completed".into(),
            outcome: Some("success".into()),
            nodes: vec![WorkflowNodeLine {
                node_id: "inspect".into(),
                state: "success".into(),
            }],
            error: None,
            outputs: Vec::new(),
        },
    );
    let batch = tracker.take_notifications("session-1");
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].run_id, "run-a");
    assert!(tracker.take_notifications("session-1").is_empty());
    assert!(tracker.has_active_or_pending_notification("session-1"));

    tracker.mark_finished(
        "run-b",
        WorkflowFinishedSnapshot {
            lifecycle: "completed".into(),
            outcome: Some("failure".into()),
            nodes: Vec::new(),
            error: None,
            outputs: Vec::new(),
        },
    );
    let batch = tracker.take_notifications("session-1");
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].run_id, "run-b");
    assert!(!tracker.has_active_or_pending_notification("session-1"));
}

#[test]
fn observe_suppresses_automatic_delivery() {
    let tracker = WorkflowRunTracker::new();
    tracker.bind_parent_session("session-1");
    tracker.register_start("run-a", "review", "digest-a", None);
    tracker.mark_finished(
        "run-a",
        WorkflowFinishedSnapshot {
            lifecycle: "completed".into(),
            outcome: Some("success".into()),
            nodes: Vec::new(),
            error: None,
            outputs: Vec::new(),
        },
    );
    tracker.observe("run-a");
    assert!(tracker.take_notifications("session-1").is_empty());
}

#[test]
fn restored_notifications_can_drain_again() {
    let tracker = WorkflowRunTracker::new();
    tracker.bind_parent_session("session-1");
    tracker.register_start("run-a", "review", "digest-a", None);
    tracker.mark_finished(
        "run-a",
        WorkflowFinishedSnapshot {
            lifecycle: "completed".into(),
            outcome: Some("success".into()),
            nodes: Vec::new(),
            error: None,
            outputs: Vec::new(),
        },
    );
    let batch = tracker.take_notifications("session-1");
    assert_eq!(batch.len(), 1);
    tracker.restore_notifications(&batch);
    let again = tracker.take_notifications("session-1");
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].run_id, "run-a");
    tracker.observe("run-a");
    tracker.restore_notifications(&again);
    assert!(tracker.take_notifications("session-1").is_empty());
}

#[test]
fn start_and_notification_prompts_include_run_identity() {
    let (start_model, start_display) =
        start_context_prompts("abc-123", "thermo", "sha256:deadbeef");
    assert!(start_model.contains("run_id: abc-123"));
    assert!(start_model.contains("workflow: thermo"));
    assert!(start_display.contains("abc-123"));

    let (model, display) = notification_prompts(&[WorkflowNotification {
        run_id: "abc-123".into(),
        workflow_name: "thermo".into(),
        program_digest: "sha256:deadbeef".into(),
        finished: WorkflowFinishedSnapshot {
            lifecycle: "completed".into(),
            outcome: Some("success".into()),
            nodes: vec![WorkflowNodeLine {
                node_id: "collect".into(),
                state: "success".into(),
            }],
            error: None,
            outputs: vec![("collect".into(), r#"{"ok":true}"#.into())],
        },
    }]);
    assert!(model.contains("abc-123"));
    assert!(model.contains("collect"));
    assert!(model.contains(r#"{"ok":true}"#));
    assert!(display.contains("abc-123"));
}
