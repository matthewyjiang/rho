use std::{
    future::{pending, poll_fn},
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    task::Poll,
    time::Duration,
};

use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::sync::mpsc;

use crate::{
    model::{
        ContentBlock, Message, ModelEvent, ModelIdentity, ModelRequest, ModelResponse, ModelUsage,
        ToolCall, ToolResult, ToolSpec,
    },
    provider::{ModelProvider, ProviderFuture, ScriptedProvider, ScriptedTurn},
    session::{RunStart, SessionCore},
    steering::SteeringQueue,
    tool::{
        Tool, ToolContext, ToolError, ToolErrorKind, ToolFuture, ToolInvocation, ToolMetadata,
        ToolOutput, ToolProgress,
    },
    CancellationToken, CompactionState, Error, HostChoice, HostInputRequest, HostQuestion,
    ProviderError, ProviderErrorKind, Retryability, Revision, Rho, Run, RunEvent, RunId,
    SelectionMode, Session, SessionId, SessionOptions, SessionState, UserInput,
};

use super::{
    apply_staged_steering, execute_run, tool_turn::INTERRUPTED_TOOL_RESULT_CONTENT, valid_response,
};

const TEST_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone)]
struct StrictContinuationProvider {
    first_response: Vec<ContentBlock>,
    calls: Arc<AtomicUsize>,
}

impl StrictContinuationProvider {
    fn new(calls: Vec<ToolCall>) -> Self {
        Self {
            first_response: calls.into_iter().map(ContentBlock::ToolCall).collect(),
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl ModelProvider for StrictContinuationProvider {
    fn identity(&self) -> ModelIdentity {
        ModelIdentity::new("strict", "test", "continuation")
    }

    fn send_turn<'a>(&'a self, request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async move {
            if request.cancellation.is_cancelled() {
                return Err(ProviderError::interrupted("provider request cancelled"));
            }
            if self.calls.fetch_add(1, Ordering::AcqRel) == 0 {
                return Ok(ModelResponse::Assistant(self.first_response.clone()));
            }
            validate_tool_result_pairs(request.messages)?;
            Ok(ModelResponse::Assistant(vec![ContentBlock::Text(
                "continued".into(),
            )]))
        })
    }
}

fn validate_tool_result_pairs(messages: &[Message]) -> Result<(), ProviderError> {
    for (index, message) in messages.iter().enumerate() {
        let Some(content) = message.completed_assistant_content() else {
            continue;
        };
        let expected = content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolCall(call) => Some(call.id.as_str()),
                ContentBlock::Text(_) | ContentBlock::Image(_) => None,
            })
            .collect::<Vec<_>>();
        if expected.is_empty() {
            continue;
        }
        let actual = messages[index + 1..]
            .iter()
            .take_while(|message| matches!(message, Message::ToolResult(_)))
            .filter_map(|message| match message {
                Message::ToolResult(result) => Some(result.id.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        if actual != expected {
            return Err(ProviderError::new(
                ProviderErrorKind::InvalidResponse,
                format!("tool results {actual:?} do not close calls {expected:?}"),
                Retryability::Permanent,
            ));
        }
    }
    Ok(())
}

fn tool_call(id: &str, name: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: json!({}),
    }
}

fn tool_spec(name: &str) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: "test tool".into(),
        input_schema: json!({"type": "object"}),
    }
}

#[derive(Clone)]
struct MetadataBlockedTool {
    name: &'static str,
    metadata_reached: Arc<AtomicBool>,
    called: Arc<AtomicBool>,
}

impl Tool for MetadataBlockedTool {
    fn spec(&self) -> ToolSpec {
        tool_spec(self.name)
    }

    fn start_metadata(&self, _arguments: &serde_json::Value) -> ToolMetadata {
        self.metadata_reached.store(true, Ordering::Release);
        ToolMetadata::default()
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            self.called.store(true, Ordering::Release);
            pending::<Result<ToolOutput, ToolError>>().await
        })
    }
}

#[derive(Clone)]
struct PendingTool {
    name: &'static str,
    called: Arc<AtomicBool>,
}

impl Tool for PendingTool {
    fn spec(&self) -> ToolSpec {
        tool_spec(self.name)
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            self.called.store(true, Ordering::Release);
            pending::<Result<ToolOutput, ToolError>>().await
        })
    }
}

#[derive(Clone)]
struct ImmediateTool {
    name: &'static str,
    called: Arc<AtomicBool>,
}

impl Tool for ImmediateTool {
    fn spec(&self) -> ToolSpec {
        tool_spec(self.name)
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            self.called.store(true, Ordering::Release);
            Ok(ToolOutput::text("completed"))
        })
    }
}

#[derive(Clone)]
struct CompletingProgressTool {
    name: &'static str,
    progress_sent: Arc<AtomicBool>,
    completion_ready: Arc<AtomicBool>,
}

impl Tool for CompletingProgressTool {
    fn spec(&self) -> ToolSpec {
        tool_spec(self.name)
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            if !context
                .progress()
                .send(ToolProgress::message("progress"))
                .await
            {
                return Err(ToolError::cancelled());
            }
            self.progress_sent.store(true, Ordering::Release);
            poll_fn(|_context| {
                if self.completion_ready.load(Ordering::Acquire) {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await;
            Ok(ToolOutput::text("progress-complete"))
        })
    }
}

#[derive(Clone)]
struct HostInputTool {
    name: &'static str,
}

impl Tool for HostInputTool {
    fn spec(&self) -> ToolSpec {
        tool_spec(self.name)
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let question = HostQuestion::new(
                "continue",
                "continue?",
                vec![HostChoice::new("yes", "Yes")],
                SelectionMode::One,
            )
            .map_err(|error| ToolError::new(ToolErrorKind::Execution, error.to_string()))?;
            let request = HostInputRequest::questionnaire("input required", vec![question])
                .map_err(|error| ToolError::new(ToolErrorKind::Execution, error.to_string()))?;
            match context.request_host_input(request).await {
                Ok(_) => Ok(ToolOutput::text("answered")),
                Err(Error::Cancelled) => Err(ToolError::cancelled()),
                Err(error) => Err(ToolError::new(ToolErrorKind::Execution, error.to_string())),
            }
        })
    }
}

async fn next_event(run: &mut Run) -> RunEvent {
    tokio::time::timeout(TEST_TIMEOUT, run.next_event())
        .await
        .expect("run event timed out")
        .expect("run event stream closed")
}

async fn wait_for_flag(flag: &AtomicBool) {
    tokio::time::timeout(TEST_TIMEOUT, async {
        while !flag.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("test probe timed out");
}

fn interrupted_result(id: &str) -> ToolResult {
    ToolResult {
        id: id.into(),
        ok: false,
        content: INTERRUPTED_TOOL_RESULT_CONTENT.into(),
    }
}

fn session_tool_results(session: &Session) -> Vec<ToolResult> {
    session
        .history()
        .into_iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some(result),
            _ => None,
        })
        .collect()
}

async fn cancel_and_continue(session: &Session, run: &mut Run, expected_results: Vec<ToolResult>) {
    run.cancel();
    let outcome = tokio::time::timeout(TEST_TIMEOUT, run.outcome())
        .await
        .expect("cancelled run timed out");
    assert!(matches!(outcome, Err(Error::Cancelled)), "{outcome:?}");
    assert_eq!(session_tool_results(session), expected_results);

    let outcome = tokio::time::timeout(TEST_TIMEOUT, session.complete("continue"))
        .await
        .expect("strict continuation timed out")
        .expect("strict provider rejected replay history");
    assert_eq!(outcome.text(), "continued");
}

#[tokio::test]
async fn cancelled_applied_event_keeps_steering_out_of_history() {
    let cancellation = CancellationToken::new();
    let (events, _receiver) = mpsc::channel(1);
    events
        .send(RunEvent::StepStarted { step: 1 })
        .await
        .unwrap();
    let mut steering = SteeringQueue::new();
    let id = steering.accept(UserInput::text("pending"));
    let mut history = Vec::new();
    cancellation.cancel();

    let result = apply_staged_steering(&mut steering, &mut history, &events, &cancellation).await;

    assert!(matches!(result, Err(Error::Cancelled)));
    assert!(history.is_empty());
    assert_eq!(steering.staged_ids(), vec![id]);
}

#[tokio::test]
async fn cancellation_before_the_first_tool_interrupts_every_unresolved_call() {
    let provider = StrictContinuationProvider::new(vec![tool_call("first", "blocked")]);
    let metadata_reached = Arc::new(AtomicBool::new(false));
    let called = Arc::new(AtomicBool::new(false));
    let runtime = Rho::builder()
        .provider(provider)
        .tool(MetadataBlockedTool {
            name: "blocked",
            metadata_reached: Arc::clone(&metadata_reached),
            called: Arc::clone(&called),
        })
        .event_capacity(NonZeroUsize::new(1).unwrap())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("start")).await.unwrap();

    assert!(matches!(
        next_event(&mut run).await,
        RunEvent::Started { .. }
    ));
    assert!(matches!(
        next_event(&mut run).await,
        RunEvent::StepStarted { .. }
    ));
    wait_for_flag(&metadata_reached).await;
    assert!(!called.load(Ordering::Acquire));

    cancel_and_continue(&session, &mut run, vec![interrupted_result("first")]).await;
}

#[tokio::test]
async fn cancellation_during_a_tool_interrupts_the_current_call() {
    let provider = StrictContinuationProvider::new(vec![tool_call("pending", "pending")]);
    let called = Arc::new(AtomicBool::new(false));
    let runtime = Rho::builder()
        .provider(provider)
        .tool(PendingTool {
            name: "pending",
            called: Arc::clone(&called),
        })
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("start")).await.unwrap();

    loop {
        if matches!(next_event(&mut run).await, RunEvent::ToolStarted { .. }) {
            break;
        }
    }
    wait_for_flag(&called).await;

    cancel_and_continue(&session, &mut run, vec![interrupted_result("pending")]).await;
}

#[tokio::test]
async fn cancellation_between_tools_preserves_completed_results() {
    let provider = StrictContinuationProvider::new(vec![
        tool_call("completed", "immediate"),
        tool_call("unresolved", "blocked"),
    ]);
    let immediate_called = Arc::new(AtomicBool::new(false));
    let metadata_reached = Arc::new(AtomicBool::new(false));
    let blocked_called = Arc::new(AtomicBool::new(false));
    let runtime = Rho::builder()
        .provider(provider)
        .tool(ImmediateTool {
            name: "immediate",
            called: Arc::clone(&immediate_called),
        })
        .tool(MetadataBlockedTool {
            name: "blocked",
            metadata_reached: Arc::clone(&metadata_reached),
            called: Arc::clone(&blocked_called),
        })
        .event_capacity(NonZeroUsize::new(1).unwrap())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("start")).await.unwrap();

    loop {
        if matches!(
            next_event(&mut run).await,
            RunEvent::ToolFinished { ref call_id, .. } if call_id.as_str() == "completed"
        ) {
            break;
        }
    }
    wait_for_flag(&metadata_reached).await;
    assert!(immediate_called.load(Ordering::Acquire));
    assert!(!blocked_called.load(Ordering::Acquire));

    cancel_and_continue(
        &session,
        &mut run,
        vec![
            ToolResult {
                id: "completed".into(),
                ok: true,
                content: "completed".into(),
            },
            interrupted_result("unresolved"),
        ],
    )
    .await;
}

#[tokio::test]
async fn cancellation_during_progress_preserves_a_completed_tool_and_interrupts_the_rest() {
    let provider = StrictContinuationProvider::new(vec![
        tool_call("progress", "progress"),
        tool_call("later", "later"),
    ]);
    let progress_sent = Arc::new(AtomicBool::new(false));
    let completion_ready = Arc::new(AtomicBool::new(false));
    let later_metadata = Arc::new(AtomicBool::new(false));
    let later_called = Arc::new(AtomicBool::new(false));
    let runtime = Rho::builder()
        .provider(provider)
        .tool(CompletingProgressTool {
            name: "progress",
            progress_sent: Arc::clone(&progress_sent),
            completion_ready: Arc::clone(&completion_ready),
        })
        .tool(MetadataBlockedTool {
            name: "later",
            metadata_reached: Arc::clone(&later_metadata),
            called: Arc::clone(&later_called),
        })
        .event_capacity(NonZeroUsize::new(1).unwrap())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("start")).await.unwrap();

    loop {
        if matches!(
            next_event(&mut run).await,
            RunEvent::ToolUpdated { ref call_id, .. } if call_id.as_str() == "progress"
        ) {
            break;
        }
    }
    assert!(progress_sent.load(Ordering::Acquire));
    completion_ready.store(true, Ordering::Release);

    cancel_and_continue(
        &session,
        &mut run,
        vec![
            ToolResult {
                id: "progress".into(),
                ok: true,
                content: "progress-complete".into(),
            },
            interrupted_result("later"),
        ],
    )
    .await;
    // Preparation resolves metadata for every registered call before the batch starts.
    assert!(later_metadata.load(Ordering::Acquire));
    assert!(!later_called.load(Ordering::Acquire));
}

#[tokio::test]
async fn cancellation_while_awaiting_host_input_interrupts_the_call() {
    let provider = StrictContinuationProvider::new(vec![tool_call("input", "host_input")]);
    let runtime = Rho::builder()
        .provider(provider)
        .tool(HostInputTool { name: "host_input" })
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("start")).await.unwrap();

    loop {
        if matches!(
            next_event(&mut run).await,
            RunEvent::HostInputRequested { .. }
        ) {
            break;
        }
    }

    cancel_and_continue(&session, &mut run, vec![interrupted_result("input")]).await;
}

#[tokio::test]
async fn simple_completion_host_input_cancellation_remains_replay_safe() {
    let provider = StrictContinuationProvider::new(vec![tool_call("input", "host_input")]);
    let runtime = Rho::builder()
        .provider(provider)
        .tool(HostInputTool { name: "host_input" })
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();

    let error = tokio::time::timeout(TEST_TIMEOUT, session.complete("start"))
        .await
        .expect("simple completion timed out")
        .unwrap_err();
    assert!(matches!(error, Error::InvalidHostResponse { .. }));
    assert_eq!(
        session_tool_results(&session),
        vec![interrupted_result("input")]
    );

    let outcome = session.complete("continue").await.unwrap();
    assert_eq!(outcome.text(), "continued");
}

#[derive(Clone)]
struct FlakyProvider {
    failures: usize,
    retryability: Retryability,
    calls: Arc<AtomicUsize>,
}

impl ModelProvider for FlakyProvider {
    fn identity(&self) -> ModelIdentity {
        ModelIdentity::new("flaky", "test", "flaky")
    }

    fn send_turn<'a>(&'a self, _request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async move {
            if self.calls.fetch_add(1, Ordering::AcqRel) < self.failures {
                return Err(ProviderError::new(
                    ProviderErrorKind::Unavailable,
                    "stream broke",
                    self.retryability,
                ));
            }
            Ok(ModelResponse::Assistant(vec![ContentBlock::Text(
                "recovered".into(),
            )]))
        })
    }
}

async fn flaky_session(failures: usize, retryability: Retryability) -> (Arc<AtomicUsize>, Session) {
    let calls = Arc::new(AtomicUsize::new(0));
    let session = Rho::builder()
        .provider(FlakyProvider {
            failures,
            retryability,
            calls: Arc::clone(&calls),
        })
        .build()
        .unwrap()
        .session(SessionOptions::default())
        .await
        .unwrap();
    (calls, session)
}

/// Generous virtual-time bound so paused-clock backoff sleeps always fire
/// before an event wait gives up.
async fn next_event_virtual(run: &mut Run) -> RunEvent {
    tokio::time::timeout(Duration::from_secs(60), run.next_event())
        .await
        .expect("run event timed out")
        .expect("run event stream closed")
}

#[tokio::test(start_paused = true)]
async fn retryable_provider_failures_are_retried() {
    let (calls, session) = flaky_session(2, Retryability::Retryable).await;
    let mut run = session.start(UserInput::text("start")).await.unwrap();

    let mut retries = 0;
    let outcome = loop {
        match next_event_virtual(&mut run).await {
            RunEvent::ProviderStreamReset {
                reason: crate::ProviderStreamResetReason::RetryableFailure(_),
                ..
            } => {
                retries += 1;
            }
            RunEvent::Completed { outcome } => break outcome,
            RunEvent::Failed { message, .. } => panic!("run failed: {message}"),
            _ => {}
        }
    };
    assert_eq!(outcome.text(), "recovered");
    assert_eq!(retries, 2);
    assert_eq!(calls.load(Ordering::Acquire), 3);
}

#[tokio::test(start_paused = true)]
async fn retry_after_partial_stream_resets_output_and_reuses_request_history() {
    let failed_usage = ModelUsage {
        output_tokens: Some(3),
        ..ModelUsage::default()
    };
    let recovered_usage = ModelUsage {
        output_tokens: Some(5),
        ..ModelUsage::default()
    };
    let provider = ScriptedProvider::new(
        ModelIdentity::new("streaming-flaky", "test", "streaming-flaky"),
        [
            ScriptedTurn::streaming_failed(
                vec![
                    ModelEvent::OutputDelta("stale partial".into()),
                    ModelEvent::Usage(failed_usage.clone()),
                ],
                ProviderError::new(
                    ProviderErrorKind::InvalidResponse,
                    "provider stream failed after emitting output",
                    Retryability::Retryable,
                ),
            ),
            ScriptedTurn::streaming(
                vec![
                    ModelEvent::OutputDelta("recovered".into()),
                    ModelEvent::Usage(recovered_usage.clone()),
                ],
                ModelResponse::Assistant(vec![ContentBlock::Text("recovered".into())]),
            ),
        ],
    );
    let session = Rho::builder()
        .provider(provider.clone())
        .build()
        .unwrap()
        .session(SessionOptions::default())
        .await
        .unwrap();
    let mut run = session.start(UserInput::text("start")).await.unwrap();

    let mut displayed = String::new();
    let mut resets = Vec::new();
    let mut usages = Vec::new();
    let outcome = loop {
        match next_event_virtual(&mut run).await {
            RunEvent::AssistantTextDelta { text } => displayed.push_str(&text),
            RunEvent::ProviderStreamReset { reason, .. } => {
                displayed.clear();
                resets.push(reason);
            }
            RunEvent::UsageUpdated { usage } => usages.push(usage),
            RunEvent::Completed { outcome } => break outcome,
            RunEvent::Failed { message, .. } => panic!("run failed: {message}"),
            _ => {}
        }
    };

    assert_eq!(displayed, "recovered");
    assert_eq!(outcome.text(), "recovered");
    assert_eq!(outcome.usage(), &recovered_usage);
    assert_eq!(
        resets,
        [crate::ProviderStreamResetReason::RetryableFailure(
            ProviderErrorKind::InvalidResponse
        )]
    );
    assert_eq!(usages, [failed_usage, recovered_usage]);
    let requests = provider.recorded_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
}

#[tokio::test(start_paused = true)]
async fn retryable_provider_failures_exhaust_after_bounded_attempts() {
    let (calls, session) = flaky_session(usize::MAX, Retryability::Retryable).await;
    let mut run = session.start(UserInput::text("start")).await.unwrap();

    let mut retries = 0;
    let retryability = loop {
        match next_event_virtual(&mut run).await {
            RunEvent::ProviderStreamReset {
                reason: crate::ProviderStreamResetReason::RetryableFailure(_),
                ..
            } => {
                retries += 1;
            }
            RunEvent::Failed { retryability, .. } => break retryability,
            RunEvent::Completed { .. } => panic!("run unexpectedly completed"),
            _ => {}
        }
    };
    assert_eq!(retries, super::PROVIDER_TURN_ATTEMPTS - 1);
    assert_eq!(retryability, Retryability::Retryable);
    assert_eq!(calls.load(Ordering::Acquire), super::PROVIDER_TURN_ATTEMPTS);
    let outcome = run.outcome().await;
    assert!(matches!(outcome, Err(Error::Provider(_))), "{outcome:?}");
}

#[test]
fn provider_response_rejects_duplicate_tool_call_ids() {
    let response = ModelResponse::Assistant(vec![
        ContentBlock::ToolCall(tool_call("duplicate", "first")),
        ContentBlock::ToolCall(tool_call("duplicate", "second")),
    ]);

    assert!(!valid_response(&response));
}

#[tokio::test(start_paused = true)]
async fn malformed_responses_and_provider_failures_share_the_turn_attempt_budget() {
    let retryable_error = ProviderError::new(
        ProviderErrorKind::Unavailable,
        "stream broke",
        Retryability::Retryable,
    );
    let provider = ScriptedProvider::new(
        ModelIdentity::new("mixed-flaky", "test", "mixed-flaky"),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(Vec::new())),
            ScriptedTurn::failed(retryable_error.clone()),
            ScriptedTurn::failed(retryable_error.clone()),
            ScriptedTurn::failed(retryable_error),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "must not be reached".into(),
            )])),
        ],
    );
    let session = Rho::builder()
        .provider(provider.clone())
        .build()
        .unwrap()
        .session(SessionOptions::default())
        .await
        .unwrap();
    let mut run = session.start(UserInput::text("start")).await.unwrap();

    loop {
        match next_event_virtual(&mut run).await {
            RunEvent::Failed { .. } => break,
            RunEvent::Completed { .. } => panic!("run unexpectedly completed"),
            _ => {}
        }
    }

    assert!(matches!(run.outcome().await, Err(Error::Provider(_))));
    assert_eq!(
        provider.recorded_requests().len(),
        super::PROVIDER_TURN_ATTEMPTS
    );
}

#[tokio::test]
async fn permanent_provider_failures_are_not_retried() {
    let (calls, session) = flaky_session(usize::MAX, Retryability::Permanent).await;

    let outcome = tokio::time::timeout(TEST_TIMEOUT, session.complete("start"))
        .await
        .expect("failing run timed out");
    assert!(matches!(outcome, Err(Error::Provider(_))), "{outcome:?}");
    assert_eq!(calls.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn cancellation_during_retry_backoff_stops_the_run() {
    let (calls, session) = flaky_session(usize::MAX, Retryability::Retryable).await;
    let mut run = session.start(UserInput::text("start")).await.unwrap();

    loop {
        match next_event(&mut run).await {
            RunEvent::ProviderStreamReset {
                reason: crate::ProviderStreamResetReason::RetryableFailure(_),
                ..
            } => break,
            RunEvent::Failed { message, .. } => panic!("run failed: {message}"),
            _ => {}
        }
    }
    run.cancel();

    let outcome = tokio::time::timeout(TEST_TIMEOUT, run.outcome())
        .await
        .expect("cancelled run timed out");
    assert!(matches!(outcome, Err(Error::Cancelled)), "{outcome:?}");
    assert_eq!(calls.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn event_delivery_failure_does_not_commit_interrupted_tool_results() {
    let runtime = Rho::builder()
        .provider(StrictContinuationProvider::new(vec![tool_call(
            "unobserved",
            "missing",
        )]))
        .event_capacity(NonZeroUsize::new(1).unwrap())
        .build()
        .unwrap();
    let core = SessionCore::new(
        SessionId::new(),
        Vec::new(),
        Revision::INITIAL,
        CompactionState::default(),
        /*metadata*/ Default::default(),
        /*prompt_cache_key*/ None,
        runtime.clone(),
    );
    let cancellation = CancellationToken::new();
    let (events, mut event_receiver) = mpsc::channel(1);
    let (_commands, command_receiver) = mpsc::channel(1);
    let worker = tokio::spawn(execute_run(
        Arc::clone(&core),
        runtime,
        RunId::new(),
        RunStart::user(UserInput::text("start")),
        cancellation,
        events,
        command_receiver,
    ));

    assert!(matches!(
        event_receiver.recv().await,
        Some(RunEvent::Started { .. })
    ));
    assert!(matches!(
        event_receiver.recv().await,
        Some(RunEvent::StepStarted { .. })
    ));
    drop(event_receiver);

    let result = tokio::time::timeout(TEST_TIMEOUT, worker)
        .await
        .expect("worker did not observe the closed event stream")
        .unwrap();
    assert!(matches!(result, Err(Error::Interrupted { .. })));
    assert_eq!(core.snapshot(), (Vec::new(), Revision::INITIAL));
    assert_eq!(core.state(), SessionState::Idle);
}
