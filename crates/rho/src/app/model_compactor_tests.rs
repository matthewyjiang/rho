use std::sync::{Arc, Mutex};

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, Message, ModelIdentity, ModelResponse, ModelUsage, ServiceTier},
    provider::{ModelProvider, ScriptedProvider, ScriptedTurn},
    CompactionRequest, Compactor, ProviderError, ProviderErrorKind, ProviderRequestUsageEvent,
    ProviderRequestUsageRecorder, ProviderRequestUsageRecorderFuture,
    ProviderRequestUsageRecording, Retryability,
};

use super::{
    super::runtime_builder::{build_compaction, CompactionSetup},
    ModelCompactor,
};
use crate::{
    compaction::CompactionConfig,
    compaction_metrics::{CompactionRunOutcome, CompactionTier, SummaryRequestPath},
};

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
        tool_specs: Vec::new(),
        reasoning: rho_sdk::ReasoningLevel::Off,
        compaction: CompactionConfig {
            auto_compact: false,
            threshold_percent: 85,
            target_percent: 20,
            summarizer: None,
        },
        context_window,
        usage_recording: ProviderRequestUsageRecording::new(usage),
        diagnostics: seeded_diagnostics(),
        recall: None,
    })
    .0
}

/// Diagnostics that report compaction, as a live runtime's do after its
/// first context refresh.
fn seeded_diagnostics() -> crate::diagnostics::RuntimeDiagnostics {
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
    diagnostics
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
    let tokens = |messages: &[Message]| rho_sdk::model::context::estimate_messages_tokens(messages);
    assert!(tokens(manual.messages()) < tokens(&history));
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
    let diagnostics = seeded_diagnostics();
    let compactor = build_compaction(CompactionSetup {
        provider: Arc::new(provider) as Arc<dyn ModelProvider>,
        tool_specs: Vec::new(),
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
        .and_then(|compaction| compaction.last_compaction)
        .and_then(|record| record.tier)
}

fn summary_provider() -> ScriptedProvider {
    ScriptedProvider::new(
        ModelIdentity::new("opencode-go", "openai-responses", "muse-test"),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("summary text".into()),
        ]))],
    )
}

// Covers: hidden schemas must not shrink the retained tail or force extra
// elision, including when the live projection advertises no tools.
// Owner: ModelCompactor context accounting and tier escalation.
#[tokio::test]
async fn compaction_sizes_partition_and_elision_against_advertised_tools() {
    use rho_sdk::model::context::estimate_context_tokens;

    let (old, mut history) = elision_history();
    history.insert(
        4,
        Message::Assistant(vec![ContentBlock::ToolCall(rho_sdk::model::ToolCall {
            id: "recent-tool".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "recent.rs"}),
        })]),
    );
    history.insert(
        5,
        Message::ToolResult(rho_sdk::model::ToolResult {
            id: "recent-tool".into(),
            ok: true,
            content: old.content[..old.content.len() / 10].to_owned(),
        }),
    );
    let hidden = rho_sdk::model::ToolSpec {
        name: "hidden".into(),
        description: "not advertised".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "description": "h".repeat(old.content.len()),
        }),
    };
    let registry = vec![read_spec(), hidden];
    for (case, advertised, can_recall) in [
        ("subset summary", vec![read_spec()], false),
        ("subset elision", vec![read_spec()], true),
        ("empty summary", Vec::new(), false),
        ("empty elision", Vec::new(), true),
    ] {
        // Twice the retained tail leaves room for the summary reserve, but
        // not the old result. A hidden schema alone exceeds this budget.
        let target = estimate_context_tokens(&history[..2], &advertised)
            + 2 * estimate_context_tokens(&history[4..], &advertised);
        assert!(estimate_context_tokens(&history, &advertised) > target);
        assert!(estimate_context_tokens(&[], &registry) > target);
        let dir = tempfile::tempdir().unwrap();
        let recall = crate::session::recall::RecallStore::default();
        recall.bind(Some(dir.path().join("recall")));
        let provider = summary_provider();
        let (mut compactor, diagnostics) = tiered_compactor(
            provider.clone(),
            RecordingUsage::default(),
            can_recall.then_some(recall),
        );
        compactor.context_window = Some(2 * target);
        compactor.tool_specs = registry.clone();
        let trigger = rho_sdk::CompactionTrigger::Automatic;
        let output = compactor
            .compact(
                CompactionRequest::new(history.clone(), Default::default())
                    .with_trigger(trigger)
                    .with_tool_specs(advertised.clone()),
            )
            .await
            .unwrap();

        let expected_tier = if can_recall {
            // Only the old result changes; the recent tool group stays verbatim.
            let mut restored = output.messages().to_vec();
            restored[3] = Message::ToolResult(old.clone());
            assert_eq!(restored, history, "{case}");
            assert_eq!(provider.recorded_requests(), Vec::new(), "{case}");
            CompactionTier::Elision
        } else {
            let mut expected = history[..2].to_vec();
            expected.push(Message::compaction_summary(trigger, "summary text"));
            expected.extend_from_slice(&history[4..]);
            assert_eq!(output.messages(), expected, "{case}");
            assert_eq!(provider.recorded_requests().len(), 1, "{case}");
            CompactionTier::TextSummary
        };
        let record = diagnostics
            .compaction()
            .and_then(|compaction| compaction.last_compaction)
            .unwrap();
        assert_eq!(
            (
                record.tier,
                record.elided_tool_results,
                record.context_tokens
            ),
            (
                Some(expected_tier),
                usize::from(can_recall),
                estimate_context_tokens(&history, &advertised),
            ),
            "{case}"
        );
    }
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

// Covers: when elision alone misses the target, the session-model summary
// resends the unelided history the provider cached, while a transcript summary
// sees the stubs so a large history cannot overflow it. Diagnostics count
// elided results only when the committed summary was written from them.
// Owner: ModelCompactor tier escalation.
#[tokio::test]
async fn escalation_elides_only_the_transcript_summary() {
    let (old, mut history) = elision_history();
    // A large verbatim user turn keeps the context above target after elision.
    history.insert(4, Message::user_text("u".repeat(40_000)));
    history.insert(5, Message::assistant_text("noted"));
    for (case, configured, sees_stub, elided) in [
        ("session model", false, false, 0),
        ("configured summarizer", true, true, 1),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let recall = crate::session::recall::RecallStore::default();
        recall.bind(Some(dir.path().join("recall")));
        let session = summary_provider();
        let summarizer = summary_provider();
        let (mut compactor, diagnostics) =
            tiered_compactor(session.clone(), RecordingUsage::default(), Some(recall));
        if configured {
            compactor.summarizer = Some(super::Summarizer::with_provider(
                crate::compaction::SummarizerModel {
                    provider: "opencode-go".into(),
                    model: "muse-mini".into(),
                    auth: "api-key".into(),
                    reasoning: rho_sdk::ReasoningLevel::Low,
                },
                Arc::new(summarizer.clone()),
            ));
        }

        compactor
            .compact(CompactionRequest::new(history.clone(), Default::default()))
            .await
            .unwrap();

        let record = diagnostics
            .compaction()
            .and_then(|compaction| compaction.last_compaction)
            .unwrap();
        assert_eq!(
            (record.tier, record.elided_tool_results),
            (Some(CompactionTier::TextSummary), elided),
            "{case}"
        );
        let requests = [session.recorded_requests(), summarizer.recorded_requests()].concat();
        let [request] = requests.as_slice() else {
            panic!(
                "{case}: expected one summary request, got {}",
                requests.len()
            );
        };
        let prompt = serde_json::to_string(&request.messages).unwrap();
        assert_eq!(
            (
                prompt.contains(&crate::compaction::recall_id(&old)),
                prompt.contains(&old.content)
            ),
            (sees_stub, !sees_stub),
            "{case}"
        );
    }
}

fn read_spec() -> rho_sdk::model::ToolSpec {
    rho_sdk::model::ToolSpec {
        name: "read".into(),
        description: "read".into(),
        input_schema: serde_json::json!({"type": "object"}),
    }
}

/// History whose older half needs a text summary under a 1k-token window.
fn summarized_history() -> Vec<Message> {
    vec![
        Message::System("system".into()),
        Message::user_text("do the task"),
        Message::assistant_text("y".repeat(8_000)),
        Message::user_text("recent"),
    ]
}

fn cached_session_request(history: Vec<Message>) -> CompactionRequest {
    CompactionRequest::new(history, Default::default())
        .with_prompt_cache_key("rho:session")
        .with_tool_specs(vec![read_spec()])
        .with_service_tier(ServiceTier::Priority)
}

fn usage_with_cache_reads(cache_read_tokens: u64) -> ModelUsage {
    ModelUsage {
        cache_read_tokens: Some(cache_read_tokens),
        ..ModelUsage::default()
    }
}

fn completed(blocks: Vec<ContentBlock>) -> ScriptedTurn {
    ScriptedTurn::completed(ModelResponse::Assistant(blocks))
}

fn summary_text() -> ScriptedTurn {
    completed(vec![ContentBlock::Text("summary text".into())])
}

fn session_compactor(
    provider: ScriptedProvider,
    usage: RecordingUsage,
    context_window: Option<u64>,
    summarizer: Option<super::Summarizer>,
) -> ModelCompactor {
    let mut compactor = compactor(provider, usage, context_window);
    compactor.reasoning = rho_sdk::ReasoningLevel::High;
    compactor.summarizer = summarizer;
    compactor
}

/// Shape of one recorded summary request, as the cache sees it.
#[derive(Debug, PartialEq)]
struct SentSummary {
    session_history: bool,
    tools: Vec<rho_sdk::model::ToolSpec>,
    reasoning: rho_sdk::ReasoningLevel,
    service_tier: Option<ServiceTier>,
    prompt_cache_key: Option<String>,
}

fn sent(request: &rho_sdk::provider::RecordedModelRequest, history: &[Message]) -> SentSummary {
    let session_history = request.messages.len() == history.len() + 1
        && request.messages[..history.len()] == *history;
    SentSummary {
        session_history,
        tools: request.tools.clone(),
        reasoning: request.reasoning_level,
        service_tier: request.service_tier,
        prompt_cache_key: request.prompt_cache_key.clone(),
    }
}

// Covers: the session-model summary request misses the provider cache because
// its history, tool specs, reasoning, service tier, or cache key differ from
// the session's main requests; every fallback (tool call, provider rejection,
// too large for the window, overflow recovery) still commits a transcript
// summary without executing a tool; output usage sums every request.
// Owner: ModelCompactor text-summary transport.
#[tokio::test]
async fn session_summary_reuses_the_cached_history_and_falls_back_to_the_transcript() {
    let cached = || SentSummary {
        session_history: true,
        tools: vec![read_spec()],
        reasoning: rho_sdk::ReasoningLevel::High,
        service_tier: Some(ServiceTier::Priority),
        prompt_cache_key: Some("rho:session".into()),
    };
    let transcript = || SentSummary {
        session_history: false,
        tools: Vec::new(),
        reasoning: rho_sdk::ReasoningLevel::High,
        service_tier: None,
        prompt_cache_key: None,
    };
    let tool_call = || {
        ScriptedTurn::streaming(
            vec![rho_sdk::model::ModelEvent::Usage(usage_with_cache_reads(
                900,
            ))],
            ModelResponse::Assistant(vec![ContentBlock::ToolCall(rho_sdk::model::ToolCall {
                id: "call".into(),
                name: "read".into(),
                arguments: serde_json::json!({}),
            })]),
        )
    };
    let summary = || {
        ScriptedTurn::streaming(
            vec![rho_sdk::model::ModelEvent::Usage(usage_with_cache_reads(
                100,
            ))],
            ModelResponse::Assistant(vec![ContentBlock::Text("summary text".into())]),
        )
    };
    let failed = |kind| {
        ScriptedTurn::failed(ProviderError::new(
            kind,
            "rejected",
            Retryability::Permanent,
        ))
    };
    struct Case {
        name: &'static str,
        turns: Vec<ScriptedTurn>,
        window: u64,
        trigger: rho_sdk::CompactionTrigger,
        sent: Vec<SentSummary>,
        cache_read_tokens: u64,
        /// The request that wrote the committed summary.
        path: SummaryRequestPath,
    }
    let manual = rho_sdk::CompactionTrigger::Manual;
    let cases = [
        Case {
            name: "session history summary",
            turns: vec![summary()],
            window: 100_000,
            trigger: manual,
            sent: vec![cached()],
            path: SummaryRequestPath::SessionHistory,
            cache_read_tokens: 100,
        },
        Case {
            name: "tool call falls back",
            turns: vec![tool_call(), summary()],
            window: 100_000,
            trigger: manual,
            sent: vec![cached(), transcript()],
            path: SummaryRequestPath::Transcript,
            cache_read_tokens: 1_000,
        },
        Case {
            name: "provider overflow falls back",
            turns: vec![failed(ProviderErrorKind::ContextOverflow), summary()],
            window: 100_000,
            trigger: manual,
            sent: vec![cached(), transcript()],
            path: SummaryRequestPath::Transcript,
            cache_read_tokens: 100,
        },
        Case {
            name: "other provider rejection falls back",
            turns: vec![failed(ProviderErrorKind::InvalidResponse), summary()],
            window: 100_000,
            trigger: manual,
            sent: vec![cached(), transcript()],
            path: SummaryRequestPath::Transcript,
            cache_read_tokens: 100,
        },
        Case {
            name: "history larger than the window",
            turns: vec![summary()],
            window: 2_000,
            trigger: manual,
            sent: vec![transcript()],
            path: SummaryRequestPath::Transcript,
            cache_read_tokens: 100,
        },
        Case {
            name: "overflow recovery",
            turns: vec![summary()],
            window: 100_000,
            trigger: rho_sdk::CompactionTrigger::ContextOverflow,
            sent: vec![transcript()],
            path: SummaryRequestPath::Transcript,
            cache_read_tokens: 100,
        },
    ];

    for case in cases {
        let history = summarized_history();
        let provider = ScriptedProvider::new(
            ModelIdentity::new("anthropic", "anthropic-messages", "claude-test"),
            case.turns,
        );
        let usage = RecordingUsage::default();
        let compactor = session_compactor(provider.clone(), usage.clone(), Some(case.window), None);
        let output = compactor
            .compact(cached_session_request(history.clone()).with_trigger(case.trigger))
            .await
            .unwrap();

        let requests = provider.recorded_requests();
        assert_eq!(
            requests
                .iter()
                .map(|request| sent(request, &history))
                .collect::<Vec<_>>(),
            case.sent,
            "{}",
            case.name
        );
        assert_eq!(
            output.messages(),
            [
                history[0].clone(),
                history[1].clone(),
                Message::compaction_summary(case.trigger, "summary text"),
                history[3].clone(),
            ],
            "{}",
            case.name
        );
        assert_eq!(
            output.usage().cache_read_tokens,
            Some(case.cache_read_tokens),
            "{}",
            case.name
        );
        let events = usage.events();
        assert_eq!(
            events
                .iter()
                .map(|event| (event.context().purpose(), event.context().attempt_index()))
                .collect::<Vec<_>>(),
            (1..=requests.len())
                .map(|index| ("compaction", Some(index)))
                .collect::<Vec<_>>(),
            "{}",
            case.name
        );
        let record = compactor
            .diagnostics
            .compaction()
            .and_then(|compaction| compaction.last_compaction)
            .unwrap();
        assert_eq!(
            (
                record.outcome,
                record.tier,
                record.request_path,
                record.model.as_deref(),
                record.cache_read_tokens,
            ),
            (
                CompactionRunOutcome::Completed,
                Some(CompactionTier::TextSummary),
                Some(case.path),
                Some("anthropic/claude-test"),
                Some(case.cache_read_tokens),
            ),
            "{}",
            case.name
        );
    }
}

// Covers: a configured summarizer receives the rendered transcript on its own
// provider and reasoning, with no session service tier. A failure falls back
// to the cached session-history request. Overflow recovery skips that request
// and uses the transcript, still without the session service tier.
// Owner: ModelCompactor summarizer routing.
#[tokio::test]
async fn configured_summarizer_gets_the_transcript_and_falls_back_on_failure() {
    let session_identity = ModelIdentity::new("anthropic", "anthropic-messages", "claude-test");
    let summarizer_identity = ModelIdentity::new("openai", "openai-responses", "gpt-mini");
    let transcript = |reasoning| SentSummary {
        session_history: false,
        tools: Vec::new(),
        reasoning,
        service_tier: None,
        prompt_cache_key: None,
    };
    let cached = || SentSummary {
        session_history: true,
        tools: vec![read_spec()],
        reasoning: rho_sdk::ReasoningLevel::High,
        service_tier: Some(ServiceTier::Priority),
        prompt_cache_key: Some("rho:session".into()),
    };
    let rejected = || {
        ScriptedTurn::failed(ProviderError::new(
            ProviderErrorKind::Authentication,
            "bad key",
            Retryability::Permanent,
        ))
    };
    for (case, trigger, summarizer_turns, session_turns, expected) in [
        (
            "summarizer answers",
            rho_sdk::CompactionTrigger::Manual,
            vec![summary_text()],
            vec![],
            vec![(
                summarizer_identity.clone(),
                transcript(rho_sdk::ReasoningLevel::Low),
            )],
        ),
        (
            "summarizer fails",
            rho_sdk::CompactionTrigger::Manual,
            vec![rejected()],
            vec![summary_text()],
            vec![
                (
                    summarizer_identity.clone(),
                    transcript(rho_sdk::ReasoningLevel::Low),
                ),
                (session_identity.clone(), cached()),
            ],
        ),
        (
            "summarizer fails during overflow recovery",
            rho_sdk::CompactionTrigger::ContextOverflow,
            vec![rejected()],
            vec![summary_text()],
            vec![
                (
                    summarizer_identity.clone(),
                    transcript(rho_sdk::ReasoningLevel::Low),
                ),
                (
                    session_identity.clone(),
                    transcript(rho_sdk::ReasoningLevel::High),
                ),
            ],
        ),
    ] {
        let history = summarized_history();
        let session = ScriptedProvider::new(session_identity.clone(), session_turns);
        let summarizer = ScriptedProvider::new(summarizer_identity.clone(), summarizer_turns);
        let usage = RecordingUsage::default();
        let compactor = session_compactor(
            session.clone(),
            usage.clone(),
            Some(100_000),
            Some(super::Summarizer::with_provider(
                crate::compaction::SummarizerModel {
                    provider: "openai".into(),
                    model: "gpt-mini".into(),
                    auth: "api-key".into(),
                    reasoning: rho_sdk::ReasoningLevel::Low,
                },
                Arc::new(summarizer.clone()),
            )),
        );

        compactor
            .compact(cached_session_request(history.clone()).with_trigger(trigger))
            .await
            .unwrap();

        let requests = [summarizer.recorded_requests(), session.recorded_requests()];
        let identities = [summarizer_identity.clone(), session_identity.clone()];
        let recorded = requests
            .iter()
            .zip(&identities)
            .flat_map(|(requests, identity)| {
                requests
                    .iter()
                    .map(|request| (identity.clone(), sent(request, &history)))
            })
            .collect::<Vec<_>>();
        assert_eq!(recorded, expected, "{case}");
        assert_eq!(
            usage
                .events()
                .iter()
                .map(|event| (
                    event.context().identity().clone(),
                    event.context().attempt_index()
                ))
                .collect::<Vec<_>>(),
            expected
                .iter()
                .enumerate()
                .map(|(index, (identity, _))| (identity.clone(), Some(index + 1)))
                .collect::<Vec<_>>(),
            "{case}"
        );
    }
}

// Covers: a summary request that fails after streaming usage still charges the
// compaction record, even though the committed output only carries the usage
// of the plan that succeeded.
// Owner: ModelCompactor compaction metrics.
#[tokio::test]
async fn failed_summary_request_usage_is_charged_to_the_compaction_record() {
    let history = summarized_history();
    let provider = ScriptedProvider::new(
        ModelIdentity::new("anthropic", "anthropic-messages", "claude-test"),
        [
            ScriptedTurn::streaming_failed(
                vec![rho_sdk::model::ModelEvent::Usage(usage_with_cache_reads(
                    900,
                ))],
                ProviderError::new(
                    ProviderErrorKind::InvalidResponse,
                    "rejected",
                    Retryability::Permanent,
                ),
            ),
            ScriptedTurn::streaming(
                vec![rho_sdk::model::ModelEvent::Usage(usage_with_cache_reads(
                    100,
                ))],
                ModelResponse::Assistant(vec![ContentBlock::Text("summary text".into())]),
            ),
        ],
    );
    let usage = RecordingUsage::default();
    let compactor = session_compactor(provider, usage.clone(), Some(100_000), None);
    let output = compactor
        .compact(cached_session_request(history))
        .await
        .unwrap();

    assert_eq!(output.usage().cache_read_tokens, Some(100));
    let record = compactor
        .diagnostics
        .compaction()
        .and_then(|compaction| compaction.last_compaction)
        .unwrap();
    assert_eq!(record.request_path, Some(SummaryRequestPath::Transcript));
    assert_eq!(record.cache_read_tokens, Some(1_000));
}
