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
use crate::model::{ModelError, ModelEvent};
use crate::provider_backend::stream_timeout::{wait_for_stream_activity_for, StreamIdleDeadline};
use rho_sdk::provider::ProviderSteeringReceiver;

use super::codex_continuation::{
    CodexContinuationCandidate, CodexContinuationResponse, CodexContinuationState,
};
use super::codex_request::codex_routing_hint;
use super::codex_steer::{
    is_steer_pending_required_input, steer_event_type, steer_frame, steer_items, PendingSteer,
    SteerMatch, SteerMode,
};
use crate::protocol::openai_responses::{
    handle_codex_sse_value, is_codex_turn_complete, CodexSseResponse, CodexSseState, CodexTransport,
};

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
    connection: Option<CodexConnection>,
    pending_steer: Option<PendingSteer>,
    /// Set while a turn is in flight, cleared when that turn records a result.
    ///
    /// A cancelled turn is dropped part way through an await, and dropping
    /// cannot run the async cleanup that clearing this state needs. Such a turn
    /// therefore leaves the flag set, and [`CodexWsState::open_turn`] treats
    /// that as a signal to discard the socket and continuation it left behind.
    turn_open: bool,
}

impl CodexWsState {
    /// Starts a turn, discarding anything an abandoned turn left cached.
    fn open_turn(&mut self) {
        if self.turn_open {
            self.discard();
        }
        self.turn_open = true;
    }

    /// Drops the cached socket and continuation snapshot.
    fn discard(&mut self) {
        self.connection = None;
        self.continuation.reset();
        self.pending_steer = None;
        self.turn_open = false;
    }
}

type CodexSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct CodexConnection {
    socket: CodexSocket,
    routing_hint: String,
}

#[derive(Debug, PartialEq)]
pub(super) enum CodexWsTurn {
    Completed(CodexSseResponse),
    /// The WebSocket transport could not complete the turn before emitting any
    /// caller-visible stream events. Continuation state has already been reset,
    /// so the caller can safely retry `body` over SSE.
    FullSseFallback {
        request_submitted: bool,
        /// The full Responses body the caller passed in, handed back so the
        /// SSE retry does not need a copy taken before every WebSocket turn.
        body: Value,
    },
}

struct CodexWsCompleted {
    response: CodexSseResponse,
    server_output_items: Vec<Value>,
    pending_steer: Option<PendingSteer>,
}

/// The `response.create` frame for one turn, plus the body an SSE retry needs.
///
/// An incremental turn frames a small delta and holds the caller's full body
/// aside. A full turn frames that body directly and recovers it from the frame
/// only if the turn fails. Either way the conversation history exists once, so
/// no WebSocket turn pays for a fallback that almost never happens.
struct CodexWsFrame {
    frame: Value,
    /// Set only when the frame carries a delta rather than the full body.
    full_body: Option<Value>,
}

impl CodexWsFrame {
    fn new(
        continuation: &mut CodexContinuationState,
        candidate: &CodexContinuationCandidate,
        body: Value,
    ) -> Self {
        let delta = continuation.continuation_delta(candidate);
        match delta {
            Some(delta) => Self {
                frame: response_create_frame(delta),
                full_body: Some(body),
            },
            None => Self {
                frame: response_create_frame(body),
                full_body: None,
            },
        }
    }

    fn into_full_body(self) -> Value {
        self.full_body
            .unwrap_or_else(|| response_body_from_frame(self.frame))
    }
}

#[derive(Debug)]
enum CodexWsFailure {
    BeforeRequest {
        _message: String,
    },
    Transport {
        message: String,
        events_emitted: bool,
    },
    Model(ModelError),
}

impl CodexWsFailure {
    fn into_turn(self, body: Value) -> Result<CodexWsTurn, ModelError> {
        match self {
            Self::BeforeRequest { .. } => Ok(CodexWsTurn::FullSseFallback {
                request_submitted: false,
                body,
            }),
            Self::Transport {
                events_emitted: false,
                ..
            } => Ok(CodexWsTurn::FullSseFallback {
                request_submitted: true,
                body,
            }),
            Self::Transport {
                message,
                events_emitted: true,
            } => Err(ModelError::StreamFailedAfterOutput { message }),
            Self::Model(error) => Err(error),
        }
    }
}

impl CodexWsTransport {
    pub(super) fn new(api_base: &str) -> Self {
        Self::new_with_url(codex_ws_url(api_base))
    }

    pub(super) fn new_with_url(ws_url: String) -> Self {
        Self {
            ws_url,
            idle_timeout: crate::provider_backend::stream_timeout::STREAM_IDLE_TIMEOUT,
            state: Mutex::new(CodexWsState {
                continuation: CodexContinuationState::default(),
                connection: None,
                pending_steer: None,
                turn_open: false,
            }),
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) async fn send_responses_turn(
        &self,
        body: Value,
        tokens: &CodexTokens,
        on_event: &mut Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)>,
    ) -> Result<CodexWsTurn, ModelError> {
        let mut steering = None;
        self.send_responses_turn_steerable(body, tokens, on_event, &mut steering)
            .await
    }

    pub(super) async fn send_responses_turn_steerable(
        &self,
        body: Value,
        tokens: &CodexTokens,
        on_event: &mut Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)>,
        steering: &mut Option<ProviderSteeringReceiver>,
    ) -> Result<CodexWsTurn, ModelError> {
        let candidate = CodexContinuationCandidate::from_responses_body(&body)?;
        let mut state = self.state.lock().await;
        state.open_turn();
        let pending_match = state
            .pending_steer
            .as_ref()
            .map(|pending| pending.matches(&candidate));
        match pending_match {
            Some(SteerMatch::Reuse) => {
                let output = state
                    .read_open_socket(self.idle_timeout, on_event, steering, &candidate)
                    .await;
                return finish_ws_turn(&mut state, candidate, body, output, /*reuse*/ true);
            }
            Some(SteerMatch::FullReplay) => {
                state.discard();
                state.open_turn();
            }
            None => {}
        }
        let turn = CodexWsFrame::new(&mut state.continuation, &candidate, body);

        let output = state
            .send_frame(
                &self.ws_url,
                tokens,
                &turn.frame,
                self.idle_timeout,
                on_event,
                steering,
                Some(&candidate),
            )
            .await;
        finish_ws_turn(&mut state, candidate, turn.into_full_body(), output, false)
    }

    pub(super) async fn send_responses_turn_silent(
        &self,
        body: Value,
        tokens: &CodexTokens,
    ) -> Result<CodexWsTurn, ModelError> {
        let candidate = CodexContinuationCandidate::from_responses_body(&body)?;
        let mut state = self.state.lock().await;
        state.open_turn();
        let turn = CodexWsFrame::new(&mut state.continuation, &candidate, body);

        match state
            .send_frame_silent(&self.ws_url, tokens, &turn.frame, self.idle_timeout)
            .await
        {
            Ok(output) => {
                let CodexWsCompleted {
                    response,
                    server_output_items,
                    ..
                } = output;
                let continuation_response = CodexContinuationResponse::from_response(
                    &response.response,
                    response.response_id.clone(),
                    server_output_items,
                );
                state
                    .continuation
                    .record_success(candidate, continuation_response);
                state.turn_open = false;
                Ok(CodexWsTurn::Completed(response))
            }
            Err(failure) => {
                state.discard();
                failure.into_turn(turn.into_full_body())
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
            .record_success(candidate, continuation_response);
        state.turn_open = false;
        Ok(())
    }

    pub(super) async fn reset(&self) {
        self.state.lock().await.discard();
    }
}

impl CodexWsState {
    #[allow(clippy::too_many_arguments)]
    async fn send_frame(
        &mut self,
        ws_url: &str,
        tokens: &CodexTokens,
        frame: &Value,
        idle_timeout: std::time::Duration,
        on_event: &mut Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)>,
        steering: &mut Option<ProviderSteeringReceiver>,
        candidate: Option<&CodexContinuationCandidate>,
    ) -> Result<CodexWsCompleted, CodexWsFailure> {
        let socket = self
            .send_on_routed_socket(ws_url, tokens, frame, idle_timeout)
            .await?;
        collect_codex_ws_response(socket, idle_timeout, on_event, steering, candidate).await
    }

    async fn send_on_routed_socket(
        &mut self,
        ws_url: &str,
        tokens: &CodexTokens,
        frame: &Value,
        idle_timeout: std::time::Duration,
    ) -> Result<&mut CodexSocket, CodexWsFailure> {
        let routing_hint = codex_routing_hint(frame).map_err(CodexWsFailure::Model)?;
        // Conservatively refresh handshake routing when request properties change;
        // do not assume a later frame can reroute an established connection.
        if self
            .connection
            .as_ref()
            .is_none_or(|connection| connection.routing_hint != routing_hint)
        {
            // Release the old connection before opening its replacement so a
            // routing change does not temporarily consume two connection slots.
            self.connection = None;
            let socket = connect_codex_ws(ws_url, tokens, &routing_hint, idle_timeout).await?;
            self.connection = Some(CodexConnection {
                socket,
                routing_hint,
            });
        }
        let socket = &mut self
            .connection
            .as_mut()
            .expect("connection was just set")
            .socket;
        wait_for_stream_activity_for(
            socket.send(Message::Text(frame.to_string().into())),
            idle_timeout,
        )
        .await
        .map_err(|err| CodexWsFailure::BeforeRequest {
            _message: err.to_string(),
        })?
        .map_err(|err| CodexWsFailure::BeforeRequest {
            _message: format!("websocket send failed: {err}"),
        })?;

        Ok(socket)
    }

    async fn read_open_socket(
        &mut self,
        idle_timeout: std::time::Duration,
        on_event: &mut Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)>,
        steering: &mut Option<ProviderSteeringReceiver>,
        candidate: &CodexContinuationCandidate,
    ) -> Result<CodexWsCompleted, CodexWsFailure> {
        let socket = &mut self
            .connection
            .as_mut()
            .ok_or(CodexWsFailure::BeforeRequest {
                _message: "steered continuation reused a closed websocket".into(),
            })?
            .socket;
        collect_codex_ws_response(socket, idle_timeout, on_event, steering, Some(candidate)).await
    }

    async fn send_frame_silent(
        &mut self,
        ws_url: &str,
        tokens: &CodexTokens,
        frame: &Value,
        idle_timeout: std::time::Duration,
    ) -> Result<CodexWsCompleted, CodexWsFailure> {
        let socket = self
            .send_on_routed_socket(ws_url, tokens, frame, idle_timeout)
            .await?;
        collect_codex_ws_response_silent(socket, idle_timeout).await
    }
}

async fn connect_codex_ws(
    ws_url: &str,
    tokens: &CodexTokens,
    routing_hint: &str,
    idle_timeout: std::time::Duration,
) -> Result<CodexSocket, CodexWsFailure> {
    let mut request =
        ws_url
            .into_client_request()
            .map_err(|err| CodexWsFailure::BeforeRequest {
                _message: format!("invalid websocket url: {err}"),
            })?;
    let headers = request.headers_mut();
    headers.insert(USER_AGENT, HeaderValue::from_static("codex-cli"));
    headers.insert("originator", HeaderValue::from_static("codex_cli_rs"));
    headers.insert(
        "OpenAI-Beta",
        HeaderValue::from_static("responses_websockets=2026-02-06"),
    );
    let routing_hint =
        HeaderValue::from_str(routing_hint).map_err(|err| CodexWsFailure::BeforeRequest {
            _message: format!("invalid Codex routing hint header: {err}"),
        })?;
    headers.insert("x-codex-routing-hint", routing_hint);
    let authorization =
        HeaderValue::from_str(&format!("Bearer {}", tokens.access_token)).map_err(|err| {
            CodexWsFailure::BeforeRequest {
                _message: format!("invalid bearer token header: {err}"),
            }
        })?;
    headers.insert(AUTHORIZATION, authorization);
    if let Some(account_id) = tokens.account_id.as_deref() {
        let account_id =
            HeaderValue::from_str(account_id).map_err(|err| CodexWsFailure::BeforeRequest {
                _message: format!("invalid ChatGPT account header: {err}"),
            })?;
        headers.insert("ChatGPT-Account-ID", account_id);
    }

    let (socket, _) = wait_for_stream_activity_for(connect_async(request), idle_timeout)
        .await
        .map_err(|err| CodexWsFailure::BeforeRequest {
            _message: err.to_string(),
        })?
        .map_err(|err| CodexWsFailure::BeforeRequest {
            _message: format!("websocket connect failed: {err}"),
        })?;
    Ok(socket)
}

async fn collect_codex_ws_response(
    socket: &mut CodexSocket,
    idle_timeout: std::time::Duration,
    on_event: &mut Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)>,
    steering: &mut Option<ProviderSteeringReceiver>,
    candidate: Option<&CodexContinuationCandidate>,
) -> Result<CodexWsCompleted, CodexWsFailure> {
    let mut state = CodexSseState::default();
    let mut server_output_items = Vec::new();
    let mut events_emitted = false;
    let mut idle_deadline = StreamIdleDeadline::with_timeout(idle_timeout);
    let mut session = SteerCollect::default();
    loop {
        tokio::select! {
            message = idle_deadline.wait_for(socket.next()) => {
                let Some(message) = message.map_err(|err| CodexWsFailure::Transport {
                    message: err.to_string(),
                    events_emitted,
                })?
                else {
                    break;
                };
                let message = message.map_err(|err| CodexWsFailure::Transport {
                    message: format!("websocket receive failed: {err}"),
                    events_emitted,
                })?;
                let text = match &message {
                    Message::Text(text) => text.as_str(),
                    Message::Binary(bytes) => {
                        std::str::from_utf8(bytes).map_err(|err| CodexWsFailure::Transport {
                            message: format!("websocket binary frame contained invalid utf-8: {err}"),
                            events_emitted,
                        })?
                    }
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
                    serde_json::from_str::<Value>(text).map_err(|err| CodexWsFailure::Transport {
                        message: format!("websocket frame was not valid JSON: {err}"),
                        events_emitted,
                    })?;
                if let Some(event_type) = steer_event_type(&payload) {
                    session.handle_ack(event_type, &payload);
                    idle_deadline.record_activity();
                    session.flush_held(socket, idle_timeout, &state).await?;
                    continue;
                }
                collect_server_output_item(&payload, &mut server_output_items);
                let (completed, activity) =
                    handle_codex_ws_value(&payload, &mut state, on_event, &mut events_emitted)?;
                if completed {
                    return session.finish(state, server_output_items, events_emitted, candidate);
                }
                if activity {
                    idle_deadline.record_activity();
                    session.flush_held(socket, idle_timeout, &state).await?;
                }
            }
            request = recv_steering(steering), if steering.is_some() => {
                match request {
                    Some(request) => {
                        session.held.push(request);
                        session.flush_held(socket, idle_timeout, &state).await?;
                    }
                    None => *steering = None,
                }
            }
        }
    }
    Err(CodexWsFailure::Transport {
        message: "websocket ended before response.completed".into(),
        events_emitted,
    })
}

async fn recv_steering(
    steering: &mut Option<ProviderSteeringReceiver>,
) -> Option<rho_sdk::provider::ProviderSteeringRequest> {
    match steering.as_mut() {
        Some(receiver) => receiver.recv().await,
        None => None,
    }
}

#[derive(Default)]
struct SteerCollect {
    held: Vec<rho_sdk::provider::ProviderSteeringRequest>,
    in_flight: Option<InFlightSteer>,
    accepted_items: Vec<Value>,
    required_input: bool,
}

struct InFlightSteer {
    request: rho_sdk::provider::ProviderSteeringRequest,
    items: Vec<Value>,
}

impl SteerCollect {
    fn has_representable_output(state: &CodexSseState) -> bool {
        !state.text.is_empty() || !state.tool_calls.is_empty()
    }

    async fn flush_held(
        &mut self,
        socket: &mut CodexSocket,
        idle_timeout: std::time::Duration,
        state: &CodexSseState,
    ) -> Result<(), CodexWsFailure> {
        if self.in_flight.is_some() || !Self::has_representable_output(state) {
            return Ok(());
        }
        let Some(response_id) = state.response_id.as_deref() else {
            return Ok(());
        };
        while !self.held.is_empty() {
            let request = self.held.remove(0);
            if !request.claim() {
                request.release();
                continue;
            }
            let items = steer_items(request.content()).map_err(CodexWsFailure::Model)?;
            let frame =
                steer_frame(response_id, request.content()).map_err(CodexWsFailure::Model)?;
            wait_for_stream_activity_for(
                socket.send(Message::Text(frame.to_string().into())),
                idle_timeout,
            )
            .await
            .map_err(|err| CodexWsFailure::Transport {
                message: err.to_string(),
                events_emitted: true,
            })?
            .map_err(|err| CodexWsFailure::Transport {
                message: format!("websocket steer send failed: {err}"),
                events_emitted: true,
            })?;
            self.in_flight = Some(InFlightSteer { request, items });
            break;
        }
        Ok(())
    }

    fn handle_ack(&mut self, event_type: &str, payload: &Value) {
        match event_type {
            "response.steer.accepted" => {
                if let Some(in_flight) = self.in_flight.take() {
                    self.accepted_items.extend(in_flight.items);
                    in_flight.request.accept();
                }
            }
            "response.steer.failed" => {
                if let Some(in_flight) = self.in_flight.take() {
                    in_flight.request.release();
                } else if !self.held.is_empty() {
                    self.held.remove(0).release();
                }
            }
            "response.steer.pending" => {
                self.required_input = is_steer_pending_required_input(payload);
            }
            _ => {}
        }
    }

    fn finish(
        mut self,
        state: CodexSseState,
        server_output_items: Vec<Value>,
        events_emitted: bool,
        candidate: Option<&CodexContinuationCandidate>,
    ) -> Result<CodexWsCompleted, CodexWsFailure> {
        for request in self.held.drain(..) {
            request.release();
        }
        let steered = state.steered;
        if let Some(in_flight) = self.in_flight.take() {
            // An earlier acknowledged steer already explains the steered
            // terminal, so a later unacknowledged send stays unconfirmed.
            if steered && self.accepted_items.is_empty() {
                self.accepted_items.extend(in_flight.items);
                in_flight.request.accept();
            } else {
                in_flight.request.release();
            }
        }
        let response_id = state.response_id.clone();
        let response = state
            .into_response()
            .map_err(|error| classify_model_error(error, events_emitted))?;
        let pending_steer = pending_from_collect(
            candidate,
            response_id,
            steered,
            self.required_input,
            self.accepted_items,
            &response.response,
        );
        Ok(CodexWsCompleted {
            response,
            server_output_items,
            pending_steer,
        })
    }
}

fn pending_from_collect(
    candidate: Option<&CodexContinuationCandidate>,
    response_id: Option<String>,
    steered: bool,
    required_input: bool,
    steer_items: Vec<Value>,
    response: &crate::model::ModelResponse,
) -> Option<PendingSteer> {
    let candidate = candidate?;
    response_id.as_ref()?;
    if steer_items.is_empty() {
        return None;
    }
    if required_input {
        return Some(PendingSteer {
            request_properties: candidate.request_properties.clone(),
            request_input: candidate.input.clone(),
            steer_items,
            mode: SteerMode::RequiredInput,
        });
    }
    if !steered {
        return None;
    }
    let _ = response;
    Some(PendingSteer {
        request_properties: candidate.request_properties.clone(),
        request_input: candidate.input.clone(),
        steer_items,
        mode: SteerMode::AutoContinuation,
    })
}

fn finish_ws_turn(
    state: &mut CodexWsState,
    candidate: CodexContinuationCandidate,
    body: Value,
    output: Result<CodexWsCompleted, CodexWsFailure>,
    reuse: bool,
) -> Result<CodexWsTurn, ModelError> {
    match output {
        Ok(output) => {
            let CodexWsCompleted {
                response,
                server_output_items,
                pending_steer,
            } = output;
            if pending_steer
                .as_ref()
                .is_some_and(|pending| pending.mode == SteerMode::RequiredInput)
            {
                state.continuation.reset();
                state.connection = None;
            } else if pending_steer
                .as_ref()
                .is_some_and(|pending| pending.mode == SteerMode::AutoContinuation)
            {
                // Keep the socket; the server is already producing the continuation.
            } else {
                let continuation_response = CodexContinuationResponse::from_response(
                    &response.response,
                    response.response_id.clone(),
                    server_output_items,
                );
                state
                    .continuation
                    .record_success(candidate, continuation_response);
            }
            state.pending_steer = if reuse {
                // The matched continuation consumed the old pending steer. Keep
                // a new steer accepted while reading that continuation.
                pending_steer
            } else {
                pending_steer.or(state.pending_steer.take())
            };
            state.turn_open = false;
            Ok(CodexWsTurn::Completed(response))
        }
        Err(failure) => {
            state.discard();
            failure.into_turn(body)
        }
    }
}

async fn collect_codex_ws_response_silent(
    socket: &mut CodexSocket,
    idle_timeout: std::time::Duration,
) -> Result<CodexWsCompleted, CodexWsFailure> {
    let mut state = CodexSseState::default();
    let mut server_output_items = Vec::new();
    let mut idle_deadline = StreamIdleDeadline::with_timeout(idle_timeout);
    loop {
        let Some(message) = idle_deadline.wait_for(socket.next()).await.map_err(|err| {
            CodexWsFailure::Transport {
                message: err.to_string(),
                events_emitted: false,
            }
        })?
        else {
            break;
        };
        let message = message.map_err(|err| CodexWsFailure::Transport {
            message: format!("websocket receive failed: {err}"),
            events_emitted: false,
        })?;
        let text = match &message {
            Message::Text(text) => text.as_str(),
            Message::Binary(bytes) => {
                std::str::from_utf8(bytes).map_err(|err| CodexWsFailure::Transport {
                    message: format!("websocket binary frame contained invalid utf-8: {err}"),
                    events_emitted: false,
                })?
            }
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Close(_) => {
                return Err(CodexWsFailure::Transport {
                    message: "websocket closed before response.completed".into(),
                    events_emitted: false,
                });
            }
            Message::Frame(_) => continue,
        };
        let payload =
            serde_json::from_str::<Value>(text).map_err(|err| CodexWsFailure::Transport {
                message: format!("websocket frame was not valid JSON: {err}"),
                events_emitted: false,
            })?;
        collect_server_output_item(&payload, &mut server_output_items);
        let (completed, activity) = handle_codex_ws_value_silent(&payload, &mut state)?;
        if completed {
            let response = state
                .into_response()
                .map_err(|error| classify_model_error(error, /*events_emitted*/ false))?;
            return Ok(CodexWsCompleted {
                response,
                server_output_items,
                pending_steer: None,
            });
        }
        if activity {
            idle_deadline.record_activity();
        }
    }
    Err(CodexWsFailure::Transport {
        message: "websocket ended before response.completed".into(),
        events_emitted: false,
    })
}

fn classify_model_error(error: ModelError, events_emitted: bool) -> CodexWsFailure {
    // Empty completed responses fail as InvalidResponse before any caller-visible
    // output. Treat that as a retryable transport failure so the existing
    // FullSseFallback path can resubmit the full Responses body over SSE.
    match error {
        ModelError::InvalidResponse(message) if !events_emitted => CodexWsFailure::Transport {
            message,
            events_emitted: false,
        },
        // Terminal protocol failures surfaced by the shared check inside
        // `handle_codex_sse_value` fall back to the SSE transport when output
        // was already emitted, or when the error is specific to the websocket
        // transport (stale continuation, connection limit).
        ModelError::ProviderReported {
            error_type,
            message,
            ..
        } if events_emitted
            || matches!(
                error_type.as_str(),
                "previous_response_not_found" | "websocket_connection_limit_reached"
            ) =>
        {
            CodexWsFailure::Transport {
                message: format!("websocket {error_type}: {message}"),
                events_emitted,
            }
        }
        error => CodexWsFailure::Model(error),
    }
}

fn handle_codex_ws_value(
    value: &Value,
    state: &mut CodexSseState,
    on_event: &mut Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)>,
    events_emitted: &mut bool,
) -> Result<(bool, bool), CodexWsFailure> {
    let mut emit_event = |event| {
        if let Some(on_event) = on_event.as_mut() {
            on_event(event)?;
            *events_emitted = true;
        }
        Ok(())
    };
    handle_codex_sse_value(
        value,
        state,
        &mut Some(&mut emit_event as &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)),
        CodexTransport::WebSocket,
    )
    .map_err(|error| classify_model_error(error, *events_emitted))?;
    let event_type = value.get("type").and_then(Value::as_str);
    Ok((
        is_codex_turn_complete(value),
        event_type.is_some_and(|event_type| event_type.starts_with("response.")),
    ))
}

fn handle_codex_ws_value_silent(
    value: &Value,
    state: &mut CodexSseState,
) -> Result<(bool, bool), CodexWsFailure> {
    let mut on_event: Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)> = None;
    handle_codex_sse_value(value, state, &mut on_event, CodexTransport::WebSocket)
        .map_err(|error| classify_model_error(error, /*events_emitted*/ false))?;
    let event_type = value.get("type").and_then(Value::as_str);
    Ok((
        is_codex_turn_complete(value),
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

/// Keys `response_create_frame` layers onto a Responses body.
///
/// Listed once so framing and unframing cannot drift: removing exactly these
/// keys turns a frame back into the body the caller passed in.
const FRAME_ONLY_KEYS: [&str; 1] = ["type"];

fn response_create_frame(mut body: Value) -> Value {
    body["type"] = json!("response.create");
    body
}

/// Recovers the Responses body from a frame this transport built.
fn response_body_from_frame(mut frame: Value) -> Value {
    if let Some(object) = frame.as_object_mut() {
        for key in FRAME_ONLY_KEYS {
            object.remove(key);
        }
    }
    frame
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
#[path = "codex_ws_test_support.rs"]
mod codex_ws_test_support;
#[cfg(test)]
#[path = "codex_ws_steering_tests.rs"]
mod steering_tests;
#[cfg(test)]
#[path = "codex_ws_tests.rs"]
mod tests;
