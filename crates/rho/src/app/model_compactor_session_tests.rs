use std::sync::Arc;

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, Message, ModelIdentity, ModelResponse, ModelUsage, ServiceTier},
    provider::{ScriptedProvider, ScriptedTurn},
    CompactionRequest, Compactor, ProviderError, ProviderErrorKind, Retryability,
};

use super::{
    tests::{compactor, RecordingUsage},
    ModelCompactor,
};
use crate::compaction_metrics::{CompactionRunOutcome, CompactionTier, SummaryRequestPath};

pub(super) fn read_spec() -> rho_sdk::model::ToolSpec {
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
