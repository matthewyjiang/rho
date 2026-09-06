use super::*;
use pretty_assertions::assert_eq;

// Covers: finish must return immediately when idle and drain a cancelled run
// without waiting for another provider response.
// Owner: interactive runtime run lifecycle.
#[tokio::test]
async fn finish_run_handles_idle_and_cancelled_runs() {
    use futures_util::FutureExt;

    let mut interactive = test_runtime(Vec::new()).await;
    assert!(interactive.finish_run().now_or_never().unwrap().is_err());
    interactive
        .start(UserInput::text("cancel before provider response"), None)
        .await
        .unwrap();
    interactive.cancel();
    let error = interactive.finish_run().await.unwrap_err();
    assert!(matches!(
        error.downcast_ref::<rho_sdk::Error>(),
        Some(rho_sdk::Error::Cancelled)
    ));
    assert!(!interactive.is_run_active());
    assert!(interactive.finish_run().now_or_never().unwrap().is_err());
}

// Covers: finishing without consuming compaction events must persist the actual
// compaction checkpoint, not just the final model history.
// Owner: interactive runtime persistence transaction.
#[tokio::test]
async fn finish_run_persists_unconsumed_automatic_compaction() {
    let mut interactive = test_runtime(vec![
        ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
            "compact summary".into(),
        )])),
        ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
            "done".into(),
        )])),
    ])
    .await;
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("workspace");
    std::fs::create_dir(&cwd).unwrap();
    let storage = StoredSession::create_in_root(root.path(), &cwd).unwrap();
    let history = vec![
        Message::user_text("x".repeat(2_000)),
        Message::assistant_text("y".repeat(2_000)),
    ];
    let session = interactive
        .runtime
        .session(
            SessionOptions::new()
                .id(SessionId::from_string(storage.id()).unwrap())
                .history(history.clone()),
        )
        .await
        .unwrap();
    interactive.sessions.replace_session(session, None);
    interactive.sessions.attach_storage(storage.clone());
    interactive.sessions.save_snapshot(&history).unwrap();
    // Match the existing automatic-compaction fixture: 4,000 history characters
    // exceed the 10-token trigger in a 1,000-token context window.
    interactive.compaction = CompactionConfig {
        auto_compact: true,
        threshold_percent: 1,
        target_percent: 1,
    };
    interactive.set_context_window(Some(1_000)).unwrap();
    interactive
        .start(UserInput::text("continue"), None)
        .await
        .unwrap();

    assert_eq!(interactive.finish_run().await.unwrap().text(), "done");

    let tree = crate::session::tree::SessionTree::load(storage.path()).unwrap();
    assert_eq!(
        tree.active_path()
            .unwrap()
            .iter()
            .filter(|node| node.kind() == crate::session::tree::SessionNodeKind::Compaction)
            .count(),
        1
    );
    let path = tree.active_path().unwrap();
    let checkpoint = path
        .iter()
        .find(|node| node.kind() == crate::session::tree::SessionNodeKind::Compaction)
        .unwrap();
    let snapshot = storage
        .snapshot_for_node(
            checkpoint.id(),
            interactive.provider_identity(),
            super::super::prompt_cache_key(storage.id()),
        )
        .unwrap();
    let mut expected_history = snapshot.history().to_vec();
    expected_history.push(interactive.history().last().unwrap().clone());
    assert_eq!(interactive.history(), expected_history);
    let (_, histories) =
        StoredSession::open_by_id_with_histories_in_root(root.path(), &cwd, storage.id()).unwrap();
    assert_eq!(histories.model, interactive.history());
}

// Covers: a successful provider response must not acknowledge durable receipt
// display when snapshot saving fails and the runtime restores the prior leaf.
// Owner: interactive runtime persistence transaction.
#[tokio::test]
async fn failed_snapshot_save_does_not_commit_turn_display() {
    let mut interactive = test_runtime(vec![
        ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
            "saved".into(),
        )])),
        ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
            "not saved".into(),
        )])),
    ])
    .await;
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("workspace");
    std::fs::create_dir(&cwd).unwrap();
    let storage = StoredSession::create_in_root(root.path(), &cwd).unwrap();
    let session = interactive
        .runtime
        .session(SessionOptions::new().id(SessionId::from_string(storage.id()).unwrap()))
        .await
        .unwrap();
    interactive.sessions.replace_session(session, None);
    interactive.sessions.attach_storage(storage.clone());
    interactive
        .start(UserInput::text("first"), None)
        .await
        .unwrap();
    while interactive.next_event().await.is_some() {}
    interactive.finish_run().await.unwrap();
    assert_eq!(
        interactive.take_last_turn_display_commit(),
        DisplayCommit::Complete
    );
    let durable_history = interactive.history();

    // Trigger the real snapshot/store identity guard without filesystem races or
    // an injected global failure. Rollback must rebind the original stored ID.
    let mismatched_session = interactive
        .runtime
        .session(SessionOptions::new().history(durable_history.clone()))
        .await
        .unwrap();
    interactive
        .sessions
        .replace_runtime_session(mismatched_session);
    interactive
        .start(UserInput::text("second"), None)
        .await
        .unwrap();
    while interactive.next_event().await.is_some() {}
    assert!(interactive.finish_run().await.is_err());
    assert_eq!(
        interactive.take_last_turn_display_commit(),
        DisplayCommit::Unsaved
    );
    assert_eq!(interactive.history(), durable_history);
    assert_eq!(interactive.session_id().as_str(), storage.id());
    let (_, histories) =
        StoredSession::open_by_id_with_histories_in_root(root.path(), &cwd, storage.id()).unwrap();
    assert_eq!(histories.display, durable_history);
}
