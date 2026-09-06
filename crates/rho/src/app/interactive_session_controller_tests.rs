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
