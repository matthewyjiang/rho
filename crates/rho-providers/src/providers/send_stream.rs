//! Send-safe stream collection helpers for SDK adaptation.
//!
//! Application stream collectors take `dyn FnMut` event callbacks. Even when the
//! callback is `None`, storing that trait-object type in an `async fn` future
//! makes the future `!Send`. These helpers keep the trait object inside
//! synchronous functions so non-streaming SDK turns remain `Send`.

use crate::{
    model::{ModelError, ModelEvent, ModelResponse},
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

/// Collects a Codex/xAI SSE body into a final model response without events.
pub(crate) async fn collect_codex_model_response_silent(
    response: reqwest::Response,
) -> Result<ModelResponse, ModelError> {
    collect_codex_sse_silent(response)
        .await
        .map(|output| output.response)
}
