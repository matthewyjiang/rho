//! OpenAI server-side compaction via `POST /responses/compact`.
//!
//! Both Codex and direct API-key OpenAI use the unary compact endpoint. The
//! server returns replacement output items (retained user messages plus one
//! encrypted compaction item). Subsequent compatible turns must use the
//! Responses API so the compaction item can be replayed.

use serde_json::Value;

use crate::model::{
    AssistantMessage, ContentBlock, Message, ModelError, ModelIdentity, ModelRequest, ModelUsage,
    ProviderContextBlock,
};

use super::auth::Auth;
use super::codex_request::{build_responses_compact_body, ResponsesProfile};
use super::codex_ws::CodexWsTransport;
use super::reasoning::OpenAiReasoningProfile;
use super::responses_http::{
    ResponsesEndpoint, ResponsesFailedAttempt, ResponsesFailedAttemptKind, ResponsesHttpTransport,
};

pub(super) const COMPACTION_OUTPUT_ITEM_KIND: &str = "openai_response_output_item";

/// Portable notice shown when the encrypted compaction artifact cannot replay
/// (model/provider/API switch). Server-returned user messages remain in history.
const PORTABLE_HANDOFF_NOTICE: &str = "\
Context was compacted with OpenAI server-side compaction. Prior assistant replies \
and tool results live in an encrypted artifact that only compatible OpenAI Responses \
turns can read. Retained recent user messages are kept below.";

fn native_failed_attempts(
    attempts: Vec<ResponsesFailedAttempt>,
) -> Vec<rho_sdk::provider::NativeCompactionFailedAttempt> {
    attempts
        .into_iter()
        .map(|attempt| {
            let kind = match attempt.kind {
                ResponsesFailedAttemptKind::Authentication => {
                    rho_sdk::ProviderErrorKind::Authentication
                }
            };
            rho_sdk::provider::NativeCompactionFailedAttempt::new(kind, ModelUsage::default())
        })
        .collect()
}

fn failure_response(
    error: ModelError,
    failed_attempts: Vec<rho_sdk::provider::NativeCompactionFailedAttempt>,
) -> rho_sdk::provider::NativeCompactionResponse {
    rho_sdk::provider::NativeCompactionResponse::failure(
        crate::providers::sdk_contract::provider_error_from_model_error(error),
    )
    .with_failed_attempts(failed_attempts)
}

/// Runs native compaction through the shared Responses HTTP transport.
pub(super) async fn compact_with_http(
    auth: &Auth,
    profile: &ResponsesProfile,
    reasoning_profile: &OpenAiReasoningProfile,
    http: &ResponsesHttpTransport<'_>,
    codex_ws: &CodexWsTransport,
    request: ModelRequest<'_>,
) -> rho_sdk::provider::NativeCompactionResponse {
    let cancellation = request.cancellation.clone();
    let identity = profile.identity().clone();
    // Only system messages are preserved from the source history; capture those
    // alone so the full conversation is not cloned across the HTTP round-trip.
    let retained_system_messages = request
        .messages
        .iter()
        .filter(|message| matches!(message, Message::System(_)))
        .cloned()
        .collect::<Vec<_>>();
    let body = match build_compact_request_body(profile, reasoning_profile, request) {
        Ok(body) => body,
        Err(error) => return failure_response(error, Vec::new()),
    };

    let http_result = http
        .post_json(auth, ResponsesEndpoint::Compact, &body, Some(&cancellation))
        .await;
    let failed_attempts = native_failed_attempts(http_result.failed_attempts);
    let response = match http_result.response {
        Ok(response) => response,
        Err(error) => return failure_response(error, failed_attempts),
    };
    if !response.status().is_success() {
        return rho_sdk::provider::NativeCompactionResponse::failure(
            crate::providers::sdk_contract::provider_error_from_model_error(
                crate::provider_backend::http_error::from_response(response).await,
            ),
        )
        .with_failed_attempts(failed_attempts);
    }

    let body = tokio::select! {
        result = response.json::<Value>() => match result {
            Ok(body) => body,
            Err(error) => {
                return failure_response(ModelError::from(error), failed_attempts);
            }
        },
        () = cancellation.cancelled() => {
            return failure_response(ModelError::Interrupted, failed_attempts);
        }
    };

    // History shape changed; drop any live previous_response_id baseline.
    if matches!(auth, Auth::Codex { .. }) {
        codex_ws.reset().await;
    }

    let (messages, usage) = match parse_compact_response(identity, &retained_system_messages, &body)
    {
        Ok(parsed) => parsed,
        Err(error) => return failure_response(error, failed_attempts),
    };
    let output = match rho_sdk::CompactionOutput::with_usage(messages, usage) {
        Ok(output) => output,
        Err(error) => {
            return failure_response(
                ModelError::InvalidResponse(error.to_string()),
                failed_attempts,
            );
        }
    };
    rho_sdk::provider::NativeCompactionResponse::success(output)
        .with_failed_attempts(failed_attempts)
}

/// Builds a unary `/responses/compact` request body from the live turn snapshot.
pub(super) fn build_compact_request_body(
    profile: &ResponsesProfile,
    reasoning_profile: &OpenAiReasoningProfile,
    request: ModelRequest<'_>,
) -> Result<Value, ModelError> {
    build_responses_compact_body(profile, reasoning_profile, request)
}

/// Parses a unary `/responses/compact` JSON body into replacement history + usage.
pub(super) fn parse_compact_response(
    identity: ModelIdentity,
    retained_system_messages: &[Message],
    body: &Value,
) -> Result<(Vec<Message>, ModelUsage), ModelError> {
    let output = body
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ModelError::InvalidResponse("compact response missing output array".into())
        })?;
    let usage = crate::protocol::openai_responses::extract_usage(body).unwrap_or_default();
    let messages = replacement_from_compact_output(identity, retained_system_messages, output)?;
    Ok((messages, usage))
}

pub(super) fn replacement_from_compact_output<'a>(
    identity: ModelIdentity,
    retained_system_messages: impl IntoIterator<Item = &'a Message>,
    output_items: &[Value],
) -> Result<Vec<Message>, ModelError> {
    let compaction_item = extract_compaction_item(output_items)?;
    let mut replacement = Vec::new();

    // System prompts stay host-owned; the compact endpoint returns conversation
    // items, not the instructions channel.
    for message in retained_system_messages {
        debug_assert!(
            matches!(message, Message::System(_)),
            "retained_system_messages must only contain system messages"
        );
        if matches!(message, Message::System(_)) {
            replacement.push(message.clone());
        }
    }

    for item in output_items {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
        match item_type {
            "compaction" => {}
            "message" if item.get("role").and_then(Value::as_str) == Some("user") => {
                if let Some(message) = user_message_from_output_item(item) {
                    replacement.push(message);
                }
            }
            // Older compact payloads may omit type and only set role=user.
            _ if item.get("role").and_then(Value::as_str) == Some("user") => {
                if let Some(message) = user_message_from_output_item(item) {
                    replacement.push(message);
                }
            }
            // Drop assistant/tool/reasoning items from compact output; the
            // encrypted compaction item is the server's compressed substitute.
            _ => {}
        }
    }

    replacement.push(Message::assistant(
        AssistantMessage {
            content: Vec::new(),
            provenance: Some(identity.clone()),
            reasoning_summary: None,
            provider_context: vec![ProviderContextBlock {
                identity,
                kind: COMPACTION_OUTPUT_ITEM_KIND.into(),
                position: Some(0),
                data: compaction_item,
            }],
        }
        .with_portable_fallback(PORTABLE_HANDOFF_NOTICE),
    ));

    if replacement
        .iter()
        .all(|message| matches!(message, Message::System(_)))
    {
        return Err(ModelError::InvalidResponse(
            "compact response produced no conversation replacement".into(),
        ));
    }
    Ok(replacement)
}

fn user_message_from_output_item(item: &Value) -> Option<Message> {
    let content = item.get("content")?;
    let mut blocks = Vec::new();
    match content {
        Value::String(text) if !text.is_empty() => {
            blocks.push(ContentBlock::Text(text.clone()));
        }
        Value::Array(parts) => {
            for part in parts {
                let part_type = part.get("type").and_then(Value::as_str).unwrap_or_default();
                match part_type {
                    "input_text" | "output_text" | "text" => {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            if !text.is_empty() {
                                blocks.push(ContentBlock::Text(text.to_string()));
                            }
                        }
                    }
                    "input_image" | "image_url" => {
                        // Image payloads in compact output are rare; keep a textual
                        // placeholder so the turn stays valid without re-fetching.
                        if let Some(url) = part
                            .get("image_url")
                            .and_then(|value| {
                                value
                                    .as_str()
                                    .or_else(|| value.get("url").and_then(Value::as_str))
                            })
                            .filter(|url| !url.is_empty())
                        {
                            blocks.push(ContentBlock::Text(format!("[image: {url}]")));
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    (!blocks.is_empty()).then_some(Message::User(blocks))
}

pub(super) fn extract_compaction_item(output_items: &[Value]) -> Result<Value, ModelError> {
    let compaction_items = output_items
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("compaction"))
        .cloned()
        .collect::<Vec<_>>();
    match compaction_items.as_slice() {
        [item] => {
            let encrypted = item
                .get("encrypted_content")
                .and_then(Value::as_str)
                .filter(|content| !content.is_empty());
            if encrypted.is_none() {
                return Err(ModelError::InvalidResponse(
                    "compact response compaction item missing encrypted_content".into(),
                ));
            }
            Ok(item.clone())
        }
        [] => Err(ModelError::InvalidResponse(
            "compact response returned no compaction item".into(),
        )),
        _ => Err(ModelError::InvalidResponse(format!(
            "compact response expected exactly one compaction item, got {}",
            compaction_items.len()
        ))),
    }
}

#[cfg(test)]
#[path = "remote_compaction_tests.rs"]
mod tests;
