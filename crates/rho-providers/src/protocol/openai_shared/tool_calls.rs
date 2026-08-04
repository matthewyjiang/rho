//! Chat-completions tool-call normalization.
//!
//! Hosts vary in how they emit tool calls: ids may arrive late or never,
//! indexes can be sparse, and zero-argument calls sometimes omit `{}`.
//! Stream and response paths accumulate raw calls as they arrive; this module
//! applies the one validation policy before blocks enter a model response.

use std::collections::BTreeSet;

use serde_json::json;

use crate::model::ModelError;
use rho_sdk::model::ToolCall;

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

/// Applies the shared tool-call policy to completed and streamed responses.
///
/// Empty slots left by sparse indexes are skipped. Calls that carry data but
/// no name fail the turn. Empty ids are synthesized and duplicate ids get a
/// suffix, so tool results can always reference a unique call.
pub(crate) fn finalize_chat_tool_calls(
    calls: Vec<RawChatToolCall>,
) -> Result<Vec<ToolCall>, ModelError> {
    let mut seen_ids = BTreeSet::new();
    let mut finalized = Vec::new();
    for (index, call) in calls.into_iter().enumerate() {
        if call.is_empty() {
            // Sparse indexes leave blank slots; skip them instead of failing the turn.
            continue;
        }
        let name = call.name.filter(|name| !name.is_empty()).ok_or_else(|| {
            ModelError::InvalidResponse(format!("tool call {index} missing name"))
        })?;
        let id = unique_call_id(
            &mut seen_ids,
            call.id
                .filter(|id| !id.is_empty())
                .unwrap_or_else(|| format!("call_{index}")),
        );
        let arguments = parse_chat_tool_arguments(&name, &call.arguments)?;
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

fn parse_chat_tool_arguments(name: &str, raw: &str) -> Result<serde_json::Value, ModelError> {
    let raw = raw.trim();
    if raw.is_empty() {
        // Hosts sometimes stream name/id first and omit `{}` for zero-arg tools.
        return Ok(json!({}));
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
