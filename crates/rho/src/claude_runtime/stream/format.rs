//! Claude-shaped usage aggregation and content stringify for stream-json.

use std::collections::BTreeMap;

use serde_json::Value;

use rho_sdk::model::{ContextUsage, ModelUsage};

use crate::cli_runtime::stream_effect::MAX_TOOL_PAYLOAD_CHARS;
use crate::cli_runtime::stream_format::bound_text;

pub(crate) use crate::cli_runtime::stream_format::{
    count_fact, display_path_field, quoted, set_lines_body, string_field, truncate,
    truncate_payload_lines, u64_field, MAX_TOOL_BODY_LINES,
};

#[derive(Clone, Debug, Default, serde::Deserialize, PartialEq)]
pub(super) struct RawUsage {
    #[serde(default)]
    pub(super) input_tokens: Option<u64>,
    #[serde(default)]
    pub(super) output_tokens: Option<u64>,
    #[serde(default)]
    pub(super) cache_read_input_tokens: Option<u64>,
    #[serde(default)]
    pub(super) cache_creation_input_tokens: Option<u64>,
}

pub(super) fn raw_usage_to_model(raw: &RawUsage) -> ModelUsage {
    ModelUsage {
        input_tokens: raw.input_tokens,
        output_tokens: raw.output_tokens,
        cache_read_tokens: raw.cache_read_input_tokens,
        cache_write_tokens: raw.cache_creation_input_tokens,
        total_tokens: None,
        context_window: None,
        cost_usd_micros: None,
    }
}

/// Build context usage for a terminal result.
///
/// Token count always prefers top-level `usage` total input (uncached +
/// cache-read + cache-creation) so it matches [`crate::subagent::RunStatus::input_tokens`].
///
/// When `modelUsage` has multiple model keys, context windows are taken as the
/// max reported window across entries (sorted by key for determinism). Token
/// fields on `modelUsage` entries are only used when top-level `usage` is
/// absent, and are aggregated the same uncached+cache way across all entries.
pub(super) fn context_usage_from_result(
    model_usage: Option<&Value>,
    usage: Option<&ModelUsage>,
) -> Option<ContextUsage> {
    let entries = model_usage_entries(model_usage);
    let context_window = entries
        .iter()
        .filter_map(|(_, entry)| {
            entry
                .get("contextWindow")
                .or_else(|| entry.get("context_window"))
                .and_then(Value::as_u64)
        })
        .max();

    let tokens = usage
        .and_then(ModelUsage::total_input_tokens)
        .or_else(|| aggregate_model_usage_input_tokens(&entries));

    if tokens.is_none() && context_window.is_none() {
        return None;
    }
    Some(ContextUsage {
        tokens,
        context_window: context_window.or_else(|| usage.and_then(|value| value.context_window)),
        source: rho_sdk::model::ContextUsageSource::ProviderReported,
    })
}

fn model_usage_entries(model_usage: Option<&Value>) -> Vec<(String, Value)> {
    let Some(Value::Object(map)) = model_usage else {
        return Vec::new();
    };
    // BTreeMap gives deterministic key order when aggregating multiple models.
    let ordered = map.iter().collect::<BTreeMap<_, _>>();
    ordered
        .into_iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn aggregate_model_usage_input_tokens(entries: &[(String, Value)]) -> Option<u64> {
    let mut total = 0_u64;
    let mut saw_any = false;
    for (_, entry) in entries {
        if let Some(tokens) = entry_total_input_tokens(entry) {
            saw_any = true;
            total = total.saturating_add(tokens);
        }
    }
    saw_any.then_some(total)
}

fn entry_total_input_tokens(entry: &Value) -> Option<u64> {
    let uncached = entry
        .get("inputTokens")
        .or_else(|| entry.get("input_tokens"))
        .and_then(Value::as_u64);
    let cache_read = entry
        .get("cacheReadInputTokens")
        .or_else(|| entry.get("cache_read_input_tokens"))
        .and_then(Value::as_u64);
    let cache_creation = entry
        .get("cacheCreationInputTokens")
        .or_else(|| entry.get("cache_creation_input_tokens"))
        .and_then(Value::as_u64);
    let has_input = uncached.is_some() || cache_read.is_some() || cache_creation.is_some();
    has_input.then_some(
        uncached
            .unwrap_or_default()
            .saturating_add(cache_read.unwrap_or_default())
            .saturating_add(cache_creation.unwrap_or_default()),
    )
}

pub(super) fn format_permission_denial(value: Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text),
        Value::Object(map) => {
            let tool = map
                .get("tool_name")
                .or_else(|| map.get("toolName"))
                .and_then(Value::as_str)
                .unwrap_or("tool");
            let reason = map
                .get("reason")
                .or_else(|| map.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("denied");
            Some(format!("{tool}: {reason}"))
        }
        _ => None,
    }
}

pub(super) fn stringify_content(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return String::new();
    };
    let raw = match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                item.get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| {
                        if item.is_string() {
                            item.as_str().map(str::to_string)
                        } else {
                            Some(compact_value_preview(item))
                        }
                    })
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => compact_value_preview(other),
    };
    bound_text(&raw, MAX_TOOL_PAYLOAD_CHARS, "tool payload")
}

fn compact_value_preview(value: &Value) -> String {
    let rendered = value.to_string();
    bound_text(&rendered, MAX_TOOL_PAYLOAD_CHARS, "json")
}
