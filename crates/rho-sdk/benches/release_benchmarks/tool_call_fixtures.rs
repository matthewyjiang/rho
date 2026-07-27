//! Large tool-call stream and overlapping preparation release-benchmark fixtures.

use std::{
    hint::black_box,
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use rho_sdk::{
    model::{
        ContentBlock, Message, ModelEvent, ModelIdentity, ModelRequest, ModelResponse, ToolCall,
        ToolSpec,
    },
    provider::{
        ModelProvider, ProviderEventSender, ProviderFuture, ScriptedProvider, ScriptedTurn,
    },
    tool::{
        PreparedToolInvocation, Tool, ToolContext, ToolFuture, ToolInvocation, ToolMetadata,
        ToolOutput, ToolPreparationContext, ToolPrepareFuture, ToolResource, ToolResourceAccess,
    },
    Rho, RunEvent, SessionOptions, UserInput,
};
use serde_json::json;

pub(super) const LARGE_TOOL_CALL_ARGUMENT_BYTES: usize = 768 * 1024;
pub(super) const LARGE_TOOL_CALL_DELTA_CHUNK_BYTES: usize = 64;
pub(super) const OVERLAPPING_PREPARE_COUNT: usize = 64;
pub(super) const OVERLAPPING_PREPARE_PARALLEL: usize = 4;

pub(super) fn large_tool_call_arguments(total_bytes: usize) -> String {
    // Keep the payload valid JSON while targeting a fixed serialized size.
    let prefix = r#"{"data":""#;
    let suffix = r#""}"#;
    let overhead = prefix.len() + suffix.len();
    let payload_len = total_bytes.saturating_sub(overhead);
    let mut arguments = String::with_capacity(overhead + payload_len);
    arguments.push_str(prefix);
    arguments.extend(std::iter::repeat_n('x', payload_len));
    arguments.push_str(suffix);
    arguments
}

pub(super) fn large_tool_call_delta_count(total_bytes: usize, chunk_bytes: usize) -> usize {
    total_bytes.div_ceil(chunk_bytes.max(1))
}

#[derive(Clone)]
struct LargeToolCallDeltaProvider {
    arguments: Arc<String>,
    chunk_bytes: usize,
}

impl ModelProvider for LargeToolCallDeltaProvider {
    fn identity(&self) -> ModelIdentity {
        ModelIdentity::new("benchmark", "large-tool-call-delta-fixture", "v1")
    }

    fn send_turn<'a>(&'a self, request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async move {
            if request
                .messages
                .iter()
                .any(|message| matches!(message, Message::ToolResult(_)))
            {
                return Ok(ModelResponse::Assistant(vec![ContentBlock::Text(
                    "done".into(),
                )]));
            }
            let arguments = serde_json::from_str(self.arguments.as_str()).unwrap();
            Ok(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                ToolCall {
                    id: "large-call".into(),
                    name: "benchmark_large".into(),
                    arguments,
                },
            )]))
        })
    }

    fn send_turn_stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
        events: ProviderEventSender,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            if request
                .messages
                .iter()
                .any(|message| matches!(message, Message::ToolResult(_)))
            {
                return Ok(ModelResponse::Assistant(vec![ContentBlock::Text(
                    "done".into(),
                )]));
            }

            let arguments = self.arguments.as_str();
            let mut offset = 0usize;
            let mut first = true;
            while offset < arguments.len() {
                let end = (offset + self.chunk_bytes).min(arguments.len());
                tokio::select! {
                    result = events.send(ModelEvent::ToolCallDelta {
                        index: 0,
                        id: first.then(|| "large-call".into()),
                        name: first.then(|| "benchmark_large".into()),
                        arguments: arguments[offset..end].to_owned(),
                    }) => result?,
                    () = request.cancellation.cancelled() => {
                        return Err(rho_sdk::ProviderError::interrupted("benchmark cancelled"));
                    }
                }
                first = false;
                offset = end;
            }

            let parsed = serde_json::from_str(arguments).unwrap();
            Ok(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                ToolCall {
                    id: "large-call".into(),
                    name: "benchmark_large".into(),
                    arguments: parsed,
                },
            )]))
        })
    }
}

#[derive(Clone)]
struct LargeArgsTool;

impl Tool for LargeArgsTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "benchmark_large".into(),
            description: "accepts a large streamed argument object".into(),
            input_schema: json!({"type":"object","required":["data"]}),
        }
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let bytes = invocation.arguments().to_string().len();
            Ok(ToolOutput::text(format!("accepted-{bytes}")))
        })
    }
}

/// Streams one large tool-call argument payload and validates the observed delta count.
pub(super) fn run_large_tool_call_delta_stream(
    tokio: &tokio::runtime::Runtime,
    arguments: Arc<String>,
    chunk_bytes: usize,
) -> (usize, usize) {
    let delta_count = large_tool_call_delta_count(arguments.len(), chunk_bytes);
    let runtime = Rho::builder()
        .provider(LargeToolCallDeltaProvider {
            arguments: Arc::clone(&arguments),
            chunk_bytes,
        })
        .tool(LargeArgsTool)
        .event_capacity(NonZeroUsize::new(256).unwrap())
        .build()
        .unwrap();
    let session = tokio
        .block_on(runtime.session(SessionOptions::default()))
        .unwrap();
    let mut run = tokio
        .block_on(session.start(UserInput::text("stream large tool call")))
        .unwrap();
    let mut observed_deltas = 0usize;
    tokio.block_on(async {
        while let Some(event) = run.next_event().await {
            if matches!(event, RunEvent::ToolCallUpdated { .. }) {
                observed_deltas += 1;
            }
        }
        black_box(run.outcome().await.unwrap());
    });
    assert_eq!(observed_deltas, delta_count);
    black_box((arguments.len(), delta_count))
}

#[derive(Clone)]
struct OverlappingPrepareTool {
    active: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
}

impl Tool for OverlappingPrepareTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "benchmark_prepare".into(),
            description: "prepare-future overlap probe".into(),
            input_schema: json!({"type":"object","required":["index"]}),
        }
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        unreachable!("benchmark prepare uses prepared execution")
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        _context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        let active = Arc::clone(&self.active);
        let peak = Arc::clone(&self.peak);
        let index = invocation.arguments()["index"].as_u64().unwrap() as usize;
        Box::pin(async move {
            let current = active.fetch_add(1, Ordering::AcqRel) + 1;
            peak.fetch_max(current, Ordering::AcqRel);
            // Yield so concurrently polled prepare futures can enter and raise the
            // peak before any finish. A barrier of the full batch size would
            // deadlock once preparation is bounded below OVERLAPPING_PREPARE_COUNT;
            // yields still reach peak 64 when unbounded and complete at peak 4 when
            // preparation is limited to OVERLAPPING_PREPARE_PARALLEL.
            for _ in 0..OVERLAPPING_PREPARE_COUNT {
                tokio::task::yield_now().await;
            }
            active.fetch_sub(1, Ordering::AcqRel);
            Ok(PreparedToolInvocation::resource_aware(
                [ToolResourceAccess::shared(ToolResource::opaque(
                    "release-benchmark-prepare",
                    "shared",
                ))],
                [],
                ToolMetadata::new(),
                move |_context| {
                    Box::pin(async move { Ok(ToolOutput::text(format!("prepared-{index}"))) })
                },
            ))
        })
    }
}

/// Runs a prepare batch, checks ordered tool results, and returns observed peak concurrency.
pub(super) fn run_overlapping_prepare_batch(tokio: &tokio::runtime::Runtime) -> usize {
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let tool = OverlappingPrepareTool {
        active: Arc::clone(&active),
        peak: Arc::clone(&peak),
    };
    let calls = (0..OVERLAPPING_PREPARE_COUNT)
        .map(|index| {
            ContentBlock::ToolCall(ToolCall {
                id: format!("prepare-{index}"),
                name: "benchmark_prepare".into(),
                arguments: json!({"index": index}),
            })
        })
        .collect::<Vec<_>>();
    let provider = ScriptedProvider::new(
        ModelIdentity::new("benchmark", "overlapping-prepare-fixture", "v1"),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(calls)),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "done".into(),
            )])),
        ],
    );
    let runtime = Rho::builder()
        .provider(provider)
        .tool(tool)
        .max_parallel_tools(NonZeroUsize::new(OVERLAPPING_PREPARE_PARALLEL).unwrap())
        .build()
        .unwrap();
    let session = tokio
        .block_on(runtime.session(SessionOptions::default()))
        .unwrap();
    let mut run = tokio
        .block_on(session.start(UserInput::text("prepare batch")))
        .unwrap();
    tokio.block_on(async {
        while run.next_event().await.is_some() {}
        black_box(run.outcome().await.unwrap());
    });
    let results = session
        .history()
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some(result.content.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let expected = (0..OVERLAPPING_PREPARE_COUNT)
        .map(|index| format!("prepared-{index}"))
        .collect::<Vec<_>>();
    assert_eq!(results, expected);
    let observed_peak = peak.load(Ordering::Acquire);
    assert!(
        observed_peak >= OVERLAPPING_PREPARE_PARALLEL,
        "expected overlapping prepare futures, peak={observed_peak}"
    );
    assert_eq!(active.load(Ordering::Acquire), 0);
    black_box(observed_peak)
}
