use std::{num::NonZeroUsize, sync::Arc, time::Duration};

use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::sync::Notify;

use crate::{
    model::{
        ContentBlock, Message, ModelEvent, ModelIdentity, ModelResponse, ProviderContextBlock,
        ToolCall, ToolResult, ToolSpec,
    },
    provider::{ScriptedProvider, ScriptedTurn},
    tool::{
        PreparedToolInvocation, Tool, ToolContext, ToolError, ToolExecutionMode, ToolFuture,
        ToolInvocation, ToolOutput, ToolPreparationContext, ToolPrepareFuture,
    },
    Error, Rho, Run, RunEvent, SessionOptions, StopReason, UserInput,
};

use super::tool_batch::INTERRUPTED_TOOL_RESULT_CONTENT;

const TEST_TIMEOUT: Duration = Duration::from_secs(2);

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

fn async_marker(call_id: &str) -> ModelEvent {
    let block = ProviderContextBlock::async_tool_call(
        ModelIdentity::new("scripted", "test", "async"),
        call_id,
    );
    ModelEvent::ProviderContext {
        kind: block.kind,
        position: block.position,
        data: block.data,
    }
}

fn identity() -> ModelIdentity {
    ModelIdentity::new("scripted", "test", "async")
}

struct GatedAsyncTool {
    name: &'static str,
    gate: Arc<Notify>,
    exclusive: bool,
}

impl Tool for GatedAsyncTool {
    fn spec(&self) -> ToolSpec {
        tool_spec(self.name)
    }

    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Async
    }

    fn prepare<'a>(
        &'a self,
        _invocation: ToolInvocation,
        _context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        let gate = Arc::clone(&self.gate);
        let exclusive = self.exclusive;
        let name = self.name;
        Box::pin(async move {
            if exclusive {
                return Ok(PreparedToolInvocation::exclusive(
                    Default::default(),
                    move |_context| {
                        Box::pin(async move { Ok(ToolOutput::text(format!("{name} exclusive"))) })
                    },
                ));
            }
            Ok(PreparedToolInvocation::resource_aware(
                [],
                [],
                Default::default(),
                move |_context| {
                    Box::pin(async move {
                        gate.notified().await;
                        Ok(ToolOutput::text(format!("{name} done")))
                    })
                },
            ))
        })
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async {
            Err(ToolError::new(
                crate::tool::ToolErrorKind::Execution,
                "use prepare",
            ))
        })
    }
}

struct ImmediateAsyncTool {
    name: &'static str,
}

impl Tool for ImmediateAsyncTool {
    fn spec(&self) -> ToolSpec {
        tool_spec(self.name)
    }

    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Async
    }

    fn prepare<'a>(
        &'a self,
        _invocation: ToolInvocation,
        _context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        let name = self.name;
        Box::pin(async move {
            Ok(PreparedToolInvocation::resource_aware(
                [],
                [],
                Default::default(),
                move |_context| {
                    Box::pin(async move { Ok(ToolOutput::text(format!("{name} done"))) })
                },
            ))
        })
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async { Ok(ToolOutput::text("sync")) })
    }
}

struct ImmediateSyncTool {
    name: &'static str,
}

impl Tool for ImmediateSyncTool {
    fn spec(&self) -> ToolSpec {
        tool_spec(self.name)
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async { Ok(ToolOutput::text("sync")) })
    }
}

async fn next_event(run: &mut Run) -> RunEvent {
    tokio::time::timeout(TEST_TIMEOUT, run.next_event())
        .await
        .expect("run event timed out")
        .expect("run event stream closed")
}

fn tool_results_in(messages: &[Message]) -> Vec<ToolResult> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some(result.clone()),
            _ => None,
        })
        .collect()
}

fn text_turn(text: &str) -> ScriptedTurn {
    ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
        text.into(),
    )]))
}

fn async_call_turn(id: &str, name: &str) -> ScriptedTurn {
    ScriptedTurn::streaming(
        vec![async_marker(id)],
        ModelResponse::Assistant(vec![ContentBlock::ToolCall(tool_call(id, name))]),
    )
}

// Covers: an honoured async call lets the loop take another model step before
// the result is appended; the following request ends with ToolResult.
// Owner: sdk orchestration
#[tokio::test]
async fn pending_job_then_end_turn_continues_with_result() {
    let gate = Arc::new(Notify::new());
    let provider = ScriptedProvider::new(
        identity(),
        [
            async_call_turn("call-a", "slow"),
            text_turn("working"),
            text_turn("done"),
        ],
    );
    let session = Rho::builder()
        .provider(provider.clone())
        .tool(GatedAsyncTool {
            name: "slow",
            gate: Arc::clone(&gate),
            exclusive: false,
        })
        .build()
        .unwrap()
        .session(SessionOptions::default())
        .await
        .unwrap();
    let mut run = session.start(UserInput::text("start")).await.unwrap();

    let mut saw_started = false;
    let mut saw_detached = false;
    let mut saw_step_2 = false;
    let mut saw_finished_after_step_2 = false;
    loop {
        match next_event(&mut run).await {
            RunEvent::ToolStarted { .. } => saw_started = true,
            RunEvent::ToolDetached { .. } => {
                assert!(saw_started);
                saw_detached = true;
            }
            RunEvent::StepStarted { step: 2, .. } => {
                assert!(saw_detached);
                saw_step_2 = true;
                gate.notify_one();
            }
            RunEvent::ToolFinished { ref call_id, .. } if call_id.as_str() == "call-a" => {
                assert!(saw_step_2);
                saw_finished_after_step_2 = true;
            }
            RunEvent::Completed { .. } => break,
            RunEvent::Failed { message, .. } => panic!("run failed: {message}"),
            _ => {}
        }
    }

    assert!(saw_finished_after_step_2);
    let outcome = run.outcome().await.unwrap();
    assert_eq!(outcome.stop_reason(), StopReason::EndTurn);
    let requests = provider.recorded_requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        tool_results_in(&requests[2].messages),
        vec![ToolResult {
            id: "call-a".into(),
            ok: true,
            content: "slow done".into(),
        }]
    );
    assert!(matches!(
        requests[2].messages.last(),
        Some(Message::ToolResult(result)) if result.id == "call-a"
    ));
}

// Covers: finished async results enter history in completion order, both before
// the next model request.
// Owner: sdk orchestration
#[tokio::test]
async fn results_deliver_in_completion_order_before_next_request() {
    let gate_a = Arc::new(Notify::new());
    let gate_b = Arc::new(Notify::new());
    let provider = ScriptedProvider::new(
        identity(),
        [
            ScriptedTurn::streaming(
                vec![async_marker("call-a"), async_marker("call-b")],
                ModelResponse::Assistant(vec![
                    ContentBlock::ToolCall(tool_call("call-a", "alpha")),
                    ContentBlock::ToolCall(tool_call("call-b", "beta")),
                ]),
            ),
            text_turn("waiting"),
            text_turn("done"),
        ],
    );
    let session = Rho::builder()
        .provider(provider.clone())
        .tool(GatedAsyncTool {
            name: "alpha",
            gate: Arc::clone(&gate_a),
            exclusive: false,
        })
        .tool(GatedAsyncTool {
            name: "beta",
            gate: Arc::clone(&gate_b),
            exclusive: false,
        })
        .build()
        .unwrap()
        .session(SessionOptions::default())
        .await
        .unwrap();
    let mut run = session.start(UserInput::text("start")).await.unwrap();

    loop {
        match next_event(&mut run).await {
            RunEvent::StepStarted { step: 2, .. } => {
                gate_b.notify_one();
                gate_a.notify_one();
            }
            RunEvent::Completed { .. } => break,
            RunEvent::Failed { message, .. } => panic!("run failed: {message}"),
            _ => {}
        }
    }

    let requests = provider.recorded_requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        tool_results_in(&requests[2].messages),
        vec![
            ToolResult {
                id: "call-b".into(),
                ok: true,
                content: "beta done".into(),
            },
            ToolResult {
                id: "call-a".into(),
                ok: true,
                content: "alpha done".into(),
            },
        ]
    );
}

// Covers: cancel while awaiting a detached job writes the interrupted result
// before the terminal commit.
// Owner: sdk orchestration
#[tokio::test]
async fn cancel_while_awaiting_interrupts_jobs() {
    let gate = Arc::new(Notify::new());
    let provider = ScriptedProvider::new(
        identity(),
        [async_call_turn("call-a", "slow"), text_turn("waiting")],
    );
    let runtime = Rho::builder()
        .provider(provider)
        .tool(GatedAsyncTool {
            name: "slow",
            gate: Arc::clone(&gate),
            exclusive: false,
        })
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("start")).await.unwrap();

    loop {
        match next_event(&mut run).await {
            RunEvent::StepStarted { step: 2, .. } => break,
            RunEvent::Failed { message, .. } => panic!("run failed: {message}"),
            _ => {}
        }
    }
    // Step 2 is the text-only turn; AwaitJobs follows ModelCallCompleted.
    loop {
        match next_event(&mut run).await {
            RunEvent::ModelCallCompleted { .. } => break,
            RunEvent::Failed { message, .. } => panic!("run failed: {message}"),
            _ => {}
        }
    }
    run.cancel();
    let outcome = tokio::time::timeout(TEST_TIMEOUT, run.outcome())
        .await
        .expect("cancelled run timed out");
    assert!(matches!(outcome, Err(Error::Cancelled)), "{outcome:?}");
    assert_eq!(
        session
            .history()
            .into_iter()
            .filter_map(|message| match message {
                Message::ToolResult(result) => Some(result),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![ToolResult {
            id: "call-a".into(),
            ok: false,
            content: INTERRUPTED_TOOL_RESULT_CONTENT.into(),
        }]
    );
}

// Covers: exhausting the step budget with a still-pending async job interrupts
// it before MaxSteps commit.
// Owner: sdk orchestration
#[tokio::test]
async fn max_steps_with_pending_job_interrupts_before_commit() {
    let gate = Arc::new(Notify::new());
    let provider = ScriptedProvider::new(identity(), [async_call_turn("call-a", "slow")]);
    let session = Rho::builder()
        .provider(provider)
        .tool(GatedAsyncTool {
            name: "slow",
            gate,
            exclusive: false,
        })
        .max_steps(NonZeroUsize::new(1).unwrap())
        .build()
        .unwrap()
        .session(SessionOptions::default())
        .await
        .unwrap();
    let outcome = tokio::time::timeout(TEST_TIMEOUT, session.complete("start"))
        .await
        .expect("run timed out")
        .unwrap();
    assert_eq!(outcome.stop_reason(), StopReason::MaxSteps);
    assert_eq!(
        session
            .history()
            .into_iter()
            .filter_map(|message| match message {
                Message::ToolResult(result) => Some(result),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![ToolResult {
            id: "call-a".into(),
            ok: false,
            content: INTERRUPTED_TOOL_RESULT_CONTENT.into(),
        }]
    );
}

// Covers: async execution requires both a provider marker and an Async tool
// declaration; either missing key stays on the sync path.
// Owner: sdk orchestration
#[tokio::test]
async fn provider_mark_without_async_declaration_runs_sync() {
    let cases = [
        ("marked + sync tool", true, false, "sync_tool"),
        ("unmarked + async tool", false, true, "async_tool"),
    ];
    for (name, marked, async_decl, tool_name) in cases {
        let provider = ScriptedProvider::new(
            identity(),
            [
                if marked {
                    ScriptedTurn::streaming(
                        vec![async_marker("call-1")],
                        ModelResponse::Assistant(vec![ContentBlock::ToolCall(tool_call(
                            "call-1", tool_name,
                        ))]),
                    )
                } else {
                    ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                        tool_call("call-1", tool_name),
                    )]))
                },
                text_turn("done"),
            ],
        );
        let mut builder = Rho::builder().provider(provider.clone());
        builder = if async_decl {
            builder.tool(ImmediateAsyncTool { name: "async_tool" })
        } else {
            builder.tool(ImmediateSyncTool { name: "sync_tool" })
        };
        let session = builder
            .build()
            .unwrap()
            .session(SessionOptions::default())
            .await
            .unwrap();
        let mut run = session.start(UserInput::text("start")).await.unwrap();
        let mut detached = false;
        let mut finished_before_step_2 = false;
        let mut finished = false;
        loop {
            match next_event(&mut run).await {
                RunEvent::ToolDetached { .. } => detached = true,
                RunEvent::ToolFinished { .. } => {
                    finished = true;
                    finished_before_step_2 = true;
                }
                RunEvent::StepStarted { step: 2, .. } => {
                    assert!(finished, "{name}");
                    finished_before_step_2 = finished;
                }
                RunEvent::Completed { .. } => break,
                RunEvent::Failed { message, .. } => panic!("{name}: {message}"),
                _ => {}
            }
        }
        assert!(!detached, "{name}");
        assert!(finished_before_step_2, "{name}");
        let requests = provider.recorded_requests();
        assert_eq!(requests.len(), 2, "{name}");
        assert!(
            matches!(
                requests[1].messages.last(),
                Some(Message::ToolResult(result)) if result.id == "call-1"
            ),
            "{name}"
        );
    }
}

// Covers: an async-declared tool with an exclusive plan fails that call and
// the run continues.
// Owner: sdk orchestration
#[tokio::test]
async fn async_tool_with_exclusive_plan_fails_that_call() {
    let provider = ScriptedProvider::new(
        identity(),
        [
            async_call_turn("call-a", "exclusive"),
            text_turn("recovered"),
        ],
    );
    let session = Rho::builder()
        .provider(provider.clone())
        .tool(GatedAsyncTool {
            name: "exclusive",
            gate: Arc::new(Notify::new()),
            exclusive: true,
        })
        .build()
        .unwrap()
        .session(SessionOptions::default())
        .await
        .unwrap();
    let outcome = tokio::time::timeout(TEST_TIMEOUT, session.complete("start"))
        .await
        .expect("run timed out")
        .unwrap();
    assert_eq!(outcome.stop_reason(), StopReason::EndTurn);
    let requests = provider.recorded_requests();
    assert_eq!(requests.len(), 2);
    match requests[1].messages.last() {
        Some(Message::ToolResult(result)) => {
            assert_eq!(result.id, "call-a");
            assert!(!result.ok);
        }
        other => panic!("expected failed tool result, got {other:?}"),
    }
}

// Covers: a steer that arrives while awaiting detached jobs is applied at that
// boundary before the next model request.
// Owner: sdk orchestration
#[tokio::test]
async fn steer_during_await_jobs_applies_at_boundary() {
    let gate = Arc::new(Notify::new());
    let provider = ScriptedProvider::new(
        identity(),
        [
            async_call_turn("call-a", "slow"),
            text_turn("waiting"),
            text_turn("steered"),
            text_turn("done"),
        ],
    );
    let session = Rho::builder()
        .provider(provider.clone())
        .tool(GatedAsyncTool {
            name: "slow",
            gate: Arc::clone(&gate),
            exclusive: false,
        })
        .build()
        .unwrap()
        .session(SessionOptions::default())
        .await
        .unwrap();
    let mut run = session.start(UserInput::text("start")).await.unwrap();
    loop {
        match next_event(&mut run).await {
            RunEvent::ModelCallCompleted { .. } => {
                // After the second model call the loop is in AwaitJobs.
                if provider.recorded_requests().len() == 2 {
                    run.steer(UserInput::text("nudge")).await.unwrap();
                    gate.notify_one();
                }
            }
            RunEvent::Completed { .. } => break,
            RunEvent::Failed { message, .. } => panic!("run failed: {message}"),
            _ => {}
        }
    }
    let requests = provider.recorded_requests();
    assert!(requests.len() >= 3);
    let last = &requests[requests.len() - 1].messages;
    assert!(
        last.iter().any(|message| matches!(
            message,
            Message::User(content) if content == &vec![ContentBlock::Text("nudge".into())]
        )),
        "steering missing from last request: {last:?}"
    );
    assert!(
        last.iter()
            .any(|message| matches!(message, Message::ToolResult(result) if result.id == "call-a")),
        "tool result missing from last request: {last:?}"
    );
}
