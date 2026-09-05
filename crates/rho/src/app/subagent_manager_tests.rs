use pretty_assertions::assert_eq;

use super::{SubagentManager, SubagentTaskIdentity};

// Covers: terminal status includes unconsumed findings and marks those exact
// queued notices consumed; later notices and explicit acknowledgements survive.
// Owner: delegated manager and its shared notice receipts.
#[test]
fn terminal_observation_reconciles_notices_and_cannot_be_undone_by_restore() {
    let root = tempfile::tempdir().unwrap();
    let manager = SubagentManager::new(
        crate::config::Config::default(),
        root.path().join("config.toml"),
        root.path().to_path_buf(),
    );
    manager.insert_completed_status_for_test(
        "abc123",
        "session",
        crate::subagent::RunStatus {
            state: crate::subagent::RunState::Ok,
            ..Default::default()
        },
    );
    let bridge = manager.executor.notices();
    let (mut receiver, permits) = bridge.bind_parent();
    let notice = super::SubagentNotice {
        run_id: "abc123".into(),
        agent_id: "fixture".into(),
        parent_session_id: rho_sdk::SessionId::from_string("session").unwrap(),
        message: "substantive finding before completion".into(),
        acknowledged: Default::default(),
    };
    bridge.post(notice.clone()).unwrap();
    let queued = receiver.try_recv().unwrap();
    let reserved = manager.take_notifications("session");
    let snapshot = manager.observe("abc123").unwrap();
    assert_eq!(snapshot.prior_notices, vec![notice.message.clone()]);
    assert!(queued.is_acknowledged());
    manager.restore_notifications(&reserved);
    assert!(manager.take_notifications("session").is_empty());
    // Same text, new receipt: do not confuse a late arrival with an earlier ack.
    let late = super::SubagentNotice {
        acknowledged: Default::default(),
        ..notice
    };
    bridge.post(late.clone()).unwrap();
    assert!(!receiver.try_recv().unwrap().is_acknowledged());
    permits.release_notice(&queued);
    assert_eq!(bridge.pending_for_run("abc123"), vec![late.clone()]);
    permits.release_notice(&late);
    assert!(bridge.pending_for_run("abc123").is_empty());
}

// Covers: generated task titles take precedence, while blank/absent titles
// fall back without observing or consuming a completed result.
// Owner: delegated-run identity policy; PTY coverage exercises only the fallback.
#[test]
fn task_identity_prefers_nonblank_generated_titles() {
    let root = tempfile::tempdir().unwrap();
    let manager = SubagentManager::new(
        crate::config::Config::default(),
        root.path().join("config.toml"),
        root.path().to_path_buf(),
    );
    for (title, fallback, expected) in [
        (
            Some("Generated title"),
            Some("Prompt line"),
            "Generated title",
        ),
        (Some(""), Some("Prompt line"), "Prompt line"),
        (Some(" \n\t"), Some("Prompt line"), "Prompt line"),
        (None, Some("Prompt line"), "Prompt line"),
        (None, None, "Delegated task"),
    ] {
        manager.insert_completed_status_for_test(
            "abc123",
            "session",
            crate::subagent::RunStatus {
                state: crate::subagent::RunState::Ok,
                title: title.map(str::to_owned),
                ..Default::default()
            },
        );
        manager
            .inner
            .lock()
            .unwrap()
            .get_mut("abc123")
            .unwrap()
            .task_fallback = fallback.map(str::to_owned);
        assert_eq!(
            manager.task_identity("abc123"),
            Some(SubagentTaskIdentity {
                run_id: "abc123".into(),
                agent_id: "fixture".into(),
                task: expected.into(),
            })
        );
        assert!(!manager.inner.lock().unwrap()["abc123"].observed);
    }
    assert_eq!(manager.task_identity("def456"), None);
}
