//! Send-safe stream collection helpers for SDK adaptation.
//!
//! Application stream collectors take `dyn FnMut` event callbacks. Even when the
//! callback is `None`, storing that trait-object type in an `async fn` future
//! makes the future `!Send`. These helpers keep the trait object inside
//! synchronous functions so non-streaming SDK turns remain `Send`.

use crate::{
    model::{ModelError, ModelEvent, ModelResponse, ModelUsage},
    protocol::{
        openai_chat::invalid_stream_utf8,
        openai_responses::{handle_codex_sse_line, CodexSseResponse, CodexSseState},
    },
    provider_backend::line_stream::collect_line_stream,
};

/// Collects a Codex/xAI SSE response without emitting intermediate events.
pub(crate) async fn collect_codex_sse_silent(
    response: reqwest::Response,
) -> Result<CodexSseResponse, ModelError> {
    let mut state = CodexSseState::default();
    collect_line_stream(response, invalid_stream_utf8, |line| {
        apply_codex_sse_line_silent(&mut state, line)
    })
    .await?;
    state.into_response()
}

fn apply_codex_sse_line_silent(state: &mut CodexSseState, line: &str) -> Result<bool, ModelError> {
    let mut on_event: Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)> = None;
    handle_codex_sse_line(line, state, &mut on_event)
}

/// Collects every finished output item and the final usage from a Responses
/// SSE body, without requiring assistant text or tool calls.
///
/// Remote compaction v2 streams a single `compaction` item and no text.
pub(crate) async fn collect_codex_sse_output_items(
    response: reqwest::Response,
) -> Result<(Vec<serde_json::Value>, ModelUsage), ModelError> {
    let mut state = CodexSseState::default();
    let mut usage = ModelUsage::default();
    collect_line_stream(response, invalid_stream_utf8, |line| {
        apply_codex_sse_line_capturing_usage(&mut state, &mut usage, line)
    })
    .await?;
    Ok((state.output_items, usage))
}

fn apply_codex_sse_line_capturing_usage(
    state: &mut CodexSseState,
    usage: &mut ModelUsage,
    line: &str,
) -> Result<bool, ModelError> {
    let mut capture = |event: ModelEvent| {
        if let ModelEvent::Usage(reported) = event {
            *usage = reported;
        }
        Ok(())
    };
    let mut on_event: Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)> =
        Some(&mut capture);
    handle_codex_sse_line(line, state, &mut on_event)
}

/// Collects a Codex/xAI SSE body into a final model response without events.
pub(crate) async fn collect_codex_model_response_silent(
    response: reqwest::Response,
) -> Result<ModelResponse, ModelError> {
    collect_codex_sse_silent(response)
        .await
        .map(|output| output.response)
}
