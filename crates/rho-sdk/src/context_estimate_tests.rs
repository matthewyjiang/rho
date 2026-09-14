use std::{
    num::NonZeroU64,
    sync::{Arc, Mutex},
};

use pretty_assertions::assert_eq;
use serde_json::json;

use crate::{
    model::{
        context::{estimate_context_tokens, estimate_message_tokens},
        ContentBlock, Message, ModelEvent, ModelIdentity, ModelResponse, ModelUsage, ToolCall,
        ToolSpec,
    },
    provider::{ScriptedProvider, ScriptedTurn},
    tool::{ScriptedTool, ScriptedToolOutcome, ToolOutput},
    CompactionFuture, CompactionOutput, CompactionPolicy, CompactionRequest, Compactor,
    ContextEstimate, Rho, SessionOptions,
};

fn identity() -> ModelIdentity {
    ModelIdentity::new("scripted", "test", "context")
}

fn turn(prompt: u64, response: Vec<ContentBlock>) -> ScriptedTurn {
    ScriptedTurn::streaming(
        vec![ModelEvent::Usage(ModelUsage {
            input_tokens: Some(prompt / 2),
            cache_read_tokens: Some(prompt / 4),
            cache_write_tokens: Some(prompt - prompt / 2 - prompt / 4),
            output_tokens: Some(7),
            context_window: Some(131_072),
            ..ModelUsage::default()
        })],
        ModelResponse::Assistant(response),
    )
}

#[derive(Clone, Default)]
struct RecordingCompactor(Arc<Mutex<Vec<CompactionRequest>>>);

impl Compactor for RecordingCompactor {
    fn compact<'a>(&'a self, request: CompactionRequest) -> CompactionFuture<'a> {
        self.0.lock().unwrap().push(request);
        Box::pin(async { CompactionOutput::new(vec![Message::user_text("summary")]) })
    }
}

// Covers: an unchanged compact result drops a valid provider baseline, making
// context appear below threshold until another usage report. Owner: SDK lifecycle.
#[tokio::test]
async fn context_unchanged_compaction_preserves_provider_baseline() {
    struct UnchangedCompactor;
    impl Compactor for UnchangedCompactor {
        fn compact<'a>(&'a self, request: CompactionRequest) -> CompactionFuture<'a> {
            Box::pin(async move { CompactionOutput::new(request.messages().to_vec()) })
        }
    }

    for trigger in [
        crate::CompactionTrigger::Manual,
        crate::CompactionTrigger::Automatic,
    ] {
        let provider = ScriptedProvider::new(
            identity(),
            [
                turn(100_000, vec![ContentBlock::Text("first".into())]),
                ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                    "second".into(),
                )])),
            ],
        );
        let runtime = Rho::builder()
            .provider(provider)
            .compactor(UnchangedCompactor)
            .compaction_policy(CompactionPolicy::at_context_tokens(
                NonZeroU64::new(90_000).unwrap(),
            ))
            .build()
            .unwrap();
        let session = runtime.session(SessionOptions::default()).await.unwrap();
        session.complete("first").await.unwrap();
        match trigger {
            crate::CompactionTrigger::Manual => {
                session.compact().await.unwrap();
            }
            crate::CompactionTrigger::Automatic => {
                session.complete("second").await.unwrap();
            }
        }
        let estimate = session.context_estimate();
        assert_eq!(
            estimate.provider_reported_tokens(),
            Some(100_000),
            "{trigger:?}"
        );
        assert_eq!(
            estimate.estimated_tokens(),
            estimate_context_tokens(&session.history(), &[])
        );
        assert!(estimate.tokens() >= 100_000, "{trigger:?}");
    }
}

// Covers: chars/4 misses provider-sized prompts, cache tokens are omitted, or
// cumulative run usage triggers repeated compaction after a smaller request.
// Owner: SDK orchestration, across tool steps and separate user runs.
#[tokio::test]
async fn context_usage_drives_compaction_across_steps_and_runs() {
    for tool_steps in [false, true] {
        let response = |index| {
            if tool_steps && index < 2 {
                vec![ContentBlock::ToolCall(ToolCall {
                    id: format!("call-{index}"),
                    name: "read".into(),
                    arguments: json!({}),
                })]
            } else {
                vec![ContentBlock::Text("done".into())]
            }
        };
        let provider = ScriptedProvider::new(
            identity(),
            [
                turn(100_000, response(0)),
                turn(60_000, response(1)),
                turn(50_000, response(2)),
            ],
        );
        let compactor = RecordingCompactor::default();
        let runtime = Rho::builder()
            .provider(provider.clone())
            .tool(ScriptedTool::new(
                ToolSpec {
                    name: "read".into(),
                    description: "read".into(),
                    input_schema: json!({"type": "object"}),
                },
                ScriptedToolOutcome::Success(ToolOutput::text("tool output")),
            ))
            .compactor(compactor.clone())
            .compaction_policy(CompactionPolicy::at_context_tokens(
                NonZeroU64::new(90_000).unwrap(),
            ))
            .build()
            .unwrap();
        let session = runtime.session(SessionOptions::default()).await.unwrap();
        session.complete("first").await.unwrap();
        if !tool_steps {
            session.complete("second").await.unwrap();
            session.complete("third").await.unwrap();
        }
        let requests = compactor.0.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let estimate = requests[0].context_estimate().unwrap();
        assert_eq!(estimate.provider_reported_tokens(), Some(100_000));
        assert!(estimate.estimated_tokens() < 90_000);
        assert!(estimate.tokens() >= 100_000);
        assert_eq!(
            session.context_estimate().provider_reported_tokens(),
            Some(50_000)
        );
        assert_eq!(
            session.last_compaction_decision().unwrap().skip_reason(),
            Some(crate::CompactionSkipReason::BelowThreshold)
        );
        assert_eq!(provider.recorded_requests().len(), 3);
    }
}

// Covers: successful normal completion/host append loses calibration, replacement
// accidentally reuses it, or resume trusts a baseline whose request was not restored.
// Owner: SDK session lifecycle. The compaction test above owns the policy gate.
#[tokio::test]
async fn context_baseline_survives_append_but_not_replacement_or_resume() {
    let provider = ScriptedProvider::new(
        identity(),
        (0..5).map(|_| turn(1_000, vec![ContentBlock::Text("done".into())])),
    );
    let runtime = Rho::builder().provider(provider.clone()).build().unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    session.complete("first").await.unwrap();
    let request = &provider.recorded_requests()[0];
    let initial = session.context_estimate();
    let assistant = session.history().last().unwrap().clone();
    assert_eq!(
        initial,
        ContextEstimate {
            estimated_tokens: estimate_context_tokens(&session.history(), &request.tools),
            provider_reported_tokens: Some(1_000),
            provider_request_estimated_tokens: Some(estimate_context_tokens(
                &request.messages,
                &request.tools
            )),
            reported_context_window: Some(131_072),
        }
    );
    assert_eq!(
        initial.tokens(),
        1_000 + estimate_message_tokens(&assistant)
    );
    let appended = Message::user_text("appended context");
    let mut proposed = session.history();
    proposed.push(appended.clone());
    let pending = session.estimate_context(&proposed);
    assert_eq!(
        pending.tokens(),
        initial.tokens() + estimate_message_tokens(&appended)
    );
    session.append_message(appended).unwrap();
    assert_eq!(session.context_estimate(), pending);
    let resumed = runtime
        .session(SessionOptions::from_snapshot(session.snapshot()))
        .await
        .unwrap();
    assert_eq!(
        resumed.context_estimate(),
        ContextEstimate::from_estimated_tokens(pending.estimated_tokens())
    );

    // Even same-count, same-size edits must not pass prefix validation.
    proposed[0] = Message::user_text("other");
    assert_eq!(
        session
            .estimate_context(&proposed)
            .provider_reported_tokens(),
        None
    );
    session.replace_history(session.history()).unwrap();
    assert_eq!(session.context_estimate().provider_reported_tokens(), None);
    session.complete("new report").await.unwrap();
    session
        .replace_provider(Arc::new(provider.clone()))
        .unwrap();
    assert_eq!(session.context_estimate().provider_reported_tokens(), None);
    session.complete("new report").await.unwrap();
    session.reset().unwrap();
    assert_eq!(
        session.context_estimate(),
        ContextEstimate::from_estimated_tokens(estimate_context_tokens(&session.history(), &[]))
    );
}

// Covers: a failed or malformed request pollutes the baseline with provisional usage.
// Owner: SDK accepted-response boundary. Retry stream reset behavior is tested elsewhere.
#[tokio::test]
async fn context_baseline_ignores_failed_and_invalid_attempts() {
    let failed = ScriptedTurn::streaming_failed(
        vec![ModelEvent::Usage(ModelUsage {
            input_tokens: Some(99_000),
            ..ModelUsage::default()
        })],
        crate::ProviderError::new(
            crate::ProviderErrorKind::Other,
            "failed",
            crate::Retryability::Permanent,
        ),
    );
    let malformed = turn(88_000, Vec::new());
    let provider = ScriptedProvider::new(
        identity(),
        [
            turn(1_000, vec![ContentBlock::Text("first".into())]),
            failed,
            malformed,
            turn(2_000, vec![ContentBlock::Text("accepted".into())]),
        ],
    );
    let runtime = Rho::builder().provider(provider).build().unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    session.complete("first").await.unwrap();
    assert!(session.complete("failed").await.is_err());
    assert_eq!(
        session.context_estimate().provider_reported_tokens(),
        Some(1_000)
    );
    session.complete("retry invalid").await.unwrap();
    assert_eq!(
        session.context_estimate().provider_reported_tokens(),
        Some(2_000)
    );
}

// Covers: a compactor sizes a protected suffix into its prefix calibration or
// loses verbatim completion-boundary input while compacting a provider-sized prompt.
// Owner: SDK checkpoint/compaction contract; existing boundary tests cover ordering.
#[tokio::test]
async fn context_compaction_estimate_excludes_protected_boundary_input() {
    let provider = ScriptedProvider::new(
        identity(),
        [
            turn(100_000, vec![ContentBlock::Text("candidate".into())]),
            turn(10_000, vec![ContentBlock::Text("done".into())]),
        ],
    );
    let compactor = RecordingCompactor::default();
    let runtime = Rho::builder()
        .provider(provider.clone())
        .compactor(compactor.clone())
        .compaction_policy(CompactionPolicy::at_context_tokens(
            NonZeroU64::new(90_000).unwrap(),
        ))
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let (source, mut requests) = crate::boundary_input_channel();
    session.set_boundary_inputs(Some(source)).unwrap();
    let mut pending = Some(crate::UserInput::text("protected input".repeat(100)));
    let fresh = Message::User(pending.as_ref().unwrap().blocks().to_vec());
    let mut run = session.start(crate::UserInput::text("work")).await.unwrap();
    loop {
        tokio::select! {
            Some(request) = requests.recv() => {
                let input = if request.boundary() == crate::InputBoundary::BeforeCompletion { pending.take() } else { None };
                request.respond(input).await;
            }
            event = run.next_event() => if event.is_none() { break; },
        }
    }
    run.outcome().await.unwrap();
    let requests = compactor.0.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    let estimate = request.context_estimate().unwrap();
    assert_eq!(
        estimate.estimated_tokens(),
        estimate_context_tokens(request.messages(), &[])
    );
    assert_eq!(estimate.provider_reported_tokens(), Some(100_000));
    assert_eq!(
        provider.recorded_requests()[1].messages,
        vec![Message::user_text("summary"), fresh]
    );
}

// Covers: unit conversion rounds up, increases an already conservative budget,
// or overflows for large measured values. Owner: pure sizing arithmetic.
#[test]
fn context_estimated_budget_is_conservative() {
    for (raw, reported, current, target, expected) in [
        (100, 200, 100, 100, 50),
        (100, 200, 300, 100, 50),
        (200, 100, 200, 100, 100),
        (3, 10, 3, 9, 2),
        (0, 100, 0, 100, 0),
        (u64::MAX / 2, u64::MAX, u64::MAX / 2, u64::MAX, u64::MAX / 2),
    ] {
        let estimate = ContextEstimate {
            estimated_tokens: current,
            provider_reported_tokens: Some(reported),
            provider_request_estimated_tokens: Some(raw),
            reported_context_window: None,
        };
        assert_eq!(estimate.estimated_budget(target), expected);
    }
}

// Covers: changing a tool schema or the request identity reuses an incompatible
// calibration even when local token counts happen to match. Owner: request anchor.
#[test]
fn context_anchor_validates_tools_identity_and_prefix() {
    let history = vec![Message::user_text("hello")];
    let tools = vec![ToolSpec {
        name: "read".into(),
        description: "read".into(),
        input_schema: json!({"type": "object"}),
    }];
    let mut accounting = super::ContextAccounting::new(&history, &tools);
    accounting.record(
        &history,
        &tools,
        identity(),
        &ModelUsage {
            input_tokens: Some(1_000),
            ..ModelUsage::default()
        },
        accounting.current(),
    );
    let mut changed_tools = tools.clone();
    changed_tools[0].description = "edit".into();
    for (messages, specs, model) in [
        (vec![Message::user_text("other")], tools.clone(), identity()),
        (history.clone(), changed_tools, identity()),
        (
            history.clone(),
            tools.clone(),
            ModelIdentity::new("scripted", "test", "other"),
        ),
        (Vec::new(), tools.clone(), identity()),
    ] {
        let estimate = accounting.estimate(&messages, &specs, &model);
        assert_eq!(
            estimate,
            ContextEstimate::from_estimated_tokens(estimate_context_tokens(&messages, &specs))
        );
    }
}

struct CancelledUsageProvider;

impl crate::provider::ModelProvider for CancelledUsageProvider {
    fn identity(&self) -> ModelIdentity {
        identity()
    }

    fn send_turn<'a>(
        &'a self,
        _request: crate::model::ModelRequest<'a>,
    ) -> crate::provider::ProviderFuture<'a> {
        Box::pin(std::future::pending())
    }

    fn send_turn_stream<'a>(
        &'a self,
        request: crate::model::ModelRequest<'a>,
        events: crate::provider::ProviderEventSender,
    ) -> crate::provider::ProviderFuture<'a> {
        Box::pin(async move {
            let first = request.messages.len() == 1;
            events
                .send(ModelEvent::Usage(ModelUsage {
                    input_tokens: Some(if first { 1_000 } else { 99_000 }),
                    ..ModelUsage::default()
                }))
                .await?;
            if first {
                Ok(ModelResponse::Assistant(vec![ContentBlock::Text(
                    "done".into(),
                )]))
            } else {
                std::future::pending().await
            }
        })
    }
}

// Covers: cancellation after usage delivery must not replace the last successful
// baseline; visible provisional usage remains untrusted. Owner: SDK run cancellation.
#[tokio::test]
async fn context_cancelled_usage_does_not_replace_successful_baseline() {
    let runtime = Rho::builder()
        .provider(CancelledUsageProvider)
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    session.complete("first").await.unwrap();
    let mut run = session
        .start(crate::UserInput::text("cancelled"))
        .await
        .unwrap();
    while let Some(event) = run.next_event().await {
        if matches!(event, crate::RunEvent::UsageUpdated { .. }) {
            assert_eq!(
                session.context_estimate().provider_reported_tokens(),
                Some(1_000)
            );
            run.cancel();
        }
    }
    assert!(matches!(run.outcome().await, Err(crate::Error::Cancelled)));
    let estimate = session.context_estimate();
    assert_eq!(estimate.provider_reported_tokens(), Some(1_000));
    assert_eq!(
        estimate.estimated_tokens(),
        estimate_context_tokens(&session.history(), &[])
    );
}
