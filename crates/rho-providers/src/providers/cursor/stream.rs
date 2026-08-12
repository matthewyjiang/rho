use bytes::Bytes;
use futures_util::StreamExt;
use prost::Message;
use reqwest::StatusCode;
use tokio::sync::mpsc;

use crate::{
    model::{
        registry::missing_credentials_error, ContentBlock, ModelError, ModelEvent, ModelRequest,
        ModelResponse, ToolCall,
    },
    protocol::cursor::{
        agent_server_message, build_cursor_turn, decode_mcp_args, exec_server_message,
        heartbeat_frame, interaction_update, kv_get_blob_response, kv_server_message,
        kv_set_blob_response, native_exec_reject, request_context_success, AgentServerMessage,
        ConnectFrameParser, CursorSpeed, CursorTurn, CONNECT_END_STREAM_FLAG,
    },
    provider_backend::{http_error, stream_timeout::StreamIdleDeadline},
};

use super::CursorProvider;

const HEARTBEAT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

pub(crate) enum CursorHandle {
    Reply(Vec<u8>),
    TextDelta(String),
    ReasoningDelta(String),
    McpTool(ToolCall),
    TurnEnded,
    Ignore,
}

pub(crate) fn handle_server_message(
    message: &AgentServerMessage,
    turn: &mut CursorTurn,
) -> Result<CursorHandle, ModelError> {
    match message.message.as_ref() {
        Some(agent_server_message::Message::InteractionUpdate(update)) => {
            match update.message.as_ref() {
                Some(interaction_update::Message::TextDelta(delta)) => {
                    Ok(CursorHandle::TextDelta(delta.text.clone()))
                }
                Some(interaction_update::Message::ThinkingDelta(delta)) => {
                    Ok(CursorHandle::ReasoningDelta(delta.text.clone()))
                }
                Some(interaction_update::Message::TurnEnded(_)) => Ok(CursorHandle::TurnEnded),
                Some(interaction_update::Message::TokenDelta(_)) | None => Ok(CursorHandle::Ignore),
            }
        }
        Some(agent_server_message::Message::ExecServerMessage(exec)) => match exec.message.as_ref()
        {
            Some(exec_server_message::Message::RequestContextArgs(_)) => Ok(CursorHandle::Reply(
                request_context_success(exec, turn.mcp_tools.clone(), turn.cloud_rule.clone()),
            )),
            Some(exec_server_message::Message::McpArgs(args)) => {
                Ok(CursorHandle::McpTool(decode_mcp_args(args)?))
            }
            Some(_) => native_exec_reject(exec)
                .map(CursorHandle::Reply)
                .ok_or_else(|| {
                    ModelError::InvalidResponse("unsupported Cursor exec message".into())
                }),
            None => Ok(CursorHandle::Ignore),
        },
        Some(agent_server_message::Message::KvServerMessage(kv)) => match kv.message.as_ref() {
            Some(kv_server_message::Message::GetBlobArgs(args)) => {
                let data = turn.blob_store.get(&args.blob_id).map(ToOwned::to_owned);
                Ok(CursorHandle::Reply(kv_get_blob_response(kv.id, data)))
            }
            Some(kv_server_message::Message::SetBlobArgs(args)) => {
                turn.blob_store
                    .insert(&args.blob_id, args.blob_data.clone());
                Ok(CursorHandle::Reply(kv_set_blob_response(kv.id)))
            }
            None => Ok(CursorHandle::Ignore),
        },
        Some(agent_server_message::Message::ConversationCheckpointUpdate(_)) | None => {
            Ok(CursorHandle::Ignore)
        }
    }
}

pub(super) async fn run_turn(
    provider: &CursorProvider,
    request: ModelRequest<'_>,
    options: rho_sdk::provider::ModelRequestOptions,
    on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
    on_request_event: &mut (dyn FnMut(rho_sdk::provider::ProviderRequestEvent) -> Result<(), ModelError>
              + Send),
) -> Result<ModelResponse, ModelError> {
    let speed = if options.service_tier() == Some(rho_sdk::model::ServiceTier::Priority)
        && crate::protocol::cursor::supports_fast_mode(&provider.model)
    {
        CursorSpeed::Fast
    } else {
        CursorSpeed::Standard
    };
    let turn = build_cursor_turn(&provider.model_identity(), &provider.model, request, speed)?;
    let token = provider.auth.access_token().await?;
    let (tx, response) = send_run(provider, &turn.request_bytes, &token).await?;
    let response = if response.status() == StatusCode::UNAUTHORIZED {
        if let Some(refreshed) = provider.auth.force_refresh(&token).await? {
            on_request_event(
                rho_sdk::provider::ProviderRequestEvent::RequestAttemptFailed {
                    kind: rho_sdk::ProviderErrorKind::Authentication,
                    usage: Default::default(),
                },
            )?;
            send_run(provider, &turn.request_bytes, &refreshed).await?.1
        } else {
            response
        }
    } else {
        response
    };
    if response.status() == StatusCode::UNAUTHORIZED {
        return Err(missing_credentials_error("cursor"));
    }
    let response = http_error::error_for_status(response).await?;
    read_run_stream(response, turn, tx, on_event).await
}

async fn send_run(
    provider: &CursorProvider,
    initial: &[u8],
    token: &str,
) -> Result<(mpsc::Sender<Bytes>, reqwest::Response), ModelError> {
    let (tx, rx) = mpsc::channel::<Bytes>(16);
    tx.send(Bytes::copy_from_slice(initial))
        .await
        .map_err(|_| ModelError::InvalidResponse("failed to start Cursor run stream".into()))?;
    let body_stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv()
            .await
            .map(|chunk| (Ok::<_, std::io::Error>(chunk), rx))
    });
    let response = CursorProvider::apply_headers(
        provider.client.post(provider.run_url()),
        token,
        /* streaming */ true,
    )
    .body(reqwest::Body::wrap_stream(body_stream))
    .send()
    .await?;
    Ok((tx, response))
}

async fn read_run_stream(
    response: reqwest::Response,
    mut turn: CursorTurn,
    tx: mpsc::Sender<Bytes>,
    on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
) -> Result<ModelResponse, ModelError> {
    let mut parser = ConnectFrameParser::default();
    let mut stream = response.bytes_stream();
    let mut idle = StreamIdleDeadline::new();
    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    loop {
        tokio::select! {
            chunk = idle.wait_for(stream.next()) => {
                let Some(chunk) = chunk? else {
                    break;
                };
                let frames = parser
                    .push(&chunk?)
                    .map_err(|error| ModelError::InvalidResponse(error.into()))?;
                for frame in frames {
                    if frame.is_end_stream() || frame.flags & CONNECT_END_STREAM_FLAG != 0 {
                        if let Some(error) = connect_error_message(&frame.payload) {
                            return Err(ModelError::InvalidResponse(error));
                        }
                        continue;
                    }
                    let message = AgentServerMessage::decode(frame.payload.as_slice())
                        .map_err(|error| ModelError::InvalidResponse(error.to_string()))?;
                    match handle_server_message(&message, &mut turn)? {
                        CursorHandle::Reply(bytes) => {
                            let _ = tx.send(Bytes::from(bytes)).await;
                            idle.record_activity();
                        }
                        CursorHandle::TextDelta(delta) => {
                            text.push_str(&delta);
                            on_event(ModelEvent::OutputDelta(delta))?;
                            idle.record_activity();
                        }
                        CursorHandle::ReasoningDelta(delta) => {
                            on_event(ModelEvent::ReasoningDelta(delta))?;
                            idle.record_activity();
                        }
                        CursorHandle::McpTool(call) => {
                            on_event(ModelEvent::ToolCallDelta {
                                index: tool_calls.len(),
                                id: Some(call.id.clone()),
                                name: Some(call.name.clone()),
                                arguments: call.arguments.to_string(),
                            })?;
                            tool_calls.push(call);
                            return finish_response(text, tool_calls);
                        }
                        CursorHandle::TurnEnded => return finish_response(text, tool_calls),
                        CursorHandle::Ignore => {}
                    }
                }
            }
            _ = heartbeat.tick() => {
                let _ = tx.send(Bytes::from(heartbeat_frame())).await;
            }
        }
    }
    finish_response(text, tool_calls)
}

fn finish_response(text: String, tool_calls: Vec<ToolCall>) -> Result<ModelResponse, ModelError> {
    let mut blocks = Vec::new();
    if !text.is_empty() {
        blocks.push(ContentBlock::Text(text));
    }
    blocks.extend(tool_calls.into_iter().map(ContentBlock::ToolCall));
    if blocks.is_empty() {
        return Err(ModelError::InvalidResponse(
            "Cursor returned an empty assistant response".into(),
        ));
    }
    Ok(ModelResponse::Assistant(blocks))
}

fn connect_error_message(payload: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(payload).ok()?;
    value
        .get("error")?
        .get("message")?
        .as_str()
        .filter(|message| !message.is_empty())
        .map(ToString::to_string)
}
