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
    Error, Rho, RunEvent, SessionOptions, UserInput,
};
use serde_json::json;

/// Geometric argument sizes used for near-linear growth checks (4x steps).
pub(super) const LARGE_TOOL_CALL_ARGUMENT_SIZES: &[usize] = &[16 * 1024, 64 * 1024, 256 * 1024];
pub(super) const LARGE_TOOL_CALL_DELTA_CHUNK_BYTES: usize = 256;
/// ns/byte at the largest size must stay within this factor of the smallest.
/// Linear capture stays near 1.0 (often below as fixed costs amortize); quadratic
/// capture grows with size and exceeds this budget across the 16x span.
pub(super) const LARGE_TOOL_CALL_NS_PER_BYTE_GROWTH_LIMIT: f64 = 2.0;
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

/// Streams tool-call argument deltas only, then waits for cancellation.
///
/// Intentionally omits a final tool-call [`ModelResponse`] so aborted history
/// must come from stream capture rather than the provider terminal response.
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
            if request.cancellation.is_cancelled() {
                return Err(rho_sdk::ProviderError::interrupted("benchmark cancelled"));
            }
            // Non-stream fallback is unused by the cancelled-capture fixture.
            Err(rho_sdk::ProviderError::interrupted(
                "large tool-call fixture requires streaming",
            ))
        })
    }

    fn send_turn_stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
        events: ProviderEventSender,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
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

            // Hold the turn open until the consumer cancels so history is committed
            // through aborted stream capture, not a terminal ModelResponse.
            request.cancellation.cancelled().await;
            Err(rho_sdk::ProviderError::interrupted("benchmark cancelled"))
        })
    }
}

/// Streams one large tool-call argument payload, cancels after the final delta,
/// and validates that aborted history retained the captured tool call.
pub(super) fn run_cancelled_large_tool_call_capture(
    tokio: &tokio::runtime::Runtime,
    arguments: Arc<String>,
    chunk_bytes: usize,
) -> usize {
    let expected_deltas = large_tool_call_delta_count(arguments.len(), chunk_bytes);
    let expected_arguments = arguments.as_str();
    let expected_value: serde_json::Value = serde_json::from_str(expected_arguments).unwrap();
    let runtime = Rho::builder()
        .provider(LargeToolCallDeltaProvider {
            arguments: Arc::clone(&arguments),
            chunk_bytes,
        })
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
                if observed_deltas == expected_deltas {
                    run.cancel();
                }
            }
        }
        assert!(
            matches!(run.outcome().await, Err(Error::Cancelled)),
            "expected cancelled outcome after streamed tool-call capture"
        );
    });
    assert_eq!(observed_deltas, expected_deltas);

    let history = session.history();
    let aborted = history
        .iter()
        .rev()
        .find_map(|message| match message {
            Message::AbortedAssistant(message) => Some(message.as_ref()),
            _ => None,
        })
        .expect("cancelled stream must commit an aborted assistant");
    assert_eq!(
        aborted.content,
        vec![ContentBlock::ToolCall(ToolCall {
            id: "large-call".into(),
            name: "benchmark_large".into(),
            arguments: expected_value.clone(),
        })]
    );
    assert_eq!(aborted.tool_calls.len(), 1);
    assert_eq!(aborted.tool_calls[0].id.as_deref(), Some("large-call"));
    assert_eq!(
        aborted.tool_calls[0].name.as_deref(),
        Some("benchmark_large")
    );
    assert_eq!(aborted.tool_calls[0].arguments, expected_arguments);
    black_box(observed_deltas)
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
            // yields reach peak COUNT while preparation remains unbounded.
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

/// Runs a prepare batch at the given execution limit, checks ordered tool results,
/// and returns observed peak preparation concurrency.
pub(super) fn run_overlapping_prepare_batch(
    tokio: &tokio::runtime::Runtime,
    max_parallel_tools: usize,
) -> usize {
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
        .max_parallel_tools(NonZeroUsize::new(max_parallel_tools).unwrap())
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
    assert_eq!(
        observed_peak, OVERLAPPING_PREPARE_COUNT,
        "preparation remains unbounded by max_parallel_tools={max_parallel_tools}, peak={observed_peak}"
    );
    assert_eq!(active.load(Ordering::Acquire), 0);
    black_box(observed_peak)
}
