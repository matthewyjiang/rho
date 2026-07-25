//! Shared Responses API server-side compaction response parsing.
//!
//! OpenAI and xAI both expose `POST /responses/compact` with the same output
//! shape: retained conversation items plus one opaque compaction item. Providers
//! own request building, auth, and portable handoff copy; this module turns the
//! compact JSON into replacement history and shared native-compaction responses.

use serde_json::Value;

use crate::model::{
    AssistantMessage, ContentBlock, Message, ModelError, ModelIdentity, ModelUsage,
    ProviderContextBlock,
};

/// Provider-context kind for Responses output items that must replay verbatim.
pub(crate) const COMPACTION_OUTPUT_ITEM_KIND: &str = "openai_response_output_item";

/// Host-owned system prompts retained across compact; the endpoint returns
/// conversation items, not the instructions channel.
pub(crate) fn retained_system_messages(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .filter(|message| matches!(message, Message::System(_)))
        .cloned()
        .collect()
}

/// Parses a unary `/responses/compact` JSON body into replacement history + usage.
pub(crate) fn parse_compact_response(
    identity: ModelIdentity,
    retained_system_messages: &[Message],
    body: &Value,
    portable_handoff_notice: &str,
) -> Result<(Vec<Message>, ModelUsage), ModelError> {
    let output = body
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ModelError::InvalidResponse("compact response missing output array".into())
        })?;
    let usage = super::stream::extract_usage(body).unwrap_or_default();
    let messages = replacement_from_compact_output(
        identity,
        retained_system_messages,
        output,
        portable_handoff_notice,
    )?;
    Ok((messages, usage))
}

/// Builds a native compaction failure, preserving any prior failed attempts.
pub(crate) fn native_compact_failure(
    error: ModelError,
    failed_attempts: Vec<rho_sdk::provider::NativeCompactionFailedAttempt>,
) -> rho_sdk::provider::NativeCompactionResponse {
    rho_sdk::provider::NativeCompactionResponse::failure(
        crate::providers::sdk_contract::provider_error_from_model_error(error),
    )
    .with_failed_attempts(failed_attempts)
}

/// Wraps parsed replacement history as a native compaction success response.
pub(crate) fn native_compact_success(
    messages: Vec<Message>,
    usage: ModelUsage,
    failed_attempts: Vec<rho_sdk::provider::NativeCompactionFailedAttempt>,
) -> rho_sdk::provider::NativeCompactionResponse {
    match rho_sdk::CompactionOutput::with_usage(messages, usage) {
        Ok(output) => rho_sdk::provider::NativeCompactionResponse::success(output)
            .with_failed_attempts(failed_attempts),
        Err(error) => native_compact_failure(
            ModelError::InvalidResponse(error.to_string()),
            failed_attempts,
        ),
    }
}

/// Parses a compact response body and finalizes the native compaction result.
pub(crate) fn native_compact_from_response_body(
    identity: ModelIdentity,
    retained_system_messages: &[Message],
    body: &Value,
    portable_handoff_notice: &str,
    failed_attempts: Vec<rho_sdk::provider::NativeCompactionFailedAttempt>,
) -> rho_sdk::provider::NativeCompactionResponse {
    match parse_compact_response(
        identity,
        retained_system_messages,
        body,
        portable_handoff_notice,
    ) {
        Ok((messages, usage)) => native_compact_success(messages, usage, failed_attempts),
        Err(error) => native_compact_failure(error, failed_attempts),
    }
}

pub(crate) fn replacement_from_compact_output<'a>(
    identity: ModelIdentity,
    retained_system_messages: impl IntoIterator<Item = &'a Message>,
    output_items: &[Value],
    portable_handoff_notice: &str,
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
        let is_user = item.get("role").and_then(Value::as_str) == Some("user");
        match item_type {
            "compaction" => {}
            // Keep user turns from typed messages or older role-only payloads.
            // Drop assistant/tool/reasoning items; the encrypted compaction item
            // is the server's compressed substitute for those.
            _ if is_user => {
                if let Some(message) = user_message_from_output_item(item) {
                    replacement.push(message);
                }
            }
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
        .with_portable_fallback(portable_handoff_notice),
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

pub(crate) fn extract_compaction_item(output_items: &[Value]) -> Result<Value, ModelError> {
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
#[path = "compact_tests.rs"]
mod tests;
