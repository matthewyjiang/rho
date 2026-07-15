use std::{
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
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
    Error, HostChoice, HostInputRequest, HostInputResponse, HostQuestion, Rho, RunEvent,
    SelectionMode, SessionOptions, SystemPrompt, UserInput,
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
    let tool = ScriptedTool::new(
        ToolSpec {
            name: "lookup".into(),
            description: "lookup".into(),
            input_schema: json!({"type": "object"}),
        },
        ScriptedToolOutcome::Success(ToolOutput::text("tool output")),
    );
    let runtime = Rho::builder()
        .provider(provider.clone())
        .tool(tool)
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
        if let RunEvent::HostInputRequested { request } = run.next_event().await.unwrap() {
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

    run.steer(UserInput::text("refine")).await.unwrap();
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
            Message::user_text("refine"),
        ]
    );
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
    assert!(diagnostics.enabled_features().is_empty());
    assert!(!format!("{diagnostics:?}").contains("secret prompt contents"));
}

#[test]
fn construction_rejects_missing_provider_and_duplicate_tools() {
    assert!(matches!(
        Rho::builder().build(),
        Err(Error::InvalidConfiguration { .. })
    ));
    let first = ScriptedTool::new(
        ToolSpec {
            name: "same".into(),
            description: "first".into(),
            input_schema: json!({}),
        },
        ScriptedToolOutcome::Success(ToolOutput::text("first")),
    );
    let second = ScriptedTool::new(
        ToolSpec {
            name: "same".into(),
            description: "second".into(),
            input_schema: json!({}),
        },
        ScriptedToolOutcome::Success(ToolOutput::text("second")),
    );

    assert!(matches!(
        Rho::builder()
            .provider(ScriptedProvider::new(identity(), []))
            .tool(first)
            .tool(second)
            .build(),
        Err(Error::InvalidConfiguration { .. })
    ));
}
