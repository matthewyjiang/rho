use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{AUTHORIZATION, USER_AGENT};
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use crate::credentials::CodexTokens;
use crate::model::{ModelError, ModelEvent, ModelResponse};
use crate::provider_backend::stream_timeout::{wait_for_stream_activity_for, StreamIdleDeadline};

use super::codex_continuation::{
    CodexContinuationCandidate, CodexContinuationResponse, CodexContinuationState,
};
use super::codex_request::CodexRequestMode;
use super::stream::{handle_codex_sse_line, CodexSseResponse, CodexSseState};

/// WebSocket transport for Codex Responses turns.
///
/// The transport owns the session continuation snapshot and the WebSocket
/// connection. Callers pass a complete Responses body; the transport decides
/// whether the next `response.create` frame can use a delta with
/// `previous_response_id` or must send the full input. If the WebSocket path is
/// unavailable or hits a retryable connection failure, callers receive an
/// explicit full-SSE fallback instruction and the stale continuation state is
/// cleared.
pub(super) struct CodexWsTransport {
    ws_url: String,
    idle_timeout: std::time::Duration,
    state: Mutex<CodexWsState>,
}

struct CodexWsState {
    continuation: CodexContinuationState,
    connection: Option<CodexSocket>,
}

type CodexSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Debug)]
pub(super) enum CodexWsTurn {
    Completed(ModelResponse),
    /// The WebSocket transport could not complete the turn before emitting any
    /// caller-visible stream events. Continuation state has already been reset,
    /// so the caller can safely retry the same full Responses body over SSE.
    FullSseFallback,
}

struct CodexWsCompleted {
    response: CodexSseResponse,
    server_output_items: Vec<Value>,
}

#[derive(Debug)]
enum CodexWsFailure {
    Transport {
        message: String,
        events_emitted: bool,
    },
    Model(ModelError),
}

impl CodexWsTransport {
    pub(super) fn new(api_base: &str) -> Self {
        Self::new_with_url(codex_ws_url(api_base))
    }

    fn new_with_url(ws_url: String) -> Self {
        Self {
            ws_url,
            idle_timeout: crate::provider_backend::stream_timeout::STREAM_IDLE_TIMEOUT,
            state: Mutex::new(CodexWsState {
                continuation: CodexContinuationState::default(),
                connection: None,
            }),
        }
    }

    pub(super) async fn send_responses_turn(
        &self,
        body: Value,
        tokens: &CodexTokens,
        mode: CodexRequestMode,
        on_event: &mut Option<&mut dyn FnMut(ModelEvent) -> Result<(), ModelError>>,
    ) -> Result<CodexWsTurn, ModelError> {
        let candidate = CodexContinuationCandidate::from_responses_body(&body)?;
        let mut state = self.state.lock().await;
        let body = if mode.supports_incremental_websocket() {
            state.continuation.continuation_body(&candidate, body)
        } else {
            state.continuation.reset();
            body
        };
        let frame = response_create_frame(body, mode);

        match state
            .send_frame(&self.ws_url, tokens, frame, self.idle_timeout, on_event)
            .await
        {
            Ok(output) => {
                let CodexWsCompleted {
                    response,
                    server_output_items,
                } = output;
                let continuation_response = CodexContinuationResponse::from_response(
                    &response.response,
                    response.response_id.clone(),
                    server_output_items,
                );
                state
                    .continuation
                    .record_success(&candidate, continuation_response);
                Ok(CodexWsTurn::Completed(response.response))
            }
            Err(CodexWsFailure::Transport {
                events_emitted: false,
                ..
            }) => {
                state.connection = None;
                state.continuation.reset();
                Ok(CodexWsTurn::FullSseFallback)
            }
            Err(CodexWsFailure::Transport {
                message,
                events_emitted: true,
            }) => {
                state.connection = None;
                state.continuation.reset();
                Err(ModelError::InvalidResponse(format!(
                    "Codex WebSocket failed after streaming output: {message}"
                )))
            }
            Err(CodexWsFailure::Model(err)) => {
                state.connection = None;
                state.continuation.reset();
                Err(err)
            }
        }
    }

    pub(super) async fn record_full_request_success(
        &self,
        body: &Value,
        response: &CodexSseResponse,
    ) -> Result<(), ModelError> {
        let candidate = CodexContinuationCandidate::from_responses_body(body)?;
        let continuation_response = CodexContinuationResponse::from_response(
            &response.response,
            response.response_id.clone(),
            Vec::new(),
        );
        let mut state = self.state.lock().await;
        state
            .continuation
            .record_success(&candidate, continuation_response);
        Ok(())
    }

    pub(super) async fn reset(&self) {
        let mut state = self.state.lock().await;
        state.connection = None;
        state.continuation.reset();
    }
}

impl CodexWsState {
    async fn send_frame(
        &mut self,
        ws_url: &str,
        tokens: &CodexTokens,
        frame: Value,
        idle_timeout: std::time::Duration,
        on_event: &mut Option<&mut dyn FnMut(ModelEvent) -> Result<(), ModelError>>,
    ) -> Result<CodexWsCompleted, CodexWsFailure> {
        if self.connection.is_none() {
            self.connection = Some(connect_codex_ws(ws_url, tokens, idle_timeout).await?);
        }
        let socket = self.connection.as_mut().expect("connection was just set");
        wait_for_stream_activity_for(
            socket.send(Message::Text(frame.to_string().into())),
            idle_timeout,
        )
        .await
        .map_err(|err| CodexWsFailure::Transport {
            message: err.to_string(),
            events_emitted: false,
        })?
        .map_err(|err| CodexWsFailure::Transport {
            message: format!("websocket send failed: {err}"),
            events_emitted: false,
        })?;

        collect_codex_ws_response(socket, idle_timeout, on_event).await
    }
}

async fn connect_codex_ws(
    ws_url: &str,
    tokens: &CodexTokens,
    idle_timeout: std::time::Duration,
) -> Result<CodexSocket, CodexWsFailure> {
    let mut request = ws_url
        .into_client_request()
        .map_err(|err| CodexWsFailure::Transport {
            message: format!("invalid websocket url: {err}"),
            events_emitted: false,
        })?;
    let headers = request.headers_mut();
    headers.insert(USER_AGENT, HeaderValue::from_static("codex-cli"));
    headers.insert("originator", HeaderValue::from_static("codex_cli_rs"));
    headers.insert(
        "OpenAI-Beta",
        HeaderValue::from_static("responses_websockets=2026-02-06"),
    );
    let authorization =
        HeaderValue::from_str(&format!("Bearer {}", tokens.access_token)).map_err(|err| {
            CodexWsFailure::Transport {
                message: format!("invalid bearer token header: {err}"),
                events_emitted: false,
            }
        })?;
    headers.insert(AUTHORIZATION, authorization);
    if let Some(account_id) = tokens.account_id.as_deref() {
        let account_id =
            HeaderValue::from_str(account_id).map_err(|err| CodexWsFailure::Transport {
                message: format!("invalid ChatGPT account header: {err}"),
                events_emitted: false,
            })?;
        headers.insert("ChatGPT-Account-ID", account_id);
    }

    let (socket, _) = wait_for_stream_activity_for(connect_async(request), idle_timeout)
        .await
        .map_err(|err| CodexWsFailure::Transport {
            message: err.to_string(),
            events_emitted: false,
        })?
        .map_err(|err| CodexWsFailure::Transport {
            message: format!("websocket connect failed: {err}"),
            events_emitted: false,
        })?;
    Ok(socket)
}

async fn collect_codex_ws_response(
    socket: &mut CodexSocket,
    idle_timeout: std::time::Duration,
    on_event: &mut Option<&mut dyn FnMut(ModelEvent) -> Result<(), ModelError>>,
) -> Result<CodexWsCompleted, CodexWsFailure> {
    let mut state = CodexSseState::default();
    let mut server_output_items = Vec::new();
    let mut events_emitted = false;
    let mut idle_deadline = StreamIdleDeadline::with_timeout(idle_timeout);
    loop {
        let Some(message) = idle_deadline.wait_for(socket.next()).await.map_err(|err| {
            CodexWsFailure::Transport {
                message: err.to_string(),
                events_emitted,
            }
        })?
        else {
            break;
        };
        let message = message.map_err(|err| CodexWsFailure::Transport {
            message: format!("websocket receive failed: {err}"),
            events_emitted,
        })?;
        let text = match message {
            Message::Text(text) => text.to_string(),
            Message::Binary(bytes) => std::str::from_utf8(&bytes)
                .map_err(|err| CodexWsFailure::Transport {
                    message: format!("websocket binary frame contained invalid utf-8: {err}"),
                    events_emitted,
                })?
                .to_string(),
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Close(_) => {
                return Err(CodexWsFailure::Transport {
                    message: "websocket closed before response.completed".into(),
                    events_emitted,
                });
            }
            Message::Frame(_) => continue,
        };
        let payload =
            serde_json::from_str::<Value>(&text).map_err(|err| CodexWsFailure::Transport {
                message: format!("websocket frame was not valid JSON: {err}"),
                events_emitted,
            })?;
        collect_server_output_item(&payload, &mut server_output_items);
        let (completed, activity) =
            handle_codex_ws_value(&payload, &mut state, on_event, &mut events_emitted)?;
        if completed {
            let response = state.into_response().map_err(CodexWsFailure::Model)?;
            return Ok(CodexWsCompleted {
                response,
                server_output_items,
            });
        }
        if activity {
            idle_deadline.record_activity();
        }
    }
    Err(CodexWsFailure::Transport {
        message: "websocket ended before response.completed".into(),
        events_emitted,
    })
}

fn handle_codex_ws_value(
    value: &Value,
    state: &mut CodexSseState,
    on_event: &mut Option<&mut dyn FnMut(ModelEvent) -> Result<(), ModelError>>,
    events_emitted: &mut bool,
) -> Result<(bool, bool), CodexWsFailure> {
    let mut emit_event = |event| {
        if let Some(on_event) = on_event.as_mut() {
            on_event(event)?;
            *events_emitted = true;
        }
        Ok(())
    };
    handle_codex_sse_line(
        &format!("data: {value}"),
        state,
        &mut Some(&mut emit_event as &mut dyn FnMut(ModelEvent) -> Result<(), ModelError>),
    )
    .map_err(CodexWsFailure::Model)?;
    let event_type = value.get("type").and_then(Value::as_str);
    Ok((
        event_type == Some("response.completed"),
        event_type.is_some_and(|event_type| event_type.starts_with("response.")),
    ))
}

fn collect_server_output_item(payload: &Value, output_items: &mut Vec<Value>) {
    if payload.get("type").and_then(Value::as_str) == Some("response.output_item.done") {
        if let Some(item) = payload.get("item") {
            output_items.push(item.clone());
        }
    }
}

fn response_create_frame(mut body: Value, mode: CodexRequestMode) -> Value {
    if mode.uses_responses_lite() {
        body["client_metadata"] = json!({
            "ws_request_header_x_openai_internal_codex_responses_lite": "true",
        });
    }
    body["type"] = json!("response.create");
    body
}

fn codex_ws_url(api_base: &str) -> String {
    let trimmed = api_base.trim_end_matches('/');
    let websocket_base = if let Some(rest) = trimmed.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        trimmed.to_string()
    };
    format!("{websocket_base}/responses")
}

#[cfg(test)]
#[path = "codex_ws_tests.rs"]
mod tests;
