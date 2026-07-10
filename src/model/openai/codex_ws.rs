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
    events: Vec<ModelEvent>,
}

#[derive(Debug)]
enum CodexWsFailure {
    Transport(String),
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
        let plan = if mode.supports_incremental_websocket() {
            state.continuation.plan_request(&candidate, body)
        } else {
            state.continuation.reset();
            CodexRequestPlan {
                planned_delta: false,
                reset_reason: None,
                body,
            }
        };
        let frame = response_create_frame(plan.body.clone(), mode);

        match state
            .send_frame(&self.ws_url, tokens, frame, self.idle_timeout)
            .await
        {
            Ok(output) => {
                let output = match output.emit_events(on_event) {
                    Ok(output) => output,
                    Err(err) => {
                        state.connection = None;
                        state.continuation.reset();
                        return Err(err);
                    }
                };
                state
                    .continuation
                    .record_success(&candidate, output.response_id);
                Ok(CodexWsTurn::Completed(output.response))
            }
            Err(CodexWsFailure::Transport(_error)) => {
                state.connection = None;
                state.continuation.reset();
                Ok(CodexWsTurn::FullSseFallback)
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
        response_id: Option<String>,
    ) -> Result<(), ModelError> {
        let candidate = CodexContinuationCandidate::from_responses_body(body)?;
        let mut state = self.state.lock().await;
        state.continuation.record_success(&candidate, response_id);
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
        .map_err(|err| CodexWsFailure::Transport(err.to_string()))?
        .map_err(|err| CodexWsFailure::Transport(format!("websocket send failed: {err}")))?;

        collect_codex_ws_response(socket, idle_timeout).await
    }
}

async fn connect_codex_ws(
    ws_url: &str,
    tokens: &CodexTokens,
    idle_timeout: std::time::Duration,
) -> Result<CodexSocket, CodexWsFailure> {
    let mut request = ws_url
        .into_client_request()
        .map_err(|err| CodexWsFailure::Transport(format!("invalid websocket url: {err}")))?;
    let headers = request.headers_mut();
    headers.insert(USER_AGENT, HeaderValue::from_static("codex-cli"));
    headers.insert("originator", HeaderValue::from_static("codex_cli_rs"));
    headers.insert(
        "OpenAI-Beta",
        HeaderValue::from_static("responses_websockets=2026-02-06"),
    );
    let authorization = HeaderValue::from_str(&format!("Bearer {}", tokens.access_token))
        .map_err(|err| CodexWsFailure::Transport(format!("invalid bearer token header: {err}")))?;
    headers.insert(AUTHORIZATION, authorization);
    if let Some(account_id) = tokens.account_id.as_deref() {
        let account_id = HeaderValue::from_str(account_id).map_err(|err| {
            CodexWsFailure::Transport(format!("invalid ChatGPT account header: {err}"))
        })?;
        headers.insert("ChatGPT-Account-ID", account_id);
    }

    let (socket, _) = wait_for_stream_activity_for(connect_async(request), idle_timeout)
        .await
        .map_err(|err| CodexWsFailure::Transport(err.to_string()))?
        .map_err(|err| CodexWsFailure::Transport(format!("websocket connect failed: {err}")))?;
    Ok(socket)
}

async fn collect_codex_ws_response(
    socket: &mut CodexSocket,
    idle_timeout: std::time::Duration,
) -> Result<CodexWsCompleted, CodexWsFailure> {
    let mut state = CodexSseState::default();
    let mut events = Vec::new();
    let mut idle_deadline = StreamIdleDeadline::with_timeout(idle_timeout);
    loop {
        let Some(message) = idle_deadline
            .wait_for(socket.next())
            .await
            .map_err(|err| CodexWsFailure::Transport(err.to_string()))?
        else {
            break;
        };
        match message
            .map_err(|err| CodexWsFailure::Transport(format!("websocket receive failed: {err}")))?
        {
            Message::Text(text) => {
                let (completed, activity) =
                    handle_codex_ws_payload(&text, &mut state, &mut events)?;
                if completed {
                    let response = state.into_response().map_err(CodexWsFailure::Model)?;
                    return Ok(CodexWsCompleted { response, events });
                }
                if activity {
                    idle_deadline.record_activity();
                }
            }
            Message::Binary(bytes) => {
                let text = std::str::from_utf8(&bytes).map_err(|err| {
                    CodexWsFailure::Transport(format!(
                        "websocket binary frame contained invalid utf-8: {err}"
                    ))
                })?;
                let (completed, activity) = handle_codex_ws_payload(text, &mut state, &mut events)?;
                if completed {
                    let response = state.into_response().map_err(CodexWsFailure::Model)?;
                    return Ok(CodexWsCompleted { response, events });
                }
                if activity {
                    idle_deadline.record_activity();
                }
            }
            Message::Ping(_) | Message::Pong(_) => {}
            Message::Close(_) => {
                return Err(CodexWsFailure::Transport(
                    "websocket closed before response.completed".into(),
                ));
            }
            Message::Frame(_) => {}
        }
    }
    Err(CodexWsFailure::Transport(
        "websocket ended before response.completed".into(),
    ))
}

fn handle_codex_ws_payload(
    payload: &str,
    state: &mut CodexSseState,
    events: &mut Vec<ModelEvent>,
) -> Result<(bool, bool), CodexWsFailure> {
    let value = serde_json::from_str::<Value>(payload).map_err(|err| {
        CodexWsFailure::Transport(format!("websocket frame was not valid JSON: {err}"))
    })?;
    let mut collect_event = |event| {
        events.push(event);
        Ok(())
    };
    handle_codex_sse_line(
        &format!("data: {value}"),
        state,
        &mut Some(&mut collect_event as &mut dyn FnMut(ModelEvent) -> Result<(), ModelError>),
    )
    .map_err(CodexWsFailure::Model)?;
    let event_type = value.get("type").and_then(Value::as_str);
    Ok((
        event_type == Some("response.completed"),
        event_type.is_some_and(|event_type| event_type.starts_with("response.")),
    ))
}

impl CodexWsCompleted {
    fn emit_events(
        self,
        on_event: &mut Option<&mut dyn FnMut(ModelEvent) -> Result<(), ModelError>>,
    ) -> Result<CodexSseResponse, ModelError> {
        if let Some(on_event) = on_event.as_mut() {
            for event in self.events {
                on_event(event)?;
            }
        }
        Ok(self.response)
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

#[derive(Debug, Default)]
struct CodexContinuationState {
    snapshot: Option<CodexContinuationSnapshot>,
}

#[derive(Clone, Debug, PartialEq)]
struct CodexContinuationSnapshot {
    response_id: String,
    key: CodexContinuationKey,
    input: Vec<Value>,
}

#[derive(Clone, Debug, PartialEq)]
struct CodexContinuationCandidate {
    key: CodexContinuationKey,
    input: Vec<Value>,
}

#[derive(Clone, Debug, PartialEq)]
struct CodexContinuationKey {
    model: String,
    instructions: String,
    tools: Vec<Value>,
    tool_choice: Option<Value>,
    reasoning: Option<Value>,
    prompt_cache_key: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
struct CodexRequestPlan {
    planned_delta: bool,
    reset_reason: Option<CodexContinuationResetReason>,
    body: Value,
}

#[derive(Clone, Debug, PartialEq)]
enum CodexContinuationPlan {
    Full {
        reason: CodexContinuationFullReason,
    },
    Delta {
        previous_response_id: String,
        input: Vec<Value>,
        body: Value,
    },
}

#[derive(Clone, Debug, PartialEq)]
enum CodexContinuationFullReason {
    MissingPreviousResponse,
    EmptyDelta,
    Incompatible(CodexContinuationResetReason),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum CodexContinuationResetReason {
    ModelChanged,
    InstructionsChanged,
    ToolsChanged,
    ToolChoiceChanged,
    ReasoningChanged,
    PromptCacheKeyChanged,
    HistoryRewritten,
}

impl CodexContinuationCandidate {
    fn from_responses_body(body: &Value) -> Result<Self, ModelError> {
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .ok_or_else(|| ModelError::InvalidResponse("Codex body missing model".into()))?
            .to_string();
        let instructions = body
            .get("instructions")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let input = body
            .get("input")
            .and_then(Value::as_array)
            .ok_or_else(|| ModelError::InvalidResponse("Codex body missing input".into()))?
            .clone();
        let tools = body
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let tool_choice = body.get("tool_choice").cloned();
        let reasoning = body.get("reasoning").cloned();
        let prompt_cache_key = body
            .get("prompt_cache_key")
            .and_then(Value::as_str)
            .map(str::to_string);

        Ok(Self {
            key: CodexContinuationKey {
                model,
                instructions,
                tools,
                tool_choice,
                reasoning,
                prompt_cache_key,
            },
            input,
        })
    }

    fn delta_body(&self, previous_response_id: &str, input: Vec<Value>) -> Value {
        let mut body = json!({
            "model": self.key.model,
            "instructions": self.key.instructions,
            "input": input,
            "previous_response_id": previous_response_id,
            "store": false,
            "stream": true,
        });

        if let Some(prompt_cache_key) = &self.key.prompt_cache_key {
            body["prompt_cache_key"] = json!(prompt_cache_key);
        }
        if !self.key.tools.is_empty() {
            body["tools"] = json!(self.key.tools);
        }
        if let Some(tool_choice) = &self.key.tool_choice {
            body["tool_choice"] = tool_choice.clone();
        }
        if let Some(reasoning) = &self.key.reasoning {
            body["reasoning"] = reasoning.clone();
        }

        body
    }
}

impl CodexContinuationState {
    fn plan_request(
        &mut self,
        candidate: &CodexContinuationCandidate,
        full_body: Value,
    ) -> CodexRequestPlan {
        match self.plan_delta(candidate) {
            CodexContinuationPlan::Delta { body, .. } => CodexRequestPlan {
                planned_delta: true,
                reset_reason: None,
                body,
            },
            CodexContinuationPlan::Full { reason } => {
                let reset_reason = match reason {
                    CodexContinuationFullReason::Incompatible(reason) => {
                        self.reset();
                        Some(reason)
                    }
                    CodexContinuationFullReason::MissingPreviousResponse
                    | CodexContinuationFullReason::EmptyDelta => None,
                };
                CodexRequestPlan {
                    planned_delta: false,
                    reset_reason,
                    body: full_body,
                }
            }
        }
    }

    fn plan_delta(&self, candidate: &CodexContinuationCandidate) -> CodexContinuationPlan {
        let Some(snapshot) = &self.snapshot else {
            return CodexContinuationPlan::Full {
                reason: CodexContinuationFullReason::MissingPreviousResponse,
            };
        };
        if let Some(reason) = incompatible_reason(&snapshot.key, &candidate.key) {
            return CodexContinuationPlan::Full {
                reason: CodexContinuationFullReason::Incompatible(reason),
            };
        }
        if !input_has_prefix(&candidate.input, &snapshot.input) {
            return CodexContinuationPlan::Full {
                reason: CodexContinuationFullReason::Incompatible(
                    CodexContinuationResetReason::HistoryRewritten,
                ),
            };
        }
        let delta = candidate.input[snapshot.input.len()..].to_vec();
        if delta.is_empty() {
            return CodexContinuationPlan::Full {
                reason: CodexContinuationFullReason::EmptyDelta,
            };
        }
        CodexContinuationPlan::Delta {
            previous_response_id: snapshot.response_id.clone(),
            input: delta.clone(),
            body: candidate.delta_body(&snapshot.response_id, delta),
        }
    }

    fn record_success(
        &mut self,
        candidate: &CodexContinuationCandidate,
        response_id: Option<String>,
    ) {
        let Some(response_id) = response_id.filter(|id| !id.is_empty()) else {
            self.reset();
            return;
        };
        self.snapshot = Some(CodexContinuationSnapshot {
            response_id,
            key: candidate.key.clone(),
            input: candidate.input.clone(),
        });
    }

    fn reset(&mut self) {
        self.snapshot = None;
    }
}

fn incompatible_reason(
    previous: &CodexContinuationKey,
    next: &CodexContinuationKey,
) -> Option<CodexContinuationResetReason> {
    if previous.model != next.model {
        return Some(CodexContinuationResetReason::ModelChanged);
    }
    if previous.instructions != next.instructions {
        return Some(CodexContinuationResetReason::InstructionsChanged);
    }
    if previous.tools != next.tools {
        return Some(CodexContinuationResetReason::ToolsChanged);
    }
    if previous.tool_choice != next.tool_choice {
        return Some(CodexContinuationResetReason::ToolChoiceChanged);
    }
    if previous.reasoning != next.reasoning {
        return Some(CodexContinuationResetReason::ReasoningChanged);
    }
    if previous.prompt_cache_key != next.prompt_cache_key {
        return Some(CodexContinuationResetReason::PromptCacheKeyChanged);
    }
    None
}

fn input_has_prefix(input: &[Value], prefix: &[Value]) -> bool {
    input.len() >= prefix.len()
        && input
            .iter()
            .zip(prefix.iter())
            .all(|(input, prefix)| input == prefix)
}

#[cfg(test)]
#[path = "codex_ws_tests.rs"]
mod tests;
