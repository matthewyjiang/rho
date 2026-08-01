use super::*;

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
fn start_and_notification_prompts_include_run_identity() {
    let (start_model, start_display) =
        start_context_prompts("abc-123", "thermo", "sha256:deadbeef");
    assert!(start_model.contains("run_id: abc-123"));
    assert!(start_model.contains("workflow: thermo"));
    assert!(start_display.contains("abc-123"));

    let (model, display) = notification_prompts(&[WorkflowNotification {
        run_id: "abc-123".into(),
        workflow_name: "thermo".into(),
        graph_digest: "sha256:deadbeef".into(),
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
    assert!(model.contains("[workflow notification]"));
    assert!(model.contains("abc-123"));
    assert!(model.contains("collect"));
    assert!(model.contains(r#"{"ok":true}"#));
    assert!(display.contains("finished"));
}
