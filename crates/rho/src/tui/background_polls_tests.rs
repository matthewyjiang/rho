use std::sync::Arc;

use pretty_assertions::assert_eq;
use rho_providers::model::{ModelMetadata, ReasoningRequestSource};
use rho_sdk::{
    model::{
        context::estimate_context_tokens, ContentBlock, ContextUsage, ModelEvent, ModelIdentity,
        ModelResponse, ModelUsage,
    },
    provider::{ScriptedProvider, ScriptedTurn},
    ReasoningLevel, UserInput,
};
use tokio::sync::oneshot;

use crate::{
    app::interactive_runtime::{test_runtime, InteractiveRuntime},
    tui::{
        background_tasks::{SessionOutput, TaskId, TaskOutput},
        tests::test_app,
        App,
    },
};

/// The idle dispatch point for metadata, without a terminal: take what the
/// session accepts now and apply it.
async fn apply_finished_model_metadata(app: &mut App, agent: &mut InteractiveRuntime) {
    while let Some(output) = app.take_next_finished_task(agent) {
        let TaskOutput::Session(SessionOutput::ModelMetadata {
            reasoning_at_start,
            metadata,
        }) = output
        else {
            panic!("only a metadata task was registered");
        };
        app.apply_model_metadata(agent, reasoning_at_start, metadata)
            .await;
    }
}

// Covers: deferred, unchanged metadata discards a successful provider baseline,
// or a no-op guard prevents a genuine reasoning change from being installed.
// Owner: host metadata/session lifecycle. Inject completion in-process because
// PTY cannot control the catalog task without adding a production test hook;
// calibrated_context_auto_compact owns the visible next-prompt compact behavior.
#[tokio::test]
async fn deferred_metadata_preserves_context_unless_reasoning_changes() {
    for effective in [ReasoningLevel::Low, ReasoningLevel::High] {
        let mut app = test_app();
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
        let mut agent = test_runtime(Vec::new()).await;
        agent
            .replace_provider(Arc::new(provider), ReasoningLevel::Low, "api-key")
            .unwrap();
        agent.set_context_window(Some(131_072)).unwrap();

        let metadata = ModelMetadata {
            usable_context_window: Some(131_072),
            supported_reasoning_levels: Some(vec![effective]),
            reasoning_capabilities_known: true,
            reasoning_metadata_complete: true,
            ..ModelMetadata::default()
        };
        let (release, fetched) = oneshot::channel();
        let (finished, completion) = oneshot::channel();
        let reasoning_at_start = (
            ReasoningLevel::Low,
            ReasoningRequestSource::PersistedOrDefault,
        );
        app.tasks.spawn(
            TaskId::ModelMetadata,
            async move {
                let metadata = fetched.await.unwrap();
                // No await after this signal: the current-thread executor
                // finishes this task before the test can observe the result.
                finished.send(()).unwrap();
                Some(metadata)
            },
            move |metadata| {
                SessionOutput::ModelMetadata {
                    reasoning_at_start,
                    metadata,
                }
                .into()
            },
        );

        agent.start(UserInput::text("work"), None).await.unwrap();
        release.send(metadata.clone()).unwrap();
        completion.await.unwrap();
        assert!(app.tasks.has_finished());
        apply_finished_model_metadata(&mut app, &mut agent).await;
        assert!(app.tasks.has_pending());
        assert_eq!(app.model_metadata, None);

        agent.finish_run().await.unwrap();
        let calibrated = agent.take_context_usage().unwrap();
        assert!(calibrated.tokens.unwrap() >= 100_000);
        let history = agent.history();

        apply_finished_model_metadata(&mut app, &mut agent).await;

        let expected_context = if effective == ReasoningLevel::Low {
            calibrated
        } else {
            assert_eq!(
                app.info
                    .services
                    .config_repository
                    .load()
                    .unwrap()
                    .reasoning,
                effective
            );
            ContextUsage::estimated(
                estimate_context_tokens(&history, &[]),
                calibrated.context_window,
            )
        };
        assert!(!app.tasks.has_pending());
        assert_eq!(app.model_metadata, Some(metadata));
        assert_eq!(app.info.runtime.reasoning, effective);
        assert_eq!(agent.history(), history);
        assert_eq!(agent.take_context_usage(), Some(expected_context));
    }
}
