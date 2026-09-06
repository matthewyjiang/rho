use super::*;
use pretty_assertions::assert_eq;

async fn automatic_compaction_fixture(
    turns: Vec<ScriptedTurn>,
) -> (
    InteractiveRuntime,
    tempfile::TempDir,
    StoredSession,
    Vec<Message>,
) {
    let mut interactive = test_runtime(turns).await;
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
    // 4,000 history characters exceed the 10-token trigger in a 1,000-token window.
    interactive.compaction = CompactionConfig {
        auto_compact: true,
        threshold_percent: 1,
        target_percent: 1,
    };
    interactive.set_context_window(Some(1_000)).unwrap();
    (interactive, root, storage, history)
}

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

// Covers: finish must drain and persist a queued compaction checkpoint, including
// cancellation after compaction commits but before the next provider response.
// Owner: interactive runtime persistence transaction.
#[tokio::test]
async fn finish_run_persists_unconsumed_automatic_compaction() {
    for cancel in [false, true] {
        let (mut interactive, root, storage, history) = automatic_compaction_fixture(vec![
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "compact summary".into(),
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "done".into(),
            )])),
        ])
        .await;
        let cwd = root.path().join("workspace");
        let mut boundaries = interactive
            .start_with_boundary_inputs(
                UserInput::text("continue"),
                /*display_user*/ None,
                /*tool_call*/ None,
            )
            .await
            .unwrap();

        // BeforeProvider is requested only after the SDK's awaited
        // CompactionCompleted send succeeds. No next_event call consumes that queue.
        let boundary = boundaries.recv().await.unwrap();
        assert_eq!(boundary.boundary(), rho_sdk::InputBoundary::BeforeProvider);
        let checkpoint_history = interactive.history();
        assert_ne!(checkpoint_history, history);
        if cancel {
            interactive.cancel();
            let error = interactive.finish_run().await.unwrap_err();
            assert!(matches!(
                error.downcast_ref::<rho_sdk::Error>(),
                Some(rho_sdk::Error::Cancelled)
            ));
        } else {
            assert!(boundary.respond(None).await);
            let respond = async {
                while let Some(boundary) = boundaries.recv().await {
                    assert!(boundary.respond(None).await);
                }
            };
            // The session retains its boundary sender after completion, so stop
            // serving requests when finish_run returns rather than awaiting EOF.
            tokio::select! {
                result = interactive.finish_run() => { result.unwrap(); }
                () = respond => panic!("boundary channel closed before finish_run"),
            }
        }
        assert!(!interactive.is_run_active());

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
        assert_eq!(snapshot.history(), checkpoint_history);
        let mut expected_history = checkpoint_history;
        if !cancel {
            expected_history.push(interactive.history().last().unwrap().clone());
        }
        assert_eq!(interactive.history(), expected_history);
        let (_, histories) =
            StoredSession::open_by_id_with_histories_in_root(root.path(), &cwd, storage.id())
                .unwrap();
        assert_eq!(histories.model, interactive.history());
    }
}

// Covers: after checkpoint A saves and B fails, draining queued checkpoint C must
// neither rewrite A's display nor advance durability beyond the first failure.
// Owner: interactive runtime persistence transaction; boundary signals exercise
// the real SDK event queue and storage identity guard without filesystem races.
#[tokio::test]
async fn failed_compaction_freezes_persistence_while_draining_queued_checkpoints() {
    let turns = [
        "summary A",
        "answer A",
        "summary B",
        "answer B",
        "summary C",
    ]
    .into_iter()
    .map(|text| {
        ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
            text.into(),
        )]))
    })
    .collect();
    let (mut interactive, root, storage, _) = automatic_compaction_fixture(turns).await;
    let cwd = root.path().join("workspace");
    let mut boundaries = interactive
        .start_with_boundary_inputs(
            UserInput::text("continue"),
            /*display_user*/ None,
            /*tool_call*/ None,
        )
        .await
        .unwrap();
    let boundary_a = boundaries.recv().await.unwrap();
    assert_eq!(
        boundary_a.boundary(),
        rho_sdk::InputBoundary::BeforeProvider
    );
    loop {
        if let RunEvent::CompactionCompleted { .. } = interactive.next_event().await.unwrap() {
            break;
        }
    }
    assert!(interactive.pending_persistence_error.is_none());
    let (_, durable_a) =
        StoredSession::open_by_id_with_histories_in_root(root.path(), &cwd, storage.id()).unwrap();
    let durable_bytes = std::fs::read(storage.path()).unwrap();
    assert_eq!(durable_a.model, interactive.history());

    assert!(boundary_a.respond(None).await);
    // Each completed answer supplies another oversized input at BeforeCompletion.
    // BeforeProvider proves the next CompactionCompleted send has finished. Leave
    // B and C queued, with C's provider blocked so no extra model turn can run.
    for checkpoint in ["B", "C"] {
        let completion = boundaries.recv().await.unwrap();
        assert_eq!(
            completion.boundary(),
            rho_sdk::InputBoundary::BeforeCompletion
        );
        assert!(
            completion
                .respond(Some(UserInput::text("z".repeat(2_000))))
                .await
        );
        let provider = boundaries.recv().await.unwrap();
        assert_eq!(provider.boundary(), rho_sdk::InputBoundary::BeforeProvider);
        assert_ne!(interactive.history(), durable_a.model);
        if checkpoint == "B" {
            assert!(provider.respond(None).await);
        } else {
            // Keep the request alive through finish_run: cancellation, not dropping
            // its response sender, must release the blocked SDK run.
            let wrong_storage = StoredSession::create_in_root(root.path(), &cwd).unwrap();
            assert_ne!(wrong_storage.id(), storage.id());
            interactive.sessions.attach_storage(wrong_storage);
            loop {
                if let RunEvent::CompactionCompleted { .. } =
                    interactive.next_event().await.unwrap()
                {
                    break;
                }
            }
            let first_error = interactive
                .pending_persistence_error
                .as_ref()
                .unwrap()
                .to_string();
            // The deliberately wrong store is not a rollback target. Exercise the
            // fallback that reloads the reattached original store at finish_run.
            interactive.pending_persistence_checkpoint = None;
            interactive.sessions.attach_storage(storage.clone());
            let error = interactive.finish_run().await.unwrap_err();
            assert_eq!(
                error.to_string(),
                format!("could not persist automatic compaction: {first_error}")
            );
            drop(provider);
        }
    }

    assert!(!interactive.is_run_active());
    assert_eq!(
        interactive.take_last_turn_display_commit(),
        DisplayCommit::Checkpoint(vec![Message::user_text("continue")])
    );
    assert_eq!(interactive.session_id().as_str(), storage.id());
    assert_eq!(interactive.sessions.storage().unwrap().id(), storage.id());
    assert_eq!(interactive.history(), durable_a.model);
    let (_, after_failure) =
        StoredSession::open_by_id_with_histories_in_root(root.path(), &cwd, storage.id()).unwrap();
    assert_eq!(after_failure.model, durable_a.model);
    assert_eq!(after_failure.display, durable_a.display);
    // Byte equality also rejects extra checkpoint rows and duplicate display rows,
    // even if restoring the active leaf would otherwise hide them from histories.
    assert_eq!(std::fs::read(storage.path()).unwrap(), durable_bytes);
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
