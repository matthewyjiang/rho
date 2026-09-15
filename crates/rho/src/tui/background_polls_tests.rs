use std::sync::Arc;

use pretty_assertions::assert_eq;
use rho_providers::model::ModelMetadata;
use rho_sdk::{
    model::{ContentBlock, ModelEvent, ModelIdentity, ModelResponse, ModelUsage},
    provider::{ScriptedProvider, ScriptedTurn},
    UserInput,
};
use tokio::sync::oneshot;

use crate::{
    app::interactive_runtime::test_edit_tool_runtime, config::EditTool, tui::tests::test_app,
};

// Covers: late catalog metadata must not discard a successful context calibration
// when the effective reasoning is unchanged. The PTY calibrated_context_auto_compact
// scenario covers the visible outcome, but cannot control catalog completion order.
// Owner: host metadata/session lifecycle, with an injected completed catalog task.
#[tokio::test]
async fn unchanged_metadata_preserves_calibrated_context() {
    let mut app = test_app();
    let reasoning = app.info.runtime.reasoning;
    let provider = ScriptedProvider::new(
        ModelIdentity::new("openai", "openai-responses", "gpt-5.5"),
        [ScriptedTurn::streaming(
            vec![ModelEvent::Usage(ModelUsage {
                input_tokens: Some(100_000),
                ..ModelUsage::default()
            })],
            ModelResponse::Assistant(vec![ContentBlock::Text("done".into())]),
        )],
    );
    let mut agent = test_edit_tool_runtime(EditTool::Auto).await;
    agent
        .replace_provider(Arc::new(provider), reasoning, "api-key")
        .unwrap();
    agent.set_context_window(Some(131_072)).unwrap();
    agent.start(UserInput::text("work"), None).await.unwrap();
    agent.finish_run().await.unwrap();
    let calibrated = agent.take_context_usage().unwrap();
    assert!(calibrated.tokens.unwrap() >= 100_000);
    let history = agent.history();

    let metadata = ModelMetadata {
        usable_context_window: Some(131_072),
        supported_reasoning_levels: Some(vec![reasoning]),
        reasoning_capabilities_known: true,
        reasoning_metadata_complete: true,
        ..ModelMetadata::default()
    };
    let fetched = metadata.clone();
    let (finished, completion) = oneshot::channel();
    app.pending_model_metadata = Some(tokio::spawn(async move {
        // No await after the signal: this current-thread task completes before
        // the receiver can resume and inspect its JoinHandle.
        finished.send(()).unwrap();
        Some(fetched)
    }));
    completion.await.unwrap();
    assert!(app.pending_model_metadata.as_ref().unwrap().is_finished());
    app.poll_model_metadata_fetch(&mut agent).await;

    assert!(app.pending_model_metadata.is_none());
    assert_eq!(app.model_metadata, Some(metadata));
    assert_eq!(agent.history(), history);
    assert_eq!(agent.take_context_usage(), Some(calibrated));
}
