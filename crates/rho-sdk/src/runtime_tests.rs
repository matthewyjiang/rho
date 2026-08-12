use std::{
    collections::BTreeMap,
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::sync::Notify;

use crate::{
    model::{
        ContentBlock, ImageContent, Message, ModelEvent, ModelIdentity, ModelRequest,
        ModelResponse, ToolCall, ToolSpec,
    },
    provider::{
        ModelProvider, ProviderEventSender, ProviderFuture, ScriptedProvider, ScriptedTurn,
    },
    tool::{
        ScriptedTool, ScriptedToolOutcome, Tool, ToolContext, ToolError, ToolErrorKind, ToolFuture,
        ToolInvocation, ToolOutput,
    },
    Error, HostChoice, HostInputRequest, HostInputResponse, HostQuestion, ProviderError,
    ProviderErrorKind, Retryability, Rho, RunEvent, SelectionMode, SessionOptions,
    SteeringRetraction, SystemPrompt, UserInput,
};

fn identity() -> ModelIdentity {
    ModelIdentity::new("scripted", "test", "model")
}

#[tokio::test]
async fn simple_completion_and_streaming_share_one_history_path() {
    let provider = ScriptedProvider::new(
        identity(),
        [ScriptedTurn::streaming(
            vec![ModelEvent::OutputDelta("hello".into())],
            ModelResponse::Assistant(vec![ContentBlock::Text("hello".into())]),
        )],
    );
    let runtime = Rho::builder().provider(provider.clone()).build().unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();

    let outcome = session.complete("hi").await.unwrap();

    assert_eq!(outcome.text(), "hello");
    assert_eq!(outcome.revision().get(), 1);
    assert_eq!(
        session.history(),
        [
            Message::user_text("hi"),
            Message::assistant(crate::model::AssistantMessage {
                content: vec![ContentBlock::Text("hello".into())],
                provenance: Some(identity()),
                reasoning_summary: None,
                provider_context: Vec::new(),
            }),
        ]
    );
    assert_eq!(provider.recorded_requests().len(), 1);
}

#[tokio::test]
async fn tool_calls_execute_in_order_and_feed_results_into_the_next_turn() {
    let provider = ScriptedProvider::new(
        identity(),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                ToolCall {
                    id: "call-1".into(),
                    name: "lookup".into(),
                    arguments: json!({"key": "value"}),
                },
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "done".into(),
            )])),
        ],
    );
    let tool = Arc::new(ScriptedTool::new(
        ToolSpec {
            name: "lookup".into(),
            description: "lookup".into(),
            input_schema: json!({"type": "object"}),
        },
        ScriptedToolOutcome::Success(ToolOutput::text("tool output")),
    ));
    let runtime = Rho::builder()
        .provider(provider.clone())
        .tool_shared(tool)
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();

    let outcome = session.complete("use a tool").await.unwrap();

    assert_eq!(outcome.text(), "done");
    let requests = provider.recorded_requests();
    assert_eq!(requests.len(), 2);
    assert!(matches!(
        requests[1].messages.as_slice(),
        [Message::User(_), Message::EnrichedAssistant(_), Message::ToolResult(result)]
            if result.ok && result.content == "tool output"
    ));
}

#[tokio::test]
async fn streaming_exposes_ordered_events_and_typed_final_outcome() {
    let provider = ScriptedProvider::new(
        identity(),
        [ScriptedTurn::streaming(
            vec![
                ModelEvent::OutputDelta("a".into()),
                ModelEvent::OutputDelta("b".into()),
            ],
            ModelResponse::Assistant(vec![ContentBlock::Text("ab".into())]),
        )],
    );
    let runtime = Rho::builder()
        .provider(provider)
        .event_capacity(NonZeroUsize::new(1).unwrap())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("go")).await.unwrap();
    let mut deltas = Vec::new();
    let mut terminal_events = 0;

    while let Some(event) = run.next_event().await {
        match event {
            RunEvent::AssistantTextDelta { text } => deltas.push(text),
            RunEvent::Completed { .. } | RunEvent::Cancelled { .. } | RunEvent::Failed { .. } => {
                terminal_events += 1
            }
            _ => {}
        }
    }
    let outcome = run.outcome().await.unwrap();

    assert_eq!(deltas, ["a", "b"]);
    assert_eq!(terminal_events, 1);
    assert_eq!(outcome.text(), "ab");
}

#[tokio::test]
async fn history_initialization_system_prompt_and_image_input_are_explicit() {
    let provider = ScriptedProvider::new(
        identity(),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("ok".into()),
        ]))],
    );
    let runtime = Rho::builder()
        .provider(provider.clone())
        .system_prompt(SystemPrompt::Custom("custom system".into()))
        .build()
        .unwrap();
    let session = runtime
        .session(SessionOptions::new().history(vec![Message::user_text("prior")]))
        .await
        .unwrap();
    let image = ImageContent {
        data: "aGVsbG8=".into(),
        mime_type: "image/png".into(),
    };

    let mut run = session
        .start(UserInput::text_and_images("describe", [image.clone()]))
        .await
        .unwrap();
    while run.next_event().await.is_some() {}
    run.outcome().await.unwrap();

    assert!(matches!(
        provider.recorded_requests()[0].messages.as_slice(),
        [Message::System(system), Message::User(_), Message::User(content)]
            if system == "custom system"
                && matches!(content.as_slice(), [ContentBlock::Text(_), ContentBlock::Image(value)] if value == &image)
    ));
}

#[tokio::test]
async fn outcome_can_be_awaited_without_consuming_the_event_stream() {
    let provider = ScriptedProvider::new(
        identity(),
        [ScriptedTurn::streaming(
            vec![
                ModelEvent::OutputDelta("a".into()),
                ModelEvent::OutputDelta("b".into()),
                ModelEvent::OutputDelta("c".into()),
            ],
            ModelResponse::Assistant(vec![ContentBlock::Text("abc".into())]),
        )],
    );
    let runtime = Rho::builder()
        .provider(provider)
        .event_capacity(NonZeroUsize::new(1).unwrap())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("go")).await.unwrap();

    let outcome = run.outcome().await.unwrap();

    assert_eq!(outcome.text(), "abc");
}

#[tokio::test]
async fn malformed_provider_responses_are_retried_before_failing() {
    let provider = ScriptedProvider::new(
        identity(),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(Vec::new())),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "recovered".into(),
            )])),
        ],
    );
    let runtime = Rho::builder().provider(provider.clone()).build().unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();

    let outcome = session.complete("retry").await.unwrap();

    assert_eq!(outcome.text(), "recovered");
    assert_eq!(provider.recorded_requests().len(), 2);
}

#[tokio::test]
async fn reset_preserves_prompt_policy_and_provider_replacement_reports_handoff() {
    let source = identity();
    let history = vec![Message::assistant(crate::model::AssistantMessage {
        content: vec![ContentBlock::Text("prior".into())],
        provenance: Some(source.clone()),
        reasoning_summary: None,
        provider_context: vec![crate::model::ProviderContextBlock {
            identity: source,
            kind: "opaque".into(),
            position: None,
            data: json!({"secret": "provider-owned"}),
        }],
    })];
    let runtime = Rho::builder()
        .provider(ScriptedProvider::new(identity(), []))
        .system_prompt(SystemPrompt::Custom("system".into()))
        .build()
        .unwrap();
    let session = runtime
        .session(SessionOptions::new().history(history))
        .await
        .unwrap();
    let replacement: Arc<dyn ModelProvider> = Arc::new(ScriptedProvider::new(
        ModelIdentity::new("other", "test", "model"),
        [],
    ));

    let report = session.replace_provider(replacement).unwrap();
    assert_eq!(report.omitted_provider_context, 1);
    session.reset().unwrap();
    assert_eq!(session.history(), [Message::System("system".into())]);
}

#[tokio::test]
async fn session_snapshot_restores_identity_history_and_revision_without_sqlite() {
    let runtime = Rho::builder()
        .provider(ScriptedProvider::new(
            identity(),
            [ScriptedTurn::completed(ModelResponse::Assistant(vec![
                ContentBlock::Text("first".into()),
            ]))],
        ))
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    session.complete("one").await.unwrap();
    let snapshot = session.snapshot();
    let restored_runtime = Rho::builder()
        .provider(ScriptedProvider::new(
            identity(),
            [ScriptedTurn::completed(ModelResponse::Assistant(vec![
                ContentBlock::Text("second".into()),
            ]))],
        ))
        .build()
        .unwrap();

    let restored = restored_runtime
        .session(SessionOptions::from_snapshot(snapshot.clone()))
        .await
        .unwrap();
    let outcome = restored.complete("two").await.unwrap();

    assert_eq!(restored.id(), snapshot.session_id());
    assert_eq!(outcome.revision(), crate::Revision::from_u64(2));
    assert_eq!(restored.history().len(), 4);
}

#[tokio::test]
async fn snapshot_metadata_survives_restore_and_session_mutations() {
    let expected_metadata = BTreeMap::from([
        ("host".to_string(), "rho-cli".to_string()),
        ("title".to_string(), "metadata restore".to_string()),
    ]);
    let source = crate::SessionSnapshot::new(
        crate::SessionId::from_string("metadata-session").unwrap(),
        crate::Revision::INITIAL,
        vec![Message::user_text("persist me")],
        identity(),
        crate::CompactionState::default(),
    )
    .with_metadata("host", "rho-cli")
    .with_metadata("title", "metadata restore");
    let runtime = Rho::builder()
        .provider(ScriptedProvider::new(identity(), []))
        .compactor(crate::ScriptedCompactor::new([
            crate::CompactionOutput::new(vec![Message::System("summary".into())]).unwrap(),
        ]))
        .build()
        .unwrap();
    let session = runtime
        .session(SessionOptions::from_snapshot(source))
        .await
        .unwrap();

    assert_eq!(session.snapshot().metadata(), &expected_metadata);

    session.reset().unwrap();
    assert_eq!(session.snapshot().metadata(), &expected_metadata);

    session
        .append_message(Message::user_text("compact this"))
        .unwrap();
    session
        .append_message(Message::assistant_text("old response"))
        .unwrap();
    session.compact().await.unwrap();
    assert_eq!(session.snapshot().metadata(), &expected_metadata);

    let replacement_identity = ModelIdentity::new("scripted", "test", "replacement");
    session
        .replace_provider(Arc::new(ScriptedProvider::new(
            replacement_identity.clone(),
            [],
        )))
        .unwrap();
    let final_snapshot = session.snapshot();

    assert_eq!(final_snapshot.metadata(), &expected_metadata);
    assert_eq!(final_snapshot.provider(), &replacement_identity);
}

#[tokio::test]
async fn manual_and_automatic_compaction_use_separate_policy_transport_and_mutation() {
    let manual_runtime = Rho::builder()
        .provider(ScriptedProvider::new(identity(), []))
        .compactor(crate::ScriptedCompactor::new([
            crate::CompactionOutput::new(vec![Message::System("manual summary".into())]).unwrap(),
        ]))
        .build()
        .unwrap();
    let manual_session = manual_runtime
        .session(SessionOptions::new().history(vec![
            Message::user_text("one"),
            Message::assistant_text("two"),
        ]))
        .await
        .unwrap();

    let manual = manual_session.compact().await.unwrap();
    assert_eq!(manual.previous_messages(), 2);
    assert_eq!(manual.current_messages(), 1);
    assert_eq!(
        manual_session
            .snapshot()
            .compaction()
            .completed_compactions(),
        1
    );

    let provider = ScriptedProvider::new(
        identity(),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("done".into()),
        ]))],
    );
    let automatic_runtime = Rho::builder()
        .provider(provider.clone())
        .compactor(crate::ScriptedCompactor::new([
            crate::CompactionOutput::new(vec![
                Message::System("automatic summary".into()),
                Message::user_text("current"),
            ])
            .unwrap(),
        ]))
        .compaction_policy(crate::CompactionPolicy::after_messages(
            NonZeroUsize::new(3).unwrap(),
        ))
        .build()
        .unwrap();
    let automatic_session = automatic_runtime
        .session(SessionOptions::new().history(vec![
            Message::user_text("old one"),
            Message::assistant_text("old two"),
        ]))
        .await
        .unwrap();
    let mut run = automatic_session
        .start(UserInput::text("current"))
        .await
        .unwrap();
    let mut compacted = false;
    while let Some(event) = run.next_event().await {
        if matches!(event, RunEvent::CompactionCompleted { .. }) {
            compacted = true;
        }
    }
    let outcome = run.outcome().await.unwrap();

    assert!(compacted);
    assert_eq!(outcome.revision(), crate::Revision::from_u64(2));
    assert_eq!(
        provider.recorded_requests()[0].messages,
        [
            Message::System("automatic summary".into()),
            Message::user_text("current"),
        ]
    );
    assert_eq!(
        automatic_session.snapshot().compaction().last_revision(),
        Some(crate::Revision::from_u64(1))
    );
}

#[tokio::test]
async fn stream_usage_events_merge_within_a_turn() {
    use crate::model::ModelUsage;

    let provider = ScriptedProvider::new(
        identity(),
        [ScriptedTurn::streaming(
            vec![
                ModelEvent::Usage(ModelUsage {
                    input_tokens: Some(7),
                    cache_read_tokens: Some(3),
                    ..ModelUsage::default()
                }),
                ModelEvent::OutputDelta("hi".into()),
                ModelEvent::Usage(ModelUsage {
                    output_tokens: Some(2),
                    ..ModelUsage::default()
                }),
                ModelEvent::Usage(ModelUsage {
                    output_tokens: Some(3),
                    ..ModelUsage::default()
                }),
            ],
            ModelResponse::Assistant(vec![ContentBlock::Text("hi".into())]),
        )],
    );
    let runtime = Rho::builder().provider(provider).build().unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("hello")).await.unwrap();
    let mut last_usage = None;
    let mut model_call = None;
    while let Some(event) = run.next_event().await {
        match event {
            RunEvent::UsageUpdated { usage } => last_usage = Some(usage),
            RunEvent::ModelCallCompleted { profile, metrics } => {
                model_call = Some((profile, metrics));
            }
            _ => {}
        }
    }
    let outcome = run.outcome().await.unwrap();

    assert_eq!(
        last_usage,
        Some(ModelUsage {
            input_tokens: Some(7),
            output_tokens: Some(5),
            cache_read_tokens: Some(3),
            ..ModelUsage::default()
        })
    );
    assert_eq!(outcome.usage().input_tokens, Some(7));
    assert_eq!(outcome.usage().output_tokens, Some(5));
    assert_eq!(outcome.usage().cache_read_tokens, Some(3));
    let (profile, metrics) = model_call.expect("model call metrics event");
    assert_eq!(
        profile,
        crate::ModelCallProfile {
            provider: "scripted".into(),
            model: "model".into(),
            reasoning: crate::ReasoningLevel::Medium,
            service_tier: None,
        }
    );
    assert_eq!(metrics.output_tokens, Some(5));
    assert!(metrics.time_to_first_token.is_some());
    assert!(metrics.generation_time.is_some());
    assert!(metrics.generation_tokens_per_second().is_some());
    assert!(metrics.response_tokens_per_second().is_some());
}

// Covers: reasoning tokens remain in billable usage but not generation throughput metrics.
// Owner: SDK orchestration
#[tokio::test]
async fn reasoning_breakdown_separates_usage_from_performance_tokens() {
    let provider = ScriptedProvider::new(
        identity(),
        [ScriptedTurn::streaming(
            vec![
                ModelEvent::OutputDelta("done".into()),
                ModelEvent::generation_output_tokens(30),
                ModelEvent::Usage(crate::model::ModelUsage {
                    output_tokens: Some(100),
                    ..crate::model::ModelUsage::default()
                }),
            ],
            ModelResponse::Assistant(vec![ContentBlock::Text("done".into())]),
        )],
    );
    let runtime = Rho::builder().provider(provider).build().unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("hello")).await.unwrap();
    let mut usage = None;
    let mut generation_output_tokens = None;
    let mut carrier_preceded_completion = false;
    let mut metrics = None;
    while let Some(event) = run.next_event().await {
        #[allow(deprecated)]
        match event {
            RunEvent::UsageUpdated { usage: reported } => usage = Some(reported),
            RunEvent::ProviderActivity { kind, detail }
                if kind == crate::PROVIDER_ACTIVITY_GENERATION_OUTPUT_TOKENS =>
            {
                generation_output_tokens = detail.parse().ok();
            }
            RunEvent::ModelCallCompleted {
                metrics: completed, ..
            } => {
                carrier_preceded_completion = generation_output_tokens.is_some();
                metrics = Some(completed);
            }
            _ => {}
        }
    }
    let outcome = run.outcome().await.unwrap();

    assert_eq!(
        usage.as_ref().and_then(|usage| usage.output_tokens),
        Some(100)
    );
    assert_eq!(outcome.usage().output_tokens, Some(100));
    assert_eq!(metrics.and_then(|metrics| metrics.output_tokens), Some(100));
    assert_eq!(generation_output_tokens, Some(30));
    assert!(carrier_preceded_completion);
}

#[derive(Debug)]
struct CompletionOnlyProvider;

impl ModelProvider for CompletionOnlyProvider {
    fn identity(&self) -> ModelIdentity {
        identity()
    }

    fn send_turn<'a>(&'a self, _request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async {
            Ok(ModelResponse::Assistant(vec![ContentBlock::Text(
                "done".into(),
            )]))
        })
    }
}

#[tokio::test]
async fn synthesized_stream_output_does_not_claim_provider_generation_timing() {
    let runtime = Rho::builder()
        .provider(CompletionOnlyProvider)
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("hello")).await.unwrap();
    let mut metrics = None;

    while let Some(event) = run.next_event().await {
        if let RunEvent::ModelCallCompleted {
            metrics: completed, ..
        } = event
        {
            metrics = Some(completed);
        }
    }
    run.outcome().await.unwrap();

    let metrics = metrics.expect("model call metrics event");
    assert_eq!(metrics.time_to_first_token, None);
    assert_eq!(metrics.generation_time, None);
    assert_eq!(metrics.generation_tokens_per_second(), None);
    assert_eq!(metrics.response_tokens_per_second(), None);
}

#[tokio::test]
async fn automatic_compaction_counts_in_flight_history() {
    let provider = ScriptedProvider::new(
        identity(),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("done".into()),
        ]))],
    );
    let automatic_runtime = Rho::builder()
        .provider(provider)
        .compactor(crate::ScriptedCompactor::new([
            crate::CompactionOutput::new(vec![
                Message::System("automatic summary".into()),
                Message::user_text("current"),
            ])
            .unwrap(),
        ]))
        .compaction_policy(crate::CompactionPolicy::after_messages(
            NonZeroUsize::new(3).unwrap(),
        ))
        .build()
        .unwrap();
    // Persisted history is only two messages. The in-flight user message makes
    // the run history three messages before compaction.
    let automatic_session = automatic_runtime
        .session(SessionOptions::new().history(vec![
            Message::user_text("old one"),
            Message::assistant_text("old two"),
        ]))
        .await
        .unwrap();
    let mut run = automatic_session
        .start(UserInput::text("current"))
        .await
        .unwrap();
    let mut previous_messages = None;
    let mut compaction_snapshot = None;
    while let Some(event) = run.next_event().await {
        if let RunEvent::CompactionCompleted { outcome, .. } = event {
            previous_messages = Some(outcome.previous_messages());
            let snapshot = outcome
                .committed_snapshot()
                .expect("automatic compaction snapshot");
            assert_eq!(snapshot.revision(), outcome.revision());
            compaction_snapshot = Some(snapshot.clone());
        }
    }
    run.outcome().await.unwrap();

    assert_eq!(previous_messages, Some(3));
    let compaction_snapshot = compaction_snapshot.expect("compaction snapshot event");
    assert_eq!(
        compaction_snapshot.history(),
        [
            Message::System("automatic summary".into()),
            Message::user_text("current"),
        ]
    );
    assert!(automatic_session.revision() > compaction_snapshot.revision());
    assert_eq!(
        automatic_session.snapshot().compaction().removed_messages(),
        1
    );
}

#[derive(Debug)]
struct QuestionnaireTool;

impl Tool for QuestionnaireTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "questionnaire".into(),
            description: "asks the host".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let question = HostQuestion::new(
                "mode",
                "Which mode?",
                vec![
                    HostChoice::new("fast", "Fast"),
                    HostChoice::new("safe", "Safe"),
                ],
                SelectionMode::One,
            )
            .map_err(|error| ToolError::new(ToolErrorKind::Execution, error.to_string()))?;
            let request = HostInputRequest::questionnaire("choose mode", vec![question])
                .map_err(|error| ToolError::new(ToolErrorKind::Execution, error.to_string()))?;
            let response = context
                .request_host_input(request)
                .await
                .map_err(|error| ToolError::new(ToolErrorKind::Execution, error.to_string()))?;
            Ok(ToolOutput::text(response.answers()["mode"][0].clone()))
        })
    }
}

#[tokio::test]
async fn questionnaire_tool_waits_for_one_valid_typed_host_response() {
    let provider = ScriptedProvider::new(
        identity(),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                ToolCall {
                    id: "question-1".into(),
                    name: "questionnaire".into(),
                    arguments: json!({}),
                },
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "configured".into(),
            )])),
        ],
    );
    let runtime = Rho::builder()
        .provider(provider.clone())
        .tool(QuestionnaireTool)
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("configure")).await.unwrap();
    let request = loop {
        if let RunEvent::ToolHostInputRequested { request, .. } = run.next_event().await.unwrap() {
            break request;
        }
    };

    assert_eq!(session.state(), crate::SessionState::WaitingForHostInput);
    let invalid = HostInputResponse::new().answer("mode", ["unknown"]);
    assert!(run.respond(request.id().clone(), invalid).await.is_err());
    assert_eq!(session.state(), crate::SessionState::WaitingForHostInput);
    run.respond(
        request.id().clone(),
        HostInputResponse::new().answer("mode", ["safe"]),
    )
    .await
    .unwrap();
    assert!(run
        .respond(
            request.id().clone(),
            HostInputResponse::new().answer("mode", ["fast"]),
        )
        .await
        .is_err());
    while run.next_event().await.is_some() {}
    let outcome = run.outcome().await.unwrap();

    assert_eq!(outcome.text(), "configured");
    assert!(matches!(
        &provider.recorded_requests()[1].messages[2],
        Message::ToolResult(result) if result.ok && result.content == "safe"
    ));
}

#[derive(Debug)]
struct SteeringProvider {
    calls: AtomicUsize,
    release_first: Arc<Notify>,
    requests: Mutex<Vec<Vec<Message>>>,
}

impl SteeringProvider {
    fn new(release_first: Arc<Notify>) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            release_first,
            requests: Mutex::new(Vec::new()),
        }
    }
}

impl ModelProvider for SteeringProvider {
    fn identity(&self) -> ModelIdentity {
        identity()
    }

    fn send_turn<'a>(&'a self, _request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async {
            Err(crate::ProviderError::new(
                crate::ProviderErrorKind::Other,
                "streaming path required",
                crate::Retryability::Permanent,
            ))
        })
    }

    fn send_turn_stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
        events: ProviderEventSender,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            self.requests
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(request.messages.to_vec());
            let call = self.calls.fetch_add(1, Ordering::AcqRel);
            if call == 0 {
                events.send(ModelEvent::OutputDelta("draft".into())).await?;
                self.release_first.notified().await;
                Ok(ModelResponse::Assistant(vec![ContentBlock::Text(
                    "draft".into(),
                )]))
            } else {
                Ok(ModelResponse::Assistant(vec![ContentBlock::Text(
                    "final".into(),
                )]))
            }
        })
    }
}

#[tokio::test]
async fn steering_during_provider_stream_is_accepted_and_applied_in_order() {
    let release_first = Arc::new(Notify::new());
    let provider = Arc::new(SteeringProvider::new(Arc::clone(&release_first)));
    let runtime = Rho::builder()
        .provider_shared(provider.clone())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("initial")).await.unwrap();
    while let Some(event) = run.next_event().await {
        if matches!(event, RunEvent::AssistantTextDelta { ref text } if text == "draft") {
            break;
        }
    }

    let first_id = run
        .steer_retractable(UserInput::text("keep first"))
        .await
        .unwrap();
    let discarded_id = run
        .steer_retractable(UserInput::text("discard"))
        .await
        .unwrap();
    assert_eq!(
        run.retract_steering(discarded_id).await.unwrap(),
        SteeringRetraction::Retracted
    );
    let steering_id = run
        .steer_retractable(UserInput::text("refine"))
        .await
        .unwrap();
    assert!(!first_id.as_str().is_empty());
    assert!(!steering_id.as_str().is_empty());
    release_first.notify_one();
    while run.next_event().await.is_some() {}
    let outcome = run.outcome().await.unwrap();

    assert_eq!(outcome.text(), "final");
    let requests = provider
        .requests
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1],
        [
            Message::user_text("initial"),
            Message::assistant(crate::model::AssistantMessage {
                content: vec![ContentBlock::Text("draft".into())],
                provenance: Some(identity()),
                reasoning_summary: None,
                provider_context: Vec::new(),
            }),
            Message::user_text("keep first"),
            Message::user_text("refine"),
        ]
    );
}

// Covers: SteeringHandle clones the command port so hosts can steer after
// moving Run into an event pump.
// Owner: sdk run surface
#[tokio::test]
async fn steering_handle_stages_input_after_run_moves_into_pump() {
    let release_first = Arc::new(Notify::new());
    let provider = Arc::new(SteeringProvider::new(Arc::clone(&release_first)));
    let runtime = Rho::builder()
        .provider_shared(provider.clone())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("initial")).await.unwrap();
    let handle = run.steering_handle();
    while let Some(event) = run.next_event().await {
        if matches!(event, RunEvent::AssistantTextDelta { ref text } if text == "draft") {
            break;
        }
    }

    handle.steer(UserInput::text("via handle")).await.unwrap();
    release_first.notify_one();
    while run.next_event().await.is_some() {}
    let outcome = run.outcome().await.unwrap();
    assert_eq!(outcome.text(), "final");
    let requests = provider
        .requests
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(requests.len(), 2);
    assert!(requests[1].iter().any(|message| {
        matches!(
            message,
            Message::User(blocks)
                if blocks.iter().any(|block| {
                    matches!(block, ContentBlock::Text(text) if text == "via handle")
                })
        )
    }));
}

#[tokio::test]
async fn steering_request_receipt_can_be_polled_while_draining_backpressured_events() {
    let release_first = Arc::new(Notify::new());
    let runtime = Rho::builder()
        .provider(SteeringProvider::new(Arc::clone(&release_first)))
        .event_capacity(NonZeroUsize::new(1).unwrap())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("initial")).await.unwrap();
    tokio::task::yield_now().await;
    let mut receipt = Box::pin(
        run.request_steer_retractable(UserInput::text("refine"))
            .unwrap(),
    );

    assert!(
        tokio::time::timeout(Duration::from_millis(20), receipt.as_mut())
            .await
            .is_err(),
        "the event channel should initially backpressure command processing"
    );
    let id = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            tokio::select! {
                result = receipt.as_mut() => break result.unwrap(),
                event = run.next_event() => assert!(event.is_some()),
            }
        }
    })
    .await
    .expect("draining events should unblock steering acceptance");

    assert!(!id.as_str().is_empty());
    release_first.notify_one();
    while run.next_event().await.is_some() {}
    run.outcome().await.unwrap();
}

#[derive(Debug)]
struct BlockingTool {
    release: Arc<Notify>,
}

impl Tool for BlockingTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "blocking".into(),
            description: "blocks until released".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            self.release.notified().await;
            Ok(ToolOutput::text("released"))
        })
    }
}

#[derive(Debug)]
struct BlockingToolProvider {
    calls: AtomicUsize,
    second_started: Notify,
    release_second: Notify,
    requests: Mutex<Vec<Vec<Message>>>,
}

impl ModelProvider for BlockingToolProvider {
    fn identity(&self) -> ModelIdentity {
        identity()
    }

    fn send_turn<'a>(&'a self, request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async move {
            self.requests
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(request.messages.to_vec());
            if self.calls.fetch_add(1, Ordering::AcqRel) == 0 {
                Ok(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                    ToolCall {
                        id: "blocked-call".into(),
                        name: "blocking".into(),
                        arguments: json!({}),
                    },
                )]))
            } else {
                self.second_started.notify_one();
                self.release_second.notified().await;
                Ok(ModelResponse::Assistant(vec![ContentBlock::Text(
                    "done".into(),
                )]))
            }
        })
    }
}

#[tokio::test]
async fn steering_retraction_during_blocked_tool_is_atomic_and_reports_too_late() {
    let release_tool = Arc::new(Notify::new());
    let provider = Arc::new(BlockingToolProvider {
        calls: AtomicUsize::new(0),
        second_started: Notify::new(),
        release_second: Notify::new(),
        requests: Mutex::new(Vec::new()),
    });
    let runtime = Rho::builder()
        .provider_shared(provider.clone())
        .tool(BlockingTool {
            release: Arc::clone(&release_tool),
        })
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("initial")).await.unwrap();
    while let Some(event) = run.next_event().await {
        if matches!(event, RunEvent::ToolStarted { .. }) {
            break;
        }
    }

    let retracted_id = run
        .steer_retractable(UserInput::text("discard me"))
        .await
        .unwrap();
    assert_eq!(
        run.retract_steering(retracted_id).await.unwrap(),
        SteeringRetraction::Retracted
    );
    let applied_id = run
        .steer_retractable(UserInput::text("keep me"))
        .await
        .unwrap();
    release_tool.notify_one();
    provider.second_started.notified().await;
    assert_eq!(
        run.retract_steering(applied_id.clone()).await.unwrap(),
        SteeringRetraction::AlreadyApplied
    );
    provider.release_second.notify_one();

    let mut observed_applied = false;
    while let Some(event) = run.next_event().await {
        if matches!(
            event,
            RunEvent::SteeringApplied { ref ids } if ids == std::slice::from_ref(&applied_id)
        ) {
            observed_applied = true;
        }
    }
    assert!(observed_applied);
    assert_eq!(run.outcome().await.unwrap().text(), "done");
    let requests = provider
        .requests
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(requests.len(), 2);
    assert!(!requests[1]
        .iter()
        .any(|message| message == &Message::user_text("discard me")));
    assert_eq!(requests[1].last(), Some(&Message::user_text("keep me")));
}

#[derive(Debug)]
struct PartialProvider;

impl ModelProvider for PartialProvider {
    fn identity(&self) -> ModelIdentity {
        identity()
    }

    fn send_turn<'a>(&'a self, request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async move {
            request.cancellation.cancelled().await;
            Err(crate::ProviderError::interrupted("cancelled"))
        })
    }

    fn send_turn_stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
        events: ProviderEventSender,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            events
                .send(ModelEvent::OutputDelta("partial".into()))
                .await?;
            request.cancellation.cancelled().await;
            Err(crate::ProviderError::interrupted("cancelled"))
        })
    }
}

#[tokio::test]
async fn cancellation_recovers_partial_assistant_and_prevents_overlapping_runs() {
    let runtime = Rho::builder().provider(PartialProvider).build().unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("first")).await.unwrap();
    assert!(matches!(
        session.start(UserInput::text("second")).await,
        Err(Error::SessionBusy)
    ));

    loop {
        if matches!(
            run.next_event().await,
            Some(RunEvent::AssistantTextDelta { .. })
        ) {
            break;
        }
    }
    run.cancellation_handle().cancel();
    while run.next_event().await.is_some() {}
    assert!(matches!(run.outcome().await, Err(Error::Cancelled)));

    assert!(!session.is_running());
    assert!(matches!(
        session.history().last(),
        Some(Message::AbortedAssistant(message))
            if matches!(message.content.as_slice(), [ContentBlock::Text(text)] if text == "partial")
    ));
}

#[tokio::test]
async fn cancellation_before_the_first_provider_turn_still_commits_the_user_message() {
    let provider = ScriptedProvider::new(
        identity(),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("unused".into()),
        ]))],
    );
    let runtime = Rho::builder()
        .provider(provider.clone())
        .event_capacity(NonZeroUsize::new(1).unwrap())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();

    // With capacity 1, `Started` fills the channel and the worker blocks on
    // `StepStarted` until cancellation fires, exercising the early-cancel path.
    let mut run = session.start(UserInput::text("hi")).await.unwrap();
    run.cancel();
    let mut saw_cancelled_event = false;
    while let Some(event) = run.next_event().await {
        if matches!(event, RunEvent::Cancelled { .. }) {
            saw_cancelled_event = true;
        }
    }
    let outcome = run.outcome().await;

    assert!(matches!(outcome, Err(Error::Cancelled)));
    assert!(saw_cancelled_event, "expected a terminal Cancelled event");
    assert!(
        session.history().contains(&Message::user_text("hi")),
        "cancelled run should still commit the user message: {:?}",
        session.history()
    );
    assert_eq!(session.revision().get(), 1);
}

// Covers: SSE/provider failure must keep the user turn for resume
// Owner: sdk orchestration
#[tokio::test]
async fn provider_failure_commits_turn_history() {
    let provider = ScriptedProvider::new(
        identity(),
        [
            ScriptedTurn::failed(ProviderError::new(
                ProviderErrorKind::Unavailable,
                "sse disconnect",
                Retryability::Permanent,
            )),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "continued".into(),
            )])),
        ],
    );
    let runtime = Rho::builder().provider(provider.clone()).build().unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();

    let error = session.complete("do work").await.unwrap_err();
    assert!(matches!(error, Error::Provider(_)));
    assert_eq!(error.to_string(), "provider failed: sse disconnect");

    assert_eq!(session.history(), [Message::user_text("do work")]);
    assert_eq!(session.revision().get(), 1);

    assert_eq!(
        session.complete("continue").await.unwrap().text(),
        "continued"
    );
    let requests = provider.recorded_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].messages.last(),
        Some(&Message::user_text("continue"))
    );
    assert!(
        requests[1]
            .messages
            .iter()
            .any(|message| message == &Message::user_text("do work")),
        "follow-up request should still see the failed turn: {:?}",
        requests[1].messages
    );
}

// Covers: failed stream with partial deltas must keep aborted assistant output
// Owner: sdk orchestration
#[tokio::test]
async fn provider_stream_failure_keeps_partial_assistant_in_history() {
    let provider = ScriptedProvider::new(
        identity(),
        [ScriptedTurn::streaming_failed(
            vec![ModelEvent::OutputDelta("partial answer".into())],
            ProviderError::new(
                ProviderErrorKind::Unavailable,
                "stream ended",
                Retryability::Permanent,
            ),
        )],
    );
    let runtime = Rho::builder().provider(provider).build().unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();

    let error = session.complete("write something").await.unwrap_err();
    assert!(matches!(error, Error::Provider(_)));

    assert!(matches!(
        session.history().as_slice(),
        [
            Message::User(_),
            Message::AbortedAssistant(aborted),
        ] if matches!(
            aborted.content.as_slice(),
            [ContentBlock::Text(text)] if text == "partial answer"
        )
    ));
    assert_eq!(session.revision().get(), 1);
}

#[tokio::test]
async fn explicit_shutdown_cancels_active_runs_and_rejects_new_work() {
    let runtime = Rho::builder().provider(PartialProvider).build().unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("wait")).await.unwrap();
    while let Some(event) = run.next_event().await {
        if matches!(event, RunEvent::AssistantTextDelta { .. }) {
            break;
        }
    }

    assert_eq!(runtime.shutdown().cancelled_runs(), 1);
    assert_eq!(runtime.shutdown().cancelled_runs(), 0);
    while run.next_event().await.is_some() {}
    assert!(matches!(run.outcome().await, Err(Error::Cancelled)));
    assert!(matches!(
        session.start(UserInput::text("again")).await,
        Err(Error::RuntimeShutdown)
    ));
    assert!(matches!(
        runtime.session(SessionOptions::default()).await,
        Err(Error::RuntimeShutdown)
    ));
}

#[tokio::test]
async fn dropping_a_run_cancels_work_and_releases_the_session() {
    let runtime = Rho::builder().provider(PartialProvider).build().unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let run = session.start(UserInput::text("first")).await.unwrap();

    drop(run);
    tokio::task::yield_now().await;

    assert!(!session.is_running());
}

#[tokio::test]
async fn reasoning_level_is_explicit_and_can_change_between_runs() {
    let provider = ScriptedProvider::new(
        identity(),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("done".into()),
        ]))],
    );
    let runtime = Rho::builder()
        .provider(provider.clone())
        .reasoning_level(crate::ReasoningLevel::Low)
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();

    assert_eq!(session.reasoning_level(), crate::ReasoningLevel::Low);
    session
        .set_reasoning_level(crate::ReasoningLevel::High)
        .unwrap();
    session.complete("reason").await.unwrap();

    assert_eq!(
        provider.recorded_requests()[0].reasoning_level,
        crate::ReasoningLevel::High
    );
    assert_eq!(
        session.diagnostics().reasoning_level(),
        crate::ReasoningLevel::High
    );
}

#[tokio::test]
async fn service_tier_is_explicit_and_can_change_between_runs() {
    let provider = ScriptedProvider::new(
        identity(),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "standard".into(),
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "fast".into(),
            )])),
        ],
    );
    let runtime = Rho::builder().provider(provider.clone()).build().unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();

    session.complete("standard").await.unwrap();
    session
        .set_service_tier(Some(crate::model::ServiceTier::Priority))
        .unwrap();
    assert_eq!(
        session.diagnostics().service_tier(),
        Some(crate::model::ServiceTier::Priority)
    );
    session.complete("fast").await.unwrap();

    let requests = provider.recorded_requests();
    assert_eq!(requests[0].service_tier, None);
    assert_eq!(
        requests[1].service_tier,
        Some(crate::model::ServiceTier::Priority)
    );
}

// Covers: a provider service-tier fallback must reach hosts as a typed run event.
// Owner: SDK orchestration
#[tokio::test]
async fn provider_service_tier_fallback_is_emitted_without_marking_a_retry() {
    let provider = ScriptedProvider::new(
        identity(),
        [ScriptedTurn::streaming(
            vec![ModelEvent::service_tier_fallback(
                crate::model::ServiceTier::Priority,
                "default",
            )],
            ModelResponse::Assistant(vec![ContentBlock::Text("standard".into())]),
        )],
    );
    let runtime = Rho::builder().provider(provider).build().unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("hello")).await.unwrap();
    let mut fallback = None;
    let mut saw_retry = false;

    while let Some(event) = run.next_event().await {
        match event {
            RunEvent::ProviderServiceTierFallback { requested, used } => {
                fallback = Some((requested, used));
            }
            RunEvent::ProviderRequestRetry => saw_retry = true,
            _ => {}
        }
    }

    assert_eq!(
        fallback,
        Some((crate::model::ServiceTier::Priority, "default".into()))
    );
    assert!(!saw_retry);
}

#[test]
fn diagnostics_are_owned_snapshots_without_prompt_contents_or_global_defaults() {
    let runtime = Rho::builder()
        .provider(ScriptedProvider::new(identity(), []))
        .system_prompt(SystemPrompt::Custom("secret prompt contents".into()))
        .build()
        .unwrap();

    let diagnostics = runtime.diagnostics();

    assert_eq!(diagnostics.provider(), &identity());
    assert_eq!(diagnostics.prompt_sources().len(), 1);
    assert_eq!(
        diagnostics.prompt_sources()[0].label(),
        "custom system prompt"
    );
    assert_eq!(diagnostics.workspace_root(), None);
    assert_eq!(diagnostics.max_parallel_tools(), 1);
    assert!(diagnostics.enabled_features().is_empty());
    assert!(!format!("{diagnostics:?}").contains("secret prompt contents"));
}

#[derive(Debug)]
struct LiveHistoryTool {
    session: Arc<Mutex<Option<crate::Session>>>,
    observed: Arc<Mutex<Vec<Vec<Message>>>>,
    declares: bool,
}

impl Tool for LiveHistoryTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "peek".into(),
            description: "records the conversation it can see".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn reads_live_history(&self) -> bool {
        self.declares
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let session = self
                .session
                .lock()
                .expect("session slot")
                .clone()
                .expect("session is bound before the run starts");
            self.observed
                .lock()
                .expect("observations")
                .push(session.live_history());
            Ok(ToolOutput::text("peeked"))
        })
    }
}

fn scripted_tool_call(id: &str) -> ScriptedTurn {
    ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
        ToolCall {
            id: id.into(),
            name: "peek".into(),
            arguments: json!({}),
        },
    )]))
}

fn assistant_tool_call(id: &str) -> Message {
    Message::assistant(crate::model::AssistantMessage {
        content: vec![ContentBlock::ToolCall(ToolCall {
            id: id.into(),
            name: "peek".into(),
            arguments: json!({}),
        })],
        provenance: Some(identity()),
        reasoning_summary: None,
        provider_context: Vec::new(),
    })
}

// Covers: a tool must see the turn that invoked it, which committed history
// does not contain until the turn ends.
// Owner: session/orchestration conversation state.
#[tokio::test]
async fn live_history_exposes_the_turn_in_flight_to_tools() {
    let session_slot = Arc::new(Mutex::new(None));
    let observed = Arc::new(Mutex::new(Vec::new()));
    let runtime = Rho::builder()
        .provider(ScriptedProvider::new(
            identity(),
            [
                scripted_tool_call("call-1"),
                scripted_tool_call("call-2"),
                ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                    "done".into(),
                )])),
            ],
        ))
        .tool(LiveHistoryTool {
            session: Arc::clone(&session_slot),
            observed: Arc::clone(&observed),
            declares: true,
        })
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    *session_slot.lock().unwrap() = Some(session.clone());

    session.complete("hi").await.unwrap();

    let observed = observed.lock().unwrap().clone();
    assert_eq!(
        observed,
        vec![
            vec![Message::user_text("hi"), assistant_tool_call("call-1")],
            vec![
                Message::user_text("hi"),
                assistant_tool_call("call-1"),
                Message::ToolResult(crate::model::ToolResult {
                    id: "call-1".into(),
                    ok: true,
                    content: "peeked".into(),
                }),
                assistant_tool_call("call-2"),
            ],
        ]
    );
    // The published view is dropped with the run, so later reads see committed
    // history rather than a retired turn.
    assert_eq!(session.live_history(), session.history());
}

// Covers: publishing the turn in flight copies the conversation every tool
// batch, so it must not happen unless a registered tool declares the need.
// Owner: session/orchestration conversation state.
#[tokio::test]
async fn live_history_stays_committed_unless_a_tool_declares_the_need() {
    let session_slot = Arc::new(Mutex::new(None));
    let observed = Arc::new(Mutex::new(Vec::new()));
    let runtime = Rho::builder()
        .provider(ScriptedProvider::new(
            identity(),
            [
                scripted_tool_call("call-1"),
                ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                    "done".into(),
                )])),
            ],
        ))
        .tool(LiveHistoryTool {
            session: Arc::clone(&session_slot),
            observed: Arc::clone(&observed),
            declares: false,
        })
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    *session_slot.lock().unwrap() = Some(session.clone());

    session.complete("hi").await.unwrap();

    let observed = observed.lock().unwrap().clone();
    assert_eq!(observed.len(), 1);
    // Nothing was published, so the tool saw committed history, which cannot
    // contain the assistant turn that invoked it.
    assert!(
        !observed[0].contains(&assistant_tool_call("call-1")),
        "{:?}",
        observed[0]
    );
}

// Covers: hosts that need live approval context must be able to publish the
// turn in flight even when no registered tool declares live-history reads.
// Owner: session/orchestration conversation state.
#[tokio::test]
async fn force_publish_live_history_exposes_the_turn_in_flight() {
    let session_slot = Arc::new(Mutex::new(None));
    let observed = Arc::new(Mutex::new(Vec::new()));
    let runtime = Rho::builder()
        .provider(ScriptedProvider::new(
            identity(),
            [
                scripted_tool_call("call-1"),
                ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                    "done".into(),
                )])),
            ],
        ))
        .force_publish_live_history(true)
        .tool(LiveHistoryTool {
            session: Arc::clone(&session_slot),
            observed: Arc::clone(&observed),
            declares: false,
        })
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    *session_slot.lock().unwrap() = Some(session.clone());

    session.complete("hi").await.unwrap();

    let observed = observed.lock().unwrap().clone();
    assert_eq!(
        observed,
        vec![vec![
            Message::user_text("hi"),
            assistant_tool_call("call-1")
        ]]
    );
    assert_eq!(session.live_history(), session.history());
}
