//! OpenAI Responses compaction v2 (server-side).
//!
//! Mirrors OpenAI's remote compaction path for both Codex and direct API-key
//! OpenAI: POST `/responses` with a trailing `compaction_trigger`, retain recent
//! user messages, and store the opaque `compaction` item for later compatible
//! turns.

use serde_json::{json, Value};

use crate::model::{
    context::estimate_message_tokens, AssistantMessage, Message, ModelError, ModelIdentity,
    ModelRequest, ProviderContextBlock,
};

use super::codex_request::{build_responses_body_with_profile, CodexRequestMode};
use super::reasoning::OpenAiReasoningProfile;

/// Matches Codex's retained-message default for remote compaction v2.
pub(super) const RETAINED_MESSAGE_TOKEN_BUDGET: u64 = 64_000;

pub(super) const COMPACTION_OUTPUT_ITEM_KIND: &str = "openai_response_output_item";

const PORTABLE_SUMMARY_PLACEHOLDER: &str = "\
Context compacted with OpenAI server-side compaction. Compatible OpenAI turns reuse \
the encrypted compaction artifact. Other models see this notice plus retained recent \
user messages.";

pub(super) fn supports_remote_compaction(identity: &ModelIdentity) -> bool {
    match (identity.provider.as_str(), identity.api.as_str()) {
        ("openai-codex", "openai-responses") => true,
        ("openai", "openai-chat-completions") => true,
        _ => false,
    }
}

pub(super) fn responses_mode_for_identity(identity: &ModelIdentity) -> CodexRequestMode {
    if identity.provider == "openai-codex" {
        CodexRequestMode::for_model(&identity.model)
    } else {
        // Direct OpenAI API-key Responses stays on the standard shape.
        CodexRequestMode::Standard
    }
}

pub(super) fn history_has_remote_compaction(
    messages: &[Message],
    identity: &ModelIdentity,
) -> bool {
    messages.iter().any(|message| {
        let blocks = match message {
            Message::EnrichedAssistant(message) => message.provider_context.as_slice(),
            Message::AbortedAssistant(message) => message.provider_context.as_slice(),
            Message::System(_)
            | Message::User(_)
            | Message::Assistant(_)
            | Message::ToolResult(_) => return false,
        };
        blocks
            .iter()
            .any(|block| is_replayable_compaction_item(block, identity))
    })
}

fn is_replayable_compaction_item(block: &ProviderContextBlock, identity: &ModelIdentity) -> bool {
    block.is_replayable_to(identity)
        && block.kind == COMPACTION_OUTPUT_ITEM_KIND
        && block.data.get("type").and_then(Value::as_str) == Some("compaction")
}

pub(super) fn build_remote_compaction_body(
    identity: &ModelIdentity,
    reasoning_profile: &OpenAiReasoningProfile,
    request: ModelRequest<'_>,
) -> Result<Value, ModelError> {
    let provider = static_provider_name(identity)?;
    let mode = responses_mode_for_identity(identity);
    let mut body = build_responses_body_with_profile(
        provider,
        &identity.model,
        identity,
        reasoning_profile,
        request,
        mode,
    )?;
    let input = body
        .get_mut("input")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| ModelError::InvalidResponse("Responses body missing input".into()))?;
    input.push(json!({ "type": "compaction_trigger" }));
    body["store"] = json!(false);
    body["stream"] = json!(true);
    body["include"] = json!(["reasoning.encrypted_content"]);
    if body.get("parallel_tool_calls").is_none() && mode == CodexRequestMode::Standard {
        body["parallel_tool_calls"] = json!(true);
    }
    Ok(body)
}

fn static_provider_name(identity: &ModelIdentity) -> Result<&'static str, ModelError> {
    match identity.provider.as_str() {
        "openai-codex" => Ok("openai-codex"),
        "openai" => Ok("openai"),
        other => Err(ModelError::InvalidResponse(format!(
            "remote compaction is not supported for provider '{other}'"
        ))),
    }
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
                    "remote compaction item missing encrypted_content".into(),
                ));
            }
            Ok(item.clone())
        }
        [] => Err(ModelError::InvalidResponse(
            "remote compaction v2 returned no compaction item".into(),
        )),
        _ => Err(ModelError::InvalidResponse(format!(
            "remote compaction v2 expected exactly one compaction item, got {}",
            compaction_items.len()
        ))),
    }
}

pub(super) fn build_remote_compaction_replacement(
    identity: ModelIdentity,
    messages: &[Message],
    compaction_item: Value,
    portable_summary: Option<String>,
) -> Result<Vec<Message>, ModelError> {
    let mut leading = Vec::new();
    let mut retained_candidates = Vec::new();
    for message in messages {
        match message {
            Message::System(_) => leading.push(message.clone()),
            Message::User(_) => retained_candidates.push(message.clone()),
            Message::Assistant(_)
            | Message::EnrichedAssistant(_)
            | Message::AbortedAssistant(_)
            | Message::ToolResult(_) => {}
        }
    }

    let retained_users =
        retain_recent_user_messages(retained_candidates, RETAINED_MESSAGE_TOKEN_BUDGET);
    let summary = portable_summary
        .map(|summary| summary.trim().to_string())
        .filter(|summary| !summary.is_empty())
        .unwrap_or_else(|| PORTABLE_SUMMARY_PLACEHOLDER.to_string());

    let mut replacement = leading;
    replacement.extend(retained_users);
    replacement.push(Message::assistant(AssistantMessage {
        content: vec![crate::model::ContentBlock::Text(summary)],
        provenance: Some(identity.clone()),
        reasoning_summary: None,
        provider_context: vec![ProviderContextBlock {
            identity,
            kind: COMPACTION_OUTPUT_ITEM_KIND.into(),
            position: Some(0),
            data: compaction_item,
        }],
    }));
    Ok(replacement)
}

fn retain_recent_user_messages(messages: Vec<Message>, max_tokens: u64) -> Vec<Message> {
    let mut remaining = max_tokens;
    let mut retained_reversed = Vec::new();
    for message in messages.into_iter().rev() {
        if remaining == 0 {
            break;
        }
        if !matches!(message, Message::User(_)) {
            continue;
        }
        let tokens = estimate_message_tokens(&message).max(1);
        if tokens <= remaining {
            remaining = remaining.saturating_sub(tokens);
            retained_reversed.push(message);
            continue;
        }
        if let Some(truncated) = truncate_user_message_to_token_budget(message, remaining) {
            retained_reversed.push(truncated);
        }
        break;
    }
    retained_reversed.reverse();
    retained_reversed
}

fn truncate_user_message_to_token_budget(message: Message, max_tokens: u64) -> Option<Message> {
    let Message::User(blocks) = message else {
        return Some(message);
    };
    let mut remaining_chars = max_tokens.saturating_mul(4);
    let mut truncated = Vec::new();
    for block in blocks {
        match block {
            crate::model::ContentBlock::Image(image) => {
                truncated.push(crate::model::ContentBlock::Image(image));
            }
            crate::model::ContentBlock::Text(text) => {
                if remaining_chars == 0 {
                    continue;
                }
                let clipped = text
                    .chars()
                    .take(remaining_chars as usize)
                    .collect::<String>();
                remaining_chars = remaining_chars.saturating_sub(clipped.chars().count() as u64);
                if !clipped.is_empty() {
                    truncated.push(crate::model::ContentBlock::Text(clipped));
                }
            }
            crate::model::ContentBlock::ToolCall(_) => {}
        }
    }
    (!truncated.is_empty()).then_some(Message::User(truncated))
}

#[cfg(test)]
#[path = "remote_compaction_tests.rs"]
mod tests;
