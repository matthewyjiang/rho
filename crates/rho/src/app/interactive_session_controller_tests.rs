use pretty_assertions::assert_eq;
use rho_sdk::{model::ModelIdentity, provider::ScriptedProvider, Rho, SessionOptions};

use super::*;

// Covers: a failed save must not carry a prior turn's checkpoint offset into a
// later save, even if no durable-state rollback succeeds. Owner: session storage.
#[tokio::test]
async fn failed_save_does_not_skip_the_next_turn_display() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("workspace");
    std::fs::create_dir(&cwd).unwrap();
    let storage = StoredSession::create_in_root(root.path(), &cwd).unwrap();
    let runtime = Rho::builder()
        .provider(ScriptedProvider::new(
            ModelIdentity::new("test", "test", "test"),
            Vec::new(),
        ))
        .build()
        .unwrap();
    // The wrong session ID triggers the real snapshot/store identity guard.
    let wrong_session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut controller = InteractiveSessionController::new(
        wrong_session,
        Some(storage.clone()),
        WebAccessStore::new(),
        /*recall*/ None,
        /*advisor*/ None,
    );
    let failed = PendingTurn::new(
        Message::user_text("failed model input"),
        Some(vec![
            Message::user_text("already checkpointed"),
            Message::System("unsaved receipt".into()),
        ]),
        /*history_start*/ 0,
    );
    controller.persisted_turn_display = 1;
    assert!(controller.sync_finished_turn(Some(&failed), None).is_err());

    // Deliberately bypass replacement helpers: this test must not rely on a
    // successful rollback or another caller resetting the offset for us.
    controller.session = runtime
        .session(SessionOptions::new().id(SessionId::from_string(storage.id()).unwrap()))
        .await
        .unwrap();
    let next = Message::user_text("next human prompt");
    let next_turn = PendingTurn::new(
        next.clone(),
        /*display_user*/ None,
        /*history_start*/ 0,
    );
    controller
        .sync_finished_turn(Some(&next_turn), None)
        .unwrap();
    let (_, histories) =
        StoredSession::open_by_id_with_histories_in_root(root.path(), &cwd, storage.id()).unwrap();
    assert_eq!(histories.display, vec![next]);
}

// Covers: context persisted between /new and its first turn uses the new durable
// identity and retains launch-owned prompts on resume, including an intentional
// no-system-prompt policy. PTY owns the newborn-empty-store request path instead.
// Owner: interactive session controller persistence.
#[tokio::test]
async fn pending_reset_context_snapshot_preserves_prompt_on_resume() {
    for system in [
        rho_sdk::SystemPrompt::None,
        rho_sdk::SystemPrompt::Custom("pinned replacement".into()),
    ] {
        let root = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let storage = StoredSession::create_in_root(root.path(), cwd.path()).unwrap();
        let runtime = Rho::builder()
            .provider(ScriptedProvider::new(
                ModelIdentity::new("test", "test", "test"),
                Vec::new(),
            ))
            .system_prompt(system.clone())
            .build()
            .unwrap();
        let session = runtime.session(SessionOptions::new()).await.unwrap();
        let mut controller = InteractiveSessionController::new(
            session,
            /*storage*/ None,
            WebAccessStore::new(),
            /*recall*/ None,
            /*advisor*/ None,
        );
        controller.prompt =
            crate::app::active_prompt::ActivePrompt::new(system.clone(), None, Vec::new());
        controller.reset().unwrap();
        // Production creates storage with reset's ID. This private-state test
        // uses the isolated store's allocated ID for the same pending identity.
        controller.pending_session_id = Some(SessionId::from_string(storage.id()).unwrap());
        controller.attach_storage(storage.clone());
        controller
            .session()
            .append_message(Message::user_text("pre-turn context"))
            .unwrap();
        controller
            .save_snapshot(&[Message::user_text("context notice")])
            .unwrap();

        let snapshot = controller.snapshot();
        let saved = storage
            .snapshot_for_resume(
                ModelIdentity::new("test", "test", "test"),
                format!("rho:{}", storage.id()),
            )
            .unwrap();
        assert_eq!(saved, snapshot);
        let restored_prompt =
            crate::app::active_prompt::ActivePrompt::from_snapshot(&saved).unwrap();
        assert_eq!(restored_prompt.system, system);
        // This runtime has no prompt template: resume takes Keep, so it cannot
        // mask a missing stored prompt by preparing a replacement from disk.
        let mut resumed = crate::app::interactive_runtime::test_runtime(Vec::new()).await;
        resumed.resume(storage.clone()).await.unwrap();
        assert_eq!(resumed.history(), controller.history());
        let (_, histories) =
            StoredSession::open_by_id_with_histories_in_root(root.path(), cwd.path(), storage.id())
                .unwrap();
        assert_eq!(
            histories.display,
            vec![Message::user_text("context notice")]
        );
    }
}

// Covers: recall follows the durable storage through every storage change:
// unbound without storage, bound on attach, cleared by /new's reset, and
// rebound on resume. A stale binding would stub results into a retired session.
// Owner: interactive session storage sidecars.
#[tokio::test]
async fn recall_binding_follows_storage_changes() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let runtime = rho_sdk::Rho::builder()
        .provider(ScriptedProvider::new(
            ModelIdentity::new("test", "test", "test"),
            Vec::new(),
        ))
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let recall = crate::session::recall::RecallStore::default();
    let mut controller = InteractiveSessionController::new(
        session,
        /*storage*/ None,
        WebAccessStore::new(),
        Some(recall.clone()),
        /*advisor*/ None,
    );
    let first = StoredSession::create_in_root(root.path(), cwd.path()).unwrap();
    let second = StoredSession::create_in_root(root.path(), cwd.path()).unwrap();

    assert_eq!(recall.dir(), None);
    controller.attach_storage(first.clone());
    assert_eq!(recall.dir(), first.recall_dir());
    controller.reset().unwrap();
    assert_eq!(recall.dir(), None);
    controller.set_resumed_storage(second.clone());
    assert_eq!(recall.dir(), second.recall_dir());
}
