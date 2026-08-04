//! Chat-completions tool-call normalization.
//!
//! Hosts vary in how they emit tool calls: ids may arrive late or never,
//! indexes can be sparse, and zero-argument calls sometimes omit `{}`.
//! Stream and response paths accumulate raw calls as they arrive; this module
//! applies a chosen validation policy before blocks enter a model response.
//!
//! Default is [`ChatToolCallPolicy::Strict`]. Lenient host quirks (Qwen-style)
//! must opt in explicitly so they do not silently rewrite every OpenAI-chat
//! dialect's contract.

use std::collections::BTreeSet;

use serde_json::json;

use crate::model::ModelError;
use rho_sdk::model::ToolCall;

/// How aggressively to normalize incomplete chat tool-call payloads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ChatToolCallPolicy {
    /// Fail loud on missing ids, empty arguments, and duplicate ids.
    #[default]
    Strict,
    /// Tolerate common OpenAI-compatible quirks:
    /// synthesize missing ids, coerce empty args to `{}`, skip empty sparse
    /// slots, and suffix duplicate ids so tool results stay referenceable.
    Lenient,
}

/// Tool call exactly as a chat-completions host sent it, before validation.
#[derive(Default)]
pub(crate) struct RawChatToolCall {
    pub(crate) id: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) arguments: String,
}

impl RawChatToolCall {
    pub(crate) fn is_empty(&self) -> bool {
        self.id.as_ref().is_none_or(|id| id.is_empty())
            && self.name.as_ref().is_none_or(|name| name.is_empty())
            && self.arguments.trim().is_empty()
    }
}

/// Applies the tool-call policy to completed and streamed responses.
pub(crate) fn finalize_chat_tool_calls(
    calls: Vec<RawChatToolCall>,
    policy: ChatToolCallPolicy,
) -> Result<Vec<ToolCall>, ModelError> {
    let mut seen_ids = BTreeSet::new();
    let mut finalized = Vec::new();
    for (index, call) in calls.into_iter().enumerate() {
        if call.is_empty() {
            // Sparse indexes leave blank slots. Lenient hosts skip them;
            // strict hosts still reject so a hole cannot hide a missing call.
            match policy {
                ChatToolCallPolicy::Lenient => continue,
                ChatToolCallPolicy::Strict => {
                    return Err(ModelError::InvalidResponse(format!(
                        "tool call {index} missing id"
                    )));
                }
            }
        }
        let name = call.name.filter(|name| !name.is_empty()).ok_or_else(|| {
            ModelError::InvalidResponse(format!("tool call {index} missing name"))
        })?;
        let raw_id = call.id.filter(|id| !id.is_empty());
        let id = match (policy, raw_id) {
            (_, Some(id)) => id,
            (ChatToolCallPolicy::Lenient, None) => format!("call_{index}"),
            (ChatToolCallPolicy::Strict, None) => {
                return Err(ModelError::InvalidResponse(format!(
                    "tool call {index} missing id"
                )));
            }
        };
        let id = match policy {
            ChatToolCallPolicy::Strict => {
                if !seen_ids.insert(id.clone()) {
                    return Err(ModelError::InvalidResponse(format!(
                        "duplicate tool call id '{id}'"
                    )));
                }
                id
            }
            ChatToolCallPolicy::Lenient => unique_call_id(&mut seen_ids, id),
        };
        let arguments = parse_chat_tool_arguments(&name, &call.arguments, policy)?;
        finalized.push(ToolCall {
            id,
            name,
            arguments,
        });
    }
    Ok(finalized)
}

/// Keeps every tool-call id unique by suffixing repeats (`id`, `id_2`, ...).
fn unique_call_id(seen_ids: &mut BTreeSet<String>, mut id: String) -> String {
    if seen_ids.insert(id.clone()) {
        return id;
    }
    let mut suffix = 2;
    loop {
        let candidate = format!("{id}_{suffix}");
        if seen_ids.insert(candidate.clone()) {
            id = candidate;
            return id;
        }
        suffix += 1;
    }
}

fn parse_chat_tool_arguments(
    name: &str,
    raw: &str,
    policy: ChatToolCallPolicy,
) -> Result<serde_json::Value, ModelError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return match policy {
            // Hosts sometimes stream name/id first and omit `{}` for zero-arg tools.
            ChatToolCallPolicy::Lenient => Ok(json!({})),
            ChatToolCallPolicy::Strict => Err(ModelError::InvalidResponse(format!(
                "invalid tool call arguments for {name}: empty arguments"
            ))),
        };
    }
    let value: serde_json::Value = serde_json::from_str(raw).map_err(|error| {
        ModelError::InvalidResponse(format!("invalid tool call arguments for {name}: {error}"))
    })?;
    if !value.is_object() {
        return Err(ModelError::InvalidResponse(format!(
            "tool call arguments for {name} are not a JSON object"
        )));
    }
    Ok(value)
}

#[cfg(test)]
#[path = "tool_calls_tests.rs"]
mod tests;
