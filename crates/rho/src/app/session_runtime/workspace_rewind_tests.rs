use std::fs;

use pretty_assertions::assert_eq;
use rho_sdk::model::Message;
use rho_tools::WorkspaceMutationObserver as _;

use super::RewoundConversation;
use crate::{
    app::session_runtime::{test_support::idle_runtime, SessionRuntime},
    config::Config,
    session::{tree::NodeId, workspace_checkpoint::CheckpointOutcome},
};

fn leaf(runtime: &SessionRuntime) -> NodeId {
    let stored = runtime.storage.file().expect("the session is saved");
    stored
        .active_checkpoint_target()
        .unwrap()
        .expect("the session saved a node")
        .0
}

fn add(runtime: &mut SessionRuntime, text: &str) -> NodeId {
    runtime
        .append_context(text.into(), Message::user_text(text), [])
        .unwrap();
    leaf(runtime)
}

// Covers: a turn's checkpoint, opened before the turn saves anything and
// closed on its last node, lets a rewind put back the files the turn added,
// changed, and removed, and returns the conversation to before the turn.
// Owner: session runtime workspace rewind
#[tokio::test]
async fn a_rewind_puts_the_turns_files_and_conversation_back() {
    let home = tempfile::tempdir().unwrap();
    let config = Config {
        provider: "test".into(),
        model: "model".into(),
        ..Config::default()
    };
    let mut runtime = idle_runtime(home.path(), &config).await;
    runtime.built.tools.take_checkpoints_for_tests();
    // Tools hand the tracker resolved paths.
    let workspace = fs::canonicalize(home.path()).unwrap();
    let (added, deleted, modified) = (
        workspace.join("added.txt"),
        workspace.join("deleted.txt"),
        workspace.join("modified.txt"),
    );
    fs::write(&deleted, "deleted original").unwrap();
    fs::write(&modified, "modified original").unwrap();
    let before = add(&mut runtime, "before");

    let begun = runtime.begin_workspace_checkpoint().unwrap();
    let tracker = runtime.built.tools.checkpoint_tracker().clone();
    let paths = [added.as_path(), deleted.as_path(), modified.as_path()];
    tracker.before_mutation(&paths).await.unwrap();
    fs::write(&added, "agent added").unwrap();
    fs::remove_file(&deleted).unwrap();
    fs::write(&modified, "agent modified").unwrap();
    tracker.after_mutation(&paths).await.unwrap();
    let turn = add(&mut runtime, "the turn");
    let told = runtime.finalize_workspace_checkpoint(CheckpointOutcome::Completed);

    let rewind = runtime.rewind_workspace(&turn, &config).await.unwrap();
    let read = |path: &std::path::Path| fs::read_to_string(path).ok();
    assert_eq!(
        (
            (begun, told),
            matches!(rewind.conversation, RewoundConversation::Restored),
            (read(&added), read(&deleted), read(&modified)),
            leaf(&runtime),
        ),
        (
            (None, Vec::new()),
            true,
            (
                None,
                Some("deleted original".to_owned()),
                Some("modified original".to_owned())
            ),
            before,
        ),
        "audit: {:?}",
        rewind.audit
    );
    runtime.shutdown().await;
}
