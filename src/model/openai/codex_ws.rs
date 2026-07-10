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

        match state.send_frame(&self.ws_url, tokens, frame).await {
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
    ) -> Result<CodexWsCompleted, CodexWsFailure> {
        if self.connection.is_none() {
            self.connection = Some(connect_codex_ws(ws_url, tokens).await?);
        }
        let socket = self.connection.as_mut().expect("connection was just set");
        socket
            .send(Message::Text(frame.to_string().into()))
            .await
            .map_err(|err| CodexWsFailure::Transport(format!("websocket send failed: {err}")))?;

        collect_codex_ws_response(socket).await
    }
}

async fn connect_codex_ws(
    ws_url: &str,
    tokens: &CodexTokens,
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

    let (socket, _) = connect_async(request)
        .await
        .map_err(|err| CodexWsFailure::Transport(format!("websocket connect failed: {err}")))?;
    Ok(socket)
}

async fn collect_codex_ws_response(
    socket: &mut CodexSocket,
) -> Result<CodexWsCompleted, CodexWsFailure> {
    let mut state = CodexSseState::default();
    let mut events = Vec::new();
    while let Some(message) = socket.next().await {
        match message
            .map_err(|err| CodexWsFailure::Transport(format!("websocket receive failed: {err}")))?
        {
            Message::Text(text) => {
                if handle_codex_ws_payload(&text, &mut state, &mut events)? {
                    let response = state.into_response().map_err(CodexWsFailure::Model)?;
                    return Ok(CodexWsCompleted { response, events });
                }
            }
            Message::Binary(bytes) => {
                let text = std::str::from_utf8(&bytes).map_err(|err| {
                    CodexWsFailure::Transport(format!(
                        "websocket binary frame contained invalid utf-8: {err}"
                    ))
                })?;
                if handle_codex_ws_payload(text, &mut state, &mut events)? {
                    let response = state.into_response().map_err(CodexWsFailure::Model)?;
                    return Ok(CodexWsCompleted { response, events });
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
) -> Result<bool, CodexWsFailure> {
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
    Ok(value.get("type").and_then(Value::as_str) == Some("response.completed"))
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
mod tests {
    use super::*;
    use crate::model::ContentBlock;
    use serde_json::json;
    use std::sync::{Arc, Mutex as StdMutex};
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;

    fn body(input: Vec<Value>) -> Value {
        json!({
            "model": "gpt-5-codex",
            "instructions": "system",
            "input": input,
            "store": false,
            "stream": true,
            "prompt_cache_key": "rho:session",
            "tools": [{"type":"function","name":"read","parameters":{"type":"object"}}],
            "tool_choice": "auto",
            "reasoning": {"effort":"low","summary":"auto"},
        })
    }

    fn candidate(input: Vec<Value>) -> CodexContinuationCandidate {
        CodexContinuationCandidate::from_responses_body(&body(input)).unwrap()
    }

    fn tokens() -> CodexTokens {
        CodexTokens {
            access_token: "token".into(),
            refresh_token: None,
            id_token: None,
            account_id: Some("account".into()),
        }
    }

    async fn ws_server(expected_messages: usize) -> (String, Arc<StdMutex<Vec<Value>>>) {
        ws_server_connections(vec![expected_messages]).await
    }

    async fn ws_server_connections(
        expected_messages_by_connection: Vec<usize>,
    ) -> (String, Arc<StdMutex<Vec<Value>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let frames = Arc::new(StdMutex::new(Vec::new()));
        let server_frames = Arc::clone(&frames);
        tokio::spawn(async move {
            let mut response_index = 0;
            for expected_messages in expected_messages_by_connection {
                let (stream, _) = listener.accept().await.unwrap();
                let mut socket = accept_async(stream).await.unwrap();
                for _ in 0..expected_messages {
                    response_index += 1;
                    let message = socket.next().await.unwrap().unwrap();
                    let text = message.into_text().unwrap();
                    let frame: Value = serde_json::from_str(&text).unwrap();
                    server_frames.lock().unwrap().push(frame);
                    let response_id = format!("resp_{response_index}");
                    socket
                        .send(Message::Text(
                            json!({"type":"response.output_text.delta","delta":format!("ok{response_index}")})
                                .to_string()
                                .into(),
                        ))
                        .await
                        .unwrap();
                    socket
                        .send(Message::Text(
                            json!({
                                "type":"response.completed",
                                "response":{
                                    "id": response_id,
                                    "output_text": format!("ok{response_index}"),
                                    "output":[],
                                    "usage":{"input_tokens": 10, "output_tokens": 2}
                                }
                            })
                            .to_string()
                            .into(),
                        ))
                        .await
                        .unwrap();
                }
            }
        });
        (format!("ws://{addr}/responses"), frames)
    }

    async fn ws_server_closes_after_delta() -> (String, Arc<StdMutex<Vec<Value>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let frames = Arc::new(StdMutex::new(Vec::new()));
        let server_frames = Arc::clone(&frames);
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            let message = socket.next().await.unwrap().unwrap();
            let text = message.into_text().unwrap();
            let frame: Value = serde_json::from_str(&text).unwrap();
            server_frames.lock().unwrap().push(frame);
            socket
                .send(Message::Text(
                    json!({"type":"response.output_text.delta","delta":"partial"})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
        });
        (format!("ws://{addr}/responses"), frames)
    }

    #[test]
    fn builds_delta_body_when_next_input_extends_previous_input() {
        let first = candidate(vec![json!({"role":"user","content":"one"})]);
        let second = candidate(vec![
            json!({"role":"user","content":"one"}),
            json!({"role":"assistant","content":"two"}),
            json!({"role":"user","content":"three"}),
        ]);
        let mut state = CodexContinuationState::default();
        state.record_success(&first, Some("resp_1".into()));

        let plan = state.plan_delta(&second);

        let CodexContinuationPlan::Delta {
            previous_response_id,
            input,
            body,
        } = plan
        else {
            panic!("expected delta plan");
        };
        assert_eq!(previous_response_id, "resp_1");
        assert_eq!(
            input,
            vec![
                json!({"role":"assistant","content":"two"}),
                json!({"role":"user","content":"three"}),
            ]
        );
        assert_eq!(body["previous_response_id"], "resp_1");
        assert_eq!(body["input"], json!(input));
        assert_eq!(body["model"], "gpt-5-codex");
        assert_eq!(body["prompt_cache_key"], "rho:session");
        assert_eq!(body["tools"][0]["name"], "read");
        assert_eq!(body["reasoning"], json!({"effort":"low","summary":"auto"}));
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], true);
    }

    #[tokio::test]
    async fn responses_lite_websocket_request_sets_lite_client_metadata() {
        let (url, frames) = ws_server(1).await;
        let transport = CodexWsTransport::new_with_url(url);
        let mut on_event = None;

        transport
            .send_responses_turn(
                body(vec![json!({"role":"user","content":"one"})]),
                &tokens(),
                CodexRequestMode::ResponsesLite,
                &mut on_event,
            )
            .await
            .unwrap();

        let frames = frames.lock().unwrap();
        assert_eq!(
            frames[0]["client_metadata"]
                ["ws_request_header_x_openai_internal_codex_responses_lite"],
            "true"
        );
    }

    #[tokio::test]
    async fn responses_lite_websocket_requests_do_not_use_incomplete_continuation_state() {
        let (url, frames) = ws_server(2).await;
        let transport = CodexWsTransport::new_with_url(url);
        let mut on_event = None;

        transport
            .send_responses_turn(
                body(vec![json!({"role":"user","content":"one"})]),
                &tokens(),
                CodexRequestMode::ResponsesLite,
                &mut on_event,
            )
            .await
            .unwrap();
        transport
            .send_responses_turn(
                body(vec![
                    json!({"role":"user","content":"one"}),
                    json!({"role":"assistant","content":"two"}),
                    json!({"role":"user","content":"three"}),
                ]),
                &tokens(),
                CodexRequestMode::ResponsesLite,
                &mut on_event,
            )
            .await
            .unwrap();

        let frames = frames.lock().unwrap();
        assert_eq!(frames.len(), 2);
        assert!(frames[1].get("previous_response_id").is_none());
        assert_eq!(
            frames[1]["input"],
            json!([
                {"role":"user","content":"one"},
                {"role":"assistant","content":"two"},
                {"role":"user","content":"three"}
            ])
        );
    }

    #[tokio::test]
    async fn standard_full_history_requests_do_not_resend_server_output_as_a_delta() {
        let (url, frames) = ws_server(2).await;
        let transport = CodexWsTransport::new_with_url(url);
        let mut on_event = None;

        transport
            .send_responses_turn(
                body(vec![json!({"role":"user","content":"one"})]),
                &tokens(),
                CodexRequestMode::StandardFullHistory,
                &mut on_event,
            )
            .await
            .unwrap();
        transport
            .send_responses_turn(
                body(vec![
                    json!({"role":"user","content":"one"}),
                    json!({"type":"function_call","call_id":"call_1","name":"read","arguments":"{}"}),
                    json!({"type":"function_call_output","call_id":"call_1","output":"done"}),
                ]),
                &tokens(),
                CodexRequestMode::StandardFullHistory,
                &mut on_event,
            )
            .await
            .unwrap();

        let frames = frames.lock().unwrap();
        assert_eq!(frames.len(), 2);
        assert!(frames[1].get("previous_response_id").is_none());
        assert_eq!(
            frames[1]["input"],
            json!([
                {"role":"user","content":"one"},
                {"type":"function_call","call_id":"call_1","name":"read","arguments":"{}"},
                {"type":"function_call_output","call_id":"call_1","output":"done"},
            ])
        );
    }

    #[tokio::test]
    async fn first_websocket_request_sends_full_input_without_previous_response_id() {
        let (url, frames) = ws_server(1).await;
        let transport = CodexWsTransport::new_with_url(url);
        let mut on_event = None;

        let turn = transport
            .send_responses_turn(
                body(vec![json!({"role":"user","content":"one"})]),
                &tokens(),
                CodexRequestMode::Standard,
                &mut on_event,
            )
            .await
            .unwrap();

        assert!(matches!(turn, CodexWsTurn::Completed(_)));
        let frames = frames.lock().unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["type"], "response.create");
        assert!(frames[0].get("previous_response_id").is_none());
        assert_eq!(frames[0]["input"], json!([{"role":"user","content":"one"}]));
    }

    #[tokio::test]
    async fn compatible_websocket_request_sends_delta_with_previous_response_id() {
        let (url, frames) = ws_server(2).await;
        let transport = CodexWsTransport::new_with_url(url);
        let mut on_event = None;

        transport
            .send_responses_turn(
                body(vec![json!({"role":"user","content":"one"})]),
                &tokens(),
                CodexRequestMode::Standard,
                &mut on_event,
            )
            .await
            .unwrap();
        let turn = transport
            .send_responses_turn(
                body(vec![
                    json!({"role":"user","content":"one"}),
                    json!({"role":"assistant","content":"two"}),
                    json!({"role":"user","content":"three"}),
                ]),
                &tokens(),
                CodexRequestMode::Standard,
                &mut on_event,
            )
            .await
            .unwrap();

        let CodexWsTurn::Completed(ModelResponse::Assistant(blocks)) = turn else {
            panic!("expected websocket completion");
        };
        assert!(matches!(
            blocks.as_slice(),
            [ContentBlock::Text(text)] if text == "ok2"
        ));
        let frames = frames.lock().unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[1]["previous_response_id"], "resp_1");
        assert_eq!(
            frames[1]["input"],
            json!([
                {"role":"assistant","content":"two"},
                {"role":"user","content":"three"}
            ])
        );
    }

    #[test]
    fn falls_back_to_full_request_without_previous_response_id() {
        let state = CodexContinuationState::default();
        let plan = state.plan_delta(&candidate(vec![json!({"role":"user","content":"one"})]));

        assert_eq!(
            plan,
            CodexContinuationPlan::Full {
                reason: CodexContinuationFullReason::MissingPreviousResponse
            }
        );
    }

    #[test]
    fn resets_when_history_is_rewritten_by_compaction() {
        let first = candidate(vec![
            json!({"role":"user","content":"old"}),
            json!({"role":"assistant","content":"answer"}),
        ]);
        let compacted_body = body(vec![
            json!({"role":"user","content":"summary of old conversation"}),
            json!({"role":"user","content":"new"}),
        ]);
        let compacted = CodexContinuationCandidate::from_responses_body(&compacted_body).unwrap();
        let mut state = CodexContinuationState::default();
        state.record_success(&first, Some("resp_1".into()));

        let plan = state.plan_request(&compacted, compacted_body.clone());

        assert!(!plan.planned_delta);
        assert_eq!(
            plan.reset_reason,
            Some(CodexContinuationResetReason::HistoryRewritten)
        );
        assert_eq!(plan.body, compacted_body);
        assert_eq!(
            state.plan_delta(&compacted),
            CodexContinuationPlan::Full {
                reason: CodexContinuationFullReason::MissingPreviousResponse
            }
        );
    }

    #[test]
    fn resets_when_tools_change() {
        let first = candidate(vec![json!({"role":"user","content":"one"})]);
        let mut changed_body = body(vec![
            json!({"role":"user","content":"one"}),
            json!({"role":"user","content":"two"}),
        ]);
        changed_body["tools"] = json!([{ "type":"function", "name":"write" }]);
        let changed = CodexContinuationCandidate::from_responses_body(&changed_body).unwrap();
        let mut state = CodexContinuationState::default();
        state.record_success(&first, Some("resp_1".into()));

        let plan = state.plan_request(&changed, changed_body);

        assert_eq!(
            plan.reset_reason,
            Some(CodexContinuationResetReason::ToolsChanged)
        );
        assert_eq!(
            state.plan_delta(&changed),
            CodexContinuationPlan::Full {
                reason: CodexContinuationFullReason::MissingPreviousResponse
            }
        );
    }

    #[test]
    fn resets_when_model_changes() {
        assert_reset_reason_for_body_change(
            |body| body["model"] = json!("gpt-5-codex-alt"),
            CodexContinuationResetReason::ModelChanged,
        );
    }

    #[test]
    fn resets_when_reasoning_changes() {
        assert_reset_reason_for_body_change(
            |body| body["reasoning"] = json!({"effort":"high","summary":"auto"}),
            CodexContinuationResetReason::ReasoningChanged,
        );
    }

    #[test]
    fn resets_when_prompt_cache_key_changes() {
        assert_reset_reason_for_body_change(
            |body| body["prompt_cache_key"] = json!("rho:other"),
            CodexContinuationResetReason::PromptCacheKeyChanged,
        );
    }

    #[test]
    fn resets_when_tool_choice_changes() {
        assert_reset_reason_for_body_change(
            |body| body["tool_choice"] = json!("none"),
            CodexContinuationResetReason::ToolChoiceChanged,
        );
    }

    fn assert_reset_reason_for_body_change(
        mutate: impl FnOnce(&mut Value),
        expected: CodexContinuationResetReason,
    ) {
        let first = candidate(vec![json!({"role":"user","content":"one"})]);
        let mut changed_body = body(vec![
            json!({"role":"user","content":"one"}),
            json!({"role":"user","content":"two"}),
        ]);
        mutate(&mut changed_body);
        let changed = CodexContinuationCandidate::from_responses_body(&changed_body).unwrap();
        let mut state = CodexContinuationState::default();
        state.record_success(&first, Some("resp_1".into()));

        let plan = state.plan_request(&changed, changed_body);

        assert_eq!(plan.reset_reason, Some(expected));
    }

    #[tokio::test]
    async fn websocket_error_resets_continuation_and_returns_full_sse_fallback() {
        let (url, frames) = ws_server_connections(vec![1, 1]).await;
        let transport = CodexWsTransport::new_with_url(url);
        let mut on_event = None;
        transport
            .send_responses_turn(
                body(vec![json!({"role":"user","content":"one"})]),
                &tokens(),
                CodexRequestMode::Standard,
                &mut on_event,
            )
            .await
            .unwrap();

        let outcome = transport
            .send_responses_turn(
                body(vec![
                    json!({"role":"user","content":"one"}),
                    json!({"role":"user","content":"two"}),
                ]),
                &tokens(),
                CodexRequestMode::Standard,
                &mut on_event,
            )
            .await
            .unwrap();

        assert!(matches!(outcome, CodexWsTurn::FullSseFallback));

        transport
            .send_responses_turn(
                body(vec![
                    json!({"role":"user","content":"one"}),
                    json!({"role":"user","content":"two"}),
                ]),
                &tokens(),
                CodexRequestMode::Standard,
                &mut on_event,
            )
            .await
            .unwrap();
        let frames = frames.lock().unwrap();
        assert_eq!(frames.len(), 2);
        assert!(frames[1].get("previous_response_id").is_none());
    }

    #[tokio::test]
    async fn websocket_fallback_does_not_emit_partial_events() {
        let (url, frames) = ws_server_closes_after_delta().await;
        let transport = CodexWsTransport::new_with_url(url);
        let mut deltas = Vec::new();
        {
            let mut collect_event = |event| {
                if let ModelEvent::OutputDelta(delta) = event {
                    deltas.push(delta);
                }
                Ok(())
            };
            let mut on_event =
                Some(&mut collect_event as &mut dyn FnMut(ModelEvent) -> Result<(), ModelError>);

            let outcome = transport
                .send_responses_turn(
                    body(vec![json!({"role":"user","content":"one"})]),
                    &tokens(),
                    CodexRequestMode::Standard,
                    &mut on_event,
                )
                .await
                .unwrap();

            assert!(matches!(outcome, CodexWsTurn::FullSseFallback));
        }

        assert!(deltas.is_empty());
        assert_eq!(frames.lock().unwrap().len(), 1);
    }

    #[test]
    fn derives_websocket_url_from_codex_api_base() {
        assert_eq!(
            codex_ws_url("https://chatgpt.com/backend-api/codex"),
            "wss://chatgpt.com/backend-api/codex/responses"
        );
        assert_eq!(
            codex_ws_url("http://127.0.0.1:1234/codex/"),
            "ws://127.0.0.1:1234/codex/responses"
        );
    }
}
