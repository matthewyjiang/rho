use std::sync::{Arc, Mutex};

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, Message, ModelIdentity, ModelResponse, ModelUsage},
    provider::{ModelProvider, ScriptedProvider, ScriptedTurn},
    CompactionRequest, Compactor, ProviderError, ProviderErrorKind, ProviderRequestUsageEvent,
    ProviderRequestUsageRecorder, ProviderRequestUsageRecorderFuture,
    ProviderRequestUsageRecording, Retryability,
};

use super::{build_compaction, ModelCompactor};
use crate::compaction::CompactionConfig;

#[derive(Clone, Default)]
struct RecordingUsage {
    events: Arc<Mutex<Vec<ProviderRequestUsageEvent>>>,
}

impl RecordingUsage {
    fn events(&self) -> Vec<ProviderRequestUsageEvent> {
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl ProviderRequestUsageRecorder for RecordingUsage {
    fn record(&self, event: ProviderRequestUsageEvent) -> ProviderRequestUsageRecorderFuture<'_> {
        let events = Arc::clone(&self.events);
        Box::pin(async move {
            events
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(event);
            Ok(())
        })
    }
}

fn messages() -> Vec<Message> {
    vec![
        Message::System("system".into()),
        Message::user_text("hello"),
        Message::assistant_text("world"),
    ]
}

fn compactor(
    provider: ScriptedProvider,
    usage: RecordingUsage,
    context_window: Option<u64>,
) -> ModelCompactor {
    build_compaction(
        Arc::new(provider) as Arc<dyn ModelProvider>,
        &[],
        rho_sdk::ReasoningLevel::Off,
        CompactionConfig {
            auto_compact: false,
            threshold_percent: 85,
            target_percent: 20,
        },
        context_window,
        ProviderRequestUsageRecording::new(usage),
    )
    .0
}

#[tokio::test]
async fn native_compaction_success_records_usage_and_returns_replacement() {
    let usage = RecordingUsage::default();
    let replacement = vec![Message::System("system".into()), Message::user_text("kept")];
    let provider = ScriptedProvider::new(
        ModelIdentity::new("openai", "openai-responses", "gpt-test"),
        [],
    )
    .with_native_compactions([Ok(rho_sdk::CompactionOutput::with_usage(
        replacement.clone(),
        ModelUsage {
            input_tokens: Some(11),
            output_tokens: Some(2),
            total_tokens: Some(13),
            ..ModelUsage::default()
        },
    )
    .unwrap())]);
    let compactor = compactor(provider.clone(), usage.clone(), Some(8_000));

    let output = compactor
        .compact(CompactionRequest::new(messages(), Default::default()))
        .await
        .unwrap();

    assert_eq!(output.messages(), replacement.as_slice());
    assert_eq!(output.usage().input_tokens, Some(11));
    let events = usage.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].usage().input_tokens, Some(11));
    assert_eq!(events[0].context().attempt_index(), Some(1));
    assert!(matches!(
        events[0].outcome(),
        rho_sdk::ProviderRequestOutcome::Completed
    ));
    assert!(provider
        .recorded_requests()
        .iter()
        .all(|request| request.tools.is_empty() && request.prompt_cache_key.is_none()));
}

#[tokio::test]
async fn native_compaction_failure_falls_back_to_summary_path() {
    let usage = RecordingUsage::default();
    let provider = ScriptedProvider::new(
        ModelIdentity::new("openai", "openai-responses", "gpt-test"),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("summary text".into()),
        ]))],
    )
    .with_native_compactions([Err(ProviderError::new(
        ProviderErrorKind::Unavailable,
        "compact unavailable",
        Retryability::Retryable,
    ))]);
    let compactor = compactor(provider.clone(), usage.clone(), Some(1_000));
    let history = vec![
        Message::System("system".into()),
        Message::user_text("x".repeat(8_000)),
        Message::assistant_text("y".repeat(8_000)),
        Message::user_text("recent"),
    ];

    let output = compactor
        .compact(CompactionRequest::new(history, Default::default()))
        .await
        .unwrap();

    assert!(output.messages().iter().any(|message| {
        matches!(
            message,
            Message::User(blocks) if blocks.iter().any(|block| matches!(
                block,
                ContentBlock::Text(text) if text.contains("summary text")
            ))
        )
    }));
    let events = usage.events();
    assert!(events.len() >= 2);
    assert_eq!(events[0].context().attempt_index(), Some(1));
    assert!(matches!(
        events[0].outcome(),
        rho_sdk::ProviderRequestOutcome::Failed(ProviderErrorKind::Unavailable)
    ));
    assert_eq!(
        events.last().unwrap().context().attempt_index(),
        Some(events.len())
    );
    assert!(matches!(
        events.last().unwrap().outcome(),
        rho_sdk::ProviderRequestOutcome::Completed
    ));
    // First request is native compact with empty tools and no invented cache key.
    assert!(provider.recorded_requests()[0].tools.is_empty());
    assert!(provider.recorded_requests()[0].prompt_cache_key.is_none());
}

#[tokio::test]
async fn native_compaction_auth_retry_keeps_monotonic_attempt_indexes() {
    use rho_sdk::{
        model::ModelUsage,
        provider::{NativeCompactionFailedAttempt, NativeCompactionResponse},
    };

    let usage = RecordingUsage::default();
    let provider = ScriptedProvider::new(
        ModelIdentity::new("openai", "openai-responses", "gpt-test"),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("summary text".into()),
        ]))],
    )
    .with_native_compactions([NativeCompactionResponse::failure(ProviderError::new(
        ProviderErrorKind::Unavailable,
        "compact unavailable after refresh",
        Retryability::Retryable,
    ))
    .with_failed_attempts([NativeCompactionFailedAttempt::new(
        ProviderErrorKind::Authentication,
        ModelUsage::default(),
    )])]);
    let compactor = compactor(provider, usage.clone(), Some(1_000));
    let history = vec![
        Message::System("system".into()),
        Message::user_text("x".repeat(8_000)),
        Message::assistant_text("y".repeat(8_000)),
        Message::user_text("recent"),
    ];

    let output = compactor
        .compact(CompactionRequest::new(history, Default::default()))
        .await
        .unwrap();
    assert!(output.messages().iter().any(|message| {
        matches!(
            message,
            Message::User(blocks) if blocks.iter().any(|block| matches!(
                block,
                ContentBlock::Text(text) if text.contains("summary text")
            ))
        )
    }));

    let events = usage.events();
    assert!(events.len() >= 3);
    assert_eq!(events[0].context().attempt_index(), Some(1));
    assert!(matches!(
        events[0].outcome(),
        rho_sdk::ProviderRequestOutcome::Failed(ProviderErrorKind::Authentication)
    ));
    assert_eq!(events[1].context().attempt_index(), Some(2));
    assert!(matches!(
        events[1].outcome(),
        rho_sdk::ProviderRequestOutcome::Failed(ProviderErrorKind::Unavailable)
    ));
    assert_eq!(
        events.last().unwrap().context().attempt_index(),
        Some(events.len())
    );
    assert!(matches!(
        events.last().unwrap().outcome(),
        rho_sdk::ProviderRequestOutcome::Completed
    ));
}

#[tokio::test]
async fn native_compaction_cancellation_is_explicit() {
    let usage = RecordingUsage::default();
    let provider = ScriptedProvider::new(
        ModelIdentity::new("openai", "openai-responses", "gpt-test"),
        [],
    )
    .with_native_compactions([Err(ProviderError::interrupted("cancelled"))]);
    let compactor = compactor(provider, usage.clone(), Some(8_000));
    let cancellation = rho_sdk::CancellationToken::new();
    cancellation.cancel();

    let error = compactor
        .compact(CompactionRequest::new(messages(), cancellation))
        .await
        .unwrap_err();

    assert!(matches!(error, rho_sdk::Error::Cancelled));
    let events = usage.events();
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0].outcome(),
        rho_sdk::ProviderRequestOutcome::Cancelled
    ));
}
