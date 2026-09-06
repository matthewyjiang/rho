use super::*;
use pretty_assertions::assert_eq;

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
