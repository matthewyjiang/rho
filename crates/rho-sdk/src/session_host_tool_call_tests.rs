use std::{num::NonZeroUsize, sync::Arc, time::Duration};

use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::sync::Notify;

use crate::{
    model::{ContentBlock, Message, ModelIdentity, ModelResponse, ToolCall, ToolResult, ToolSpec},
    provider::{ScriptedProvider, ScriptedTurn},
    tool::{
        PreparedToolInvocation, ScriptedTool, ScriptedToolOutcome, Tool, ToolInvocation,
        ToolMetadata, ToolOutput, ToolPreparationContext, ToolPrepareFuture,
    },
    Error, Rho, RunEvent, SessionOptions, UserInput,
};

const TEST_TIMEOUT: Duration = Duration::from_secs(10);

fn identity() -> ModelIdentity {
    ModelIdentity::new("scripted", "test", "model")
}

fn spec(name: &str) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: name.into(),
        input_schema: json!({"type": "object"}),
    }
}

fn call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        arguments,
    }
}

fn done_provider() -> ScriptedProvider {
    ScriptedProvider::new(
        identity(),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("done".into()),
        ]))],
    )
}

/// Parallel-safe tool whose `wait` calls finish only after a sibling call in
/// the same batch has run, so completion order is the reverse of call order.
/// Each output names the invocation source it observed.
struct GateTool {
    gate: Arc<Notify>,
}

impl Tool for GateTool {
    fn spec(&self) -> ToolSpec {
        spec("gate")
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        _context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        Box::pin(async move {
            let waits = invocation.arguments()["wait"] == json!(true);
            let output = format!(
                "{} via {:?}",
                invocation.arguments()["label"].as_str().unwrap_or_default(),
                invocation.source()
            );
            Ok(PreparedToolInvocation::resource_aware(
                [],
                [],
                ToolMetadata::new(),
                move |_context| {
                    Box::pin(async move {
                        if waits {
                            self.gate.notified().await;
                        } else {
                            self.gate.notify_one();
                        }
                        Ok(ToolOutput::text(output))
                    })
                },
            ))
        })
    }
}

// Covers: a host batch must run every call as host-sourced work before the
// first model request and hand all results to it in call order, even when the
// calls finish in the opposite order.
// Owner: SDK host tool-call orchestration.
#[tokio::test]
async fn host_batch_results_reach_the_first_model_request_in_call_order() {
    let provider = done_provider();
    let runtime = Rho::builder()
        .provider(provider.clone())
        .max_parallel_tools(NonZeroUsize::new(2).unwrap())
        .tool(GateTool {
            gate: Arc::new(Notify::new()),
        })
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let calls = vec![
        call("host-1", "gate", json!({"label": "first", "wait": true})),
        call("host-2", "gate", json!({"label": "second", "wait": false})),
    ];

    let mut run = session
        .start_with_tool_calls(UserInput::text("run both"), calls.clone())
        .await
        .unwrap();
    let outcome = tokio::time::timeout(TEST_TIMEOUT, async {
        while run.next_event().await.is_some() {}
        run.outcome().await
    })
    .await
    .expect("host batch did not run its calls in parallel");

    assert_eq!(outcome.unwrap().text(), "done");
    let requests = provider.recorded_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].messages,
        vec![
            Message::user_text("run both"),
            Message::Assistant(calls.into_iter().map(ContentBlock::ToolCall).collect()),
            Message::ToolResult(ToolResult {
                id: "host-1".into(),
                ok: true,
                content: "first via Host".into(),
            }),
            Message::ToolResult(ToolResult {
                id: "host-2".into(),
                ok: true,
                content: "second via Host".into(),
            }),
        ]
    );
}

// Covers: a host-requested completed failure still reaches the first model
// request as an error result rather than silently becoming success.
// Owner: SDK model-history completion contract.
#[tokio::test]
async fn host_requested_completed_failure_reaches_the_first_model_request() {
    let provider = done_provider();
    let tool = Arc::new(ScriptedTool::new(
        spec("lookup"),
        ScriptedToolOutcome::Success(ToolOutput::text("tool output").failed()),
    ));
    let runtime = Rho::builder()
        .provider(provider.clone())
        .tool_shared(tool)
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let call = call("host-call-1", "lookup", json!({"key": "value"}));

    let mut run = session
        .start_with_tool_call(UserInput::text("use the selected tool"), call.clone())
        .await
        .unwrap();
    while run.next_event().await.is_some() {}
    let outcome = run.outcome().await.unwrap();

    assert_eq!(outcome.text(), "done");
    let requests = provider.recorded_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].messages,
        vec![
            Message::user_text("use the selected tool"),
            Message::Assistant(vec![ContentBlock::ToolCall(call)]),
            Message::ToolResult(ToolResult {
                id: "host-call-1".into(),
                ok: false,
                content: "tool output".into(),
            }),
        ]
    );
}

// Covers: cancelling a host batch after one call finished must keep that
// result and pair every unfinished call, so committed history stays
// replayable to providers that require a result for every call.
// Owner: SDK host tool-call cancellation.
#[tokio::test]
async fn cancel_mid_host_batch_pairs_every_call_with_a_result() {
    let runtime = Rho::builder()
        .provider(ScriptedProvider::new(
            identity(),
            Vec::<ScriptedTurn>::new(),
        ))
        .tool(ScriptedTool::new(
            spec("fast"),
            ScriptedToolOutcome::Success(ToolOutput::text("fast output")),
        ))
        .tool(ScriptedTool::new(
            spec("slow"),
            ScriptedToolOutcome::WaitForCancellation,
        ))
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let calls = vec![
        call("host-fast", "fast", json!({})),
        call("host-slow", "slow", json!({})),
    ];

    let mut run = session
        .start_with_tool_calls(UserInput::text("run both"), calls.clone())
        .await
        .unwrap();
    // Exclusive tools run in call order, so the slow call starts only after
    // the fast call finished.
    tokio::time::timeout(TEST_TIMEOUT, async {
        while let Some(event) = run.next_event().await {
            if matches!(&event, RunEvent::ToolStarted { name, .. } if name == "slow") {
                return;
            }
        }
        panic!("run ended before the slow call started");
    })
    .await
    .expect("slow call did not start");
    run.cancel();
    let outcome = tokio::time::timeout(TEST_TIMEOUT, run.outcome())
        .await
        .expect("cancelled run timed out");

    assert!(matches!(outcome, Err(Error::Cancelled)), "{outcome:?}");
    let history = session.history();
    assert_eq!(
        history[..3],
        [
            Message::user_text("run both"),
            Message::Assistant(calls.into_iter().map(ContentBlock::ToolCall).collect()),
            Message::ToolResult(ToolResult {
                id: "host-fast".into(),
                ok: true,
                content: "fast output".into(),
            }),
        ]
    );
    assert!(
        matches!(
            &history[3..],
            [Message::ToolResult(ToolResult { id, ok: false, .. })] if id == "host-slow"
        ),
        "{history:?}"
    );
}

#[tokio::test]
async fn host_tool_call_batches_reject_invalid_shapes() {
    let provider = ScriptedProvider::new(identity(), Vec::<ScriptedTurn>::new());
    let runtime = Rho::builder().provider(provider).build().unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let valid = call("host-call-1", "lookup", json!({}));
    let invalid_batches = [
        ("empty", vec![]),
        (
            "duplicate id",
            vec![valid.clone(), call("host-call-1", "other", json!({}))],
        ),
        (
            "empty id",
            vec![valid.clone(), call("", "lookup", json!({}))],
        ),
        (
            "empty name",
            vec![valid.clone(), call("host-call-2", "", json!({}))],
        ),
        (
            "non-object arguments",
            vec![valid.clone(), call("host-call-2", "lookup", json!([]))],
        ),
    ];

    for (case, calls) in invalid_batches {
        let result = session
            .start_with_tool_calls(UserInput::text("invalid"), calls)
            .await;
        assert!(
            matches!(result, Err(Error::InvalidConfiguration { .. })),
            "{case}: {:?}",
            result.as_ref().err()
        );
    }
    assert!(session.history().is_empty());
}
