use std::sync::{Arc, Mutex};

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, Message, ModelIdentity, ModelResponse, ModelUsage, ServiceTier},
    provider::{ModelProvider, ScriptedProvider, ScriptedTurn},
    CompactionRequest, Compactor, ProviderError, ProviderErrorKind, ProviderRequestUsageEvent,
    ProviderRequestUsageRecorder, ProviderRequestUsageRecorderFuture,
    ProviderRequestUsageRecording, Retryability,
};

use super::{build_compaction, CompactionSetup, ModelCompactor};
use crate::{compaction::CompactionConfig, diagnostics::CompactionTier};

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
    build_compaction(CompactionSetup {
        provider: Arc::new(provider) as Arc<dyn ModelProvider>,
        tools: &[],
        reasoning: rho_sdk::ReasoningLevel::Off,
        compaction: CompactionConfig {
            auto_compact: false,
            threshold_percent: 85,
            target_percent: 20,
        },
        context_window,
        usage_recording: ProviderRequestUsageRecording::new(usage),
        diagnostics: crate::diagnostics::test_diagnostics("test", "test"),
        recall: None,
    })
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

// Covers: native compaction receives the session service tier so xAI fast mode
// can stamp the request model without rebuilding the provider.
// Owner: compaction runtime
#[tokio::test]
async fn native_compaction_forwards_the_service_tier() {
    let usage = RecordingUsage::default();
    let provider = ScriptedProvider::new(
        ModelIdentity::new("xai", "openai-responses", "grok-4.7"),
        [],
    )
    .with_native_compactions([Ok(rho_sdk::CompactionOutput::new(vec![
        Message::user_text("kept"),
    ])
    .unwrap())]);
    let compactor = compactor(provider.clone(), usage, None);

    compactor
        .compact(
            CompactionRequest::new(messages(), Default::default())
                .with_service_tier(ServiceTier::Priority),
        )
        .await
        .unwrap();

    assert_eq!(
        provider.recorded_requests()[0].service_tier,
        Some(ServiceTier::Priority)
    );
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

/// A 1M-window model with a 175k-token session: automatic compaction has
/// nothing to do, but an explicit `/compact` must still remove history.
#[tokio::test]
async fn manual_trigger_summarizes_below_automatic_target() {
    let history = vec![
        Message::System("system".into()),
        Message::user_text("x".repeat(8_000)),
        Message::assistant_text("y".repeat(8_000)),
        Message::user_text("recent"),
    ];
    let summarized = |messages: &[Message]| {
        messages.iter().any(|message| {
            matches!(
                message,
                Message::User(blocks) if blocks.iter().any(|block| matches!(
                    block,
                    ContentBlock::Text(text) if text.contains("summary text")
                ))
            )
        })
    };
    let provider = || {
        ScriptedProvider::new(
            ModelIdentity::new("opencode-go", "openai-responses", "muse-test"),
            [ScriptedTurn::completed(ModelResponse::Assistant(vec![
                ContentBlock::Text("summary text".into()),
            ]))],
        )
    };

    let automatic = compactor(provider(), RecordingUsage::default(), Some(1_000_000))
        .compact(
            CompactionRequest::new(history.clone(), Default::default())
                .with_trigger(rho_sdk::CompactionTrigger::Automatic),
        )
        .await
        .unwrap();
    assert_eq!(automatic.messages(), history.as_slice());

    let manual = compactor(provider(), RecordingUsage::default(), Some(1_000_000))
        .compact(CompactionRequest::new(history.clone(), Default::default()))
        .await
        .unwrap();
    assert!(summarized(manual.messages()));
    assert!(manual.messages().len() < history.len());
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

fn elision_history() -> (rho_sdk::model::ToolResult, Vec<Message>) {
    let old = rho_sdk::model::ToolResult {
        id: "old".into(),
        ok: true,
        content: "x".repeat(40_000),
    };
    let history = vec![
        Message::System("system".into()),
        Message::user_text("read it"),
        Message::Assistant(vec![ContentBlock::ToolCall(rho_sdk::model::ToolCall {
            id: "old".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "big.rs"}),
        })]),
        Message::ToolResult(old.clone()),
        Message::user_text("recent"),
        Message::assistant_text("ok"),
    ];
    (old, history)
}

fn tiered_compactor(
    provider: ScriptedProvider,
    usage: RecordingUsage,
    recall: Option<crate::session::recall::RecallStore>,
) -> (ModelCompactor, crate::diagnostics::RuntimeDiagnostics) {
    let diagnostics = crate::diagnostics::test_diagnostics("test", "test");
    diagnostics.record_compaction_context(
        crate::diagnostics::CompactionContext::new(
            rho_sdk::ContextEstimate::from_estimated_tokens(0),
            None,
            &CompactionConfig::default(),
        ),
        None,
        Default::default(),
    );
    let compactor = build_compaction(CompactionSetup {
        provider: Arc::new(provider) as Arc<dyn ModelProvider>,
        tools: &[],
        reasoning: rho_sdk::ReasoningLevel::Off,
        compaction: CompactionConfig::default(),
        context_window: Some(1_000_000),
        usage_recording: ProviderRequestUsageRecording::new(usage),
        diagnostics: diagnostics.clone(),
        recall,
    })
    .0;
    (compactor, diagnostics)
}

fn last_tier(diagnostics: &crate::diagnostics::RuntimeDiagnostics) -> Option<CompactionTier> {
    diagnostics
        .compaction()
        .and_then(|compaction| compaction.last_tier)
        .map(|report| report.tier)
}

fn summary_provider() -> ScriptedProvider {
    ScriptedProvider::new(
        ModelIdentity::new("opencode-go", "openai-responses", "muse-test"),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("summary text".into()),
        ]))],
    )
}

// Covers: elision that reaches the target commits without a model request,
// records the tier, and saves the original so the stub is recallable.
// Owner: ModelCompactor tier escalation.
#[tokio::test]
async fn elision_that_reaches_target_skips_the_model_and_saves_originals() {
    let (old, history) = elision_history();
    let dir = tempfile::tempdir().unwrap();
    let recall = crate::session::recall::RecallStore::default();
    recall.bind(Some(dir.path().join("recall")));
    let usage = RecordingUsage::default();
    let (compactor, diagnostics) = tiered_compactor(
        ScriptedProvider::new(
            ModelIdentity::new("opencode-go", "openai-responses", "muse-test"),
            Vec::<ScriptedTurn>::new(),
        ),
        usage.clone(),
        Some(recall),
    );

    let compacted = compactor
        .compact(CompactionRequest::new(history.clone(), Default::default()))
        .await
        .unwrap();

    assert_eq!(usage.events(), Vec::new());
    assert_eq!(last_tier(&diagnostics), Some(CompactionTier::Elision));
    let recall_id = crate::compaction::recall_id(&old);
    assert!(matches!(
        &compacted.messages()[3],
        Message::ToolResult(stub) if stub.id == "old" && stub.content.contains(&recall_id)
    ));
    let saved: rho_sdk::model::ToolResult = serde_json::from_slice(
        &std::fs::read(dir.path().join("recall").join(format!("{recall_id}.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(saved, old);
}

// Covers: when stubs could not be recalled (no `sessions` tool, or no durable
// session bound), compaction skips elision and summarizes the original text.
// Owner: ModelCompactor tier gating.
#[tokio::test]
async fn elision_is_skipped_when_results_cannot_be_recalled() {
    let (_, history) = elision_history();
    let cases = [
        ("no sessions tool", None),
        ("unbound session", Some(Default::default())),
    ];

    for (case, recall) in cases {
        let usage = RecordingUsage::default();
        let (compactor, diagnostics) = tiered_compactor(summary_provider(), usage.clone(), recall);

        compactor
            .compact(CompactionRequest::new(history.clone(), Default::default()))
            .await
            .unwrap();

        assert_eq!(usage.events().len(), 1, "{case}");
        assert_eq!(
            last_tier(&diagnostics),
            Some(CompactionTier::TextSummary),
            "{case}"
        );
    }
}

// Covers: when elision alone misses the target, the summarizer receives the
// elided history (stubs, not the original output).
// Owner: ModelCompactor tier escalation.
#[tokio::test]
async fn escalation_summarizes_the_elided_history() {
    let (old, mut history) = elision_history();
    // A large verbatim user turn keeps the context above target after elision.
    history.insert(4, Message::user_text("u".repeat(40_000)));
    history.insert(5, Message::assistant_text("noted"));
    let dir = tempfile::tempdir().unwrap();
    let recall = crate::session::recall::RecallStore::default();
    recall.bind(Some(dir.path().join("recall")));
    let provider = summary_provider();
    let (compactor, diagnostics) =
        tiered_compactor(provider.clone(), RecordingUsage::default(), Some(recall));

    compactor
        .compact(CompactionRequest::new(history, Default::default()))
        .await
        .unwrap();

    assert_eq!(last_tier(&diagnostics), Some(CompactionTier::TextSummary));
    let requests = provider.recorded_requests();
    let [request] = requests.as_slice() else {
        panic!("expected one summary request, got {}", requests.len());
    };
    let prompt = serde_json::to_string(&request.messages).unwrap();
    assert!(prompt.contains(&crate::compaction::recall_id(&old)));
    assert!(!prompt.contains(&old.content));
}
