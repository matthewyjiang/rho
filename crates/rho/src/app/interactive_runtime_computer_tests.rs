use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, Message, ModelResponse},
    provider::ScriptedTurn,
    UserInput,
};

use crate::tools::computer_use::{ComputerUseSession, ComputerUseStatus};

// Covers: start_run consuming a completed driver failure must not discard input.
// Owner: interactive runtime boundary. PTY polling usually consumes the failure
// first, so hold the completed connection result until this exact boundary.
#[tokio::test]
async fn failed_computer_connect_preserves_next_prompt() {
    let mut runtime = super::super::tests::test_runtime(vec![ScriptedTurn::completed(
        ModelResponse::Assistant(vec![ContentBlock::Text("prompt received".into())]),
    )])
    .await;
    let root = tempfile::tempdir().unwrap();
    let computer = ComputerUseSession::new(
        Some(root.path().join("missing-driver")),
        crate::config::Config::default().max_output_bytes,
        root.path().into(),
    );
    runtime.tools = runtime.tools.with_computer_use(computer.clone());
    runtime.enable_computer_use().unwrap();
    computer.wait_for_connect_result().await;
    runtime
        .start(
            UserInput::text("keep my prompt"),
            /*display_user*/ None,
        )
        .await
        .expect("driver failures must not abort a turn");
    while runtime.next_event().await.is_some() {}
    runtime.finish_run().await.unwrap();
    assert_eq!(computer.status(), ComputerUseStatus::Off);
    assert!(!runtime.tools.contains("computer"));
    assert_eq!(
        runtime
            .history()
            .iter()
            .find(|message| **message == Message::user_text("keep my prompt")),
        Some(&Message::user_text("keep my prompt"))
    );
    assert_eq!(runtime.take_notices().len(), 1);
    assert!(runtime.take_notices().is_empty());
}
