use pretty_assertions::assert_eq;

use super::*;
use crate::model::ProviderReportedErrorKind;

// Covers: an HTTP Responses SSE stream that starts normally and then fails must
// surface the provider's own error instead of the empty-content diagnostic.
// Owner: providers stream parse
#[test]
fn response_failed_event_surfaces_provider_error() {
    let mut state = CodexSseState::default();
    let error = handle_codex_sse_line(
        r#"data: {"type":"response.failed","response":{"id":"resp_failed","status":"failed","error":{"code":"server_error","message":"upstream request failed"}}}"#,
        &mut state,
        &mut None,
    )
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "provider reported server_error: upstream request failed"
    );
    assert!(matches!(
        error,
        ModelError::ProviderReported {
            kind: ProviderReportedErrorKind::Unavailable,
            ..
        }
    ));
}

// Covers: a bare `error` event with no nested payload falls back to the HTTP
// transport defaults instead of reporting the event discriminator as the type.
// Owner: providers stream parse
#[test]
fn bare_error_event_uses_http_fallbacks() {
    let mut state = CodexSseState::default();
    let error =
        handle_codex_sse_line(r#"data: {"type":"error"}"#, &mut state, &mut None).unwrap_err();

    assert!(matches!(
        error,
        ModelError::ProviderReported {
            kind: ProviderReportedErrorKind::InvalidResponse,
            error_type,
            message,
        } if error_type == "response_error" && message == "error event received"
    ));
}

// Covers: a bare `error` event on the websocket transport falls back to
// `websocket_error`, which classifies as a retryable Unavailable kind.
// Owner: providers stream parse
#[test]
fn bare_error_event_uses_websocket_fallbacks() {
    let mut state = CodexSseState::default();
    let mut on_event: Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)> = None;
    let error = handle_codex_sse_value(
        &serde_json::json!({"type":"error"}),
        &mut state,
        &mut on_event,
        CodexTransport::WebSocket,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        ModelError::ProviderReported {
            kind: ProviderReportedErrorKind::Unavailable,
            error_type,
            message,
        } if error_type == "websocket_error" && message == "websocket error event received"
    ));
}

// Covers: `response.incomplete` is terminal on the HTTP SSE path, mirroring the
// websocket transport.
// Owner: providers stream parse
#[test]
fn response_incomplete_event_surfaces_reason() {
    let mut state = CodexSseState::default();
    let error = handle_codex_sse_line(
        r#"data: {"type":"response.incomplete","response":{"id":"resp_incomplete","status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}}"#,
        &mut state,
        &mut None,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        ModelError::ProviderReported {
            kind: ProviderReportedErrorKind::InvalidResponse,
            error_type,
            message,
        } if error_type == "response_incomplete" && message.contains("max_output_tokens")
    ));
}

// Covers: an incomplete response after streamed output still fails with the
// provider's reason rather than reporting the partial content as completed,
// mirroring the websocket transport's handling.
// Owner: providers stream parse
#[test]
fn response_incomplete_after_text_delta_still_fails() {
    let mut state = CodexSseState::default();
    let mut deltas = Vec::new();
    let mut on_event: Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)> =
        Some(&mut |event| {
            if let ModelEvent::OutputDelta(delta) = event {
                deltas.push(delta);
            }
            Ok(())
        });
    handle_codex_sse_line(
        r#"data: {"type":"response.output_text.delta","delta":"partial"}"#,
        &mut state,
        &mut on_event,
    )
    .unwrap();
    let error = handle_codex_sse_line(
        r#"data: {"type":"response.incomplete","response":{"id":"resp_incomplete","status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}}"#,
        &mut state,
        &mut on_event,
    )
    .unwrap_err();

    assert_eq!(deltas, ["partial"]);
    assert!(matches!(
        error,
        ModelError::ProviderReported {
            kind: ProviderReportedErrorKind::InvalidResponse,
            error_type,
            message,
        } if error_type == "response_incomplete" && message.contains("max_output_tokens")
    ));
}

// Covers: a failure after streamed output still surfaces the provider error
// rather than reporting the partial content as a completed response.
// Owner: providers stream parse
#[test]
fn response_failed_after_text_delta_still_fails() {
    let mut state = CodexSseState::default();
    let mut deltas = Vec::new();
    let mut on_event: Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)> =
        Some(&mut |event| {
            if let ModelEvent::OutputDelta(delta) = event {
                deltas.push(delta);
            }
            Ok(())
        });
    handle_codex_sse_line(
        r#"data: {"type":"response.output_text.delta","delta":"partial"}"#,
        &mut state,
        &mut on_event,
    )
    .unwrap();
    let error = handle_codex_sse_line(
        r#"data: {"type":"response.failed","response":{"error":{"type":"server_error","message":"generation failed"}}}"#,
        &mut state,
        &mut on_event,
    )
    .unwrap_err();

    assert_eq!(deltas, ["partial"]);
    assert_eq!(
        error.to_string(),
        "provider reported server_error: generation failed"
    );
}
