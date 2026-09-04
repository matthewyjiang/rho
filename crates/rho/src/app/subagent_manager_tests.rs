use pretty_assertions::assert_eq;

use super::{SubagentManager, SubagentTaskIdentity};

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
