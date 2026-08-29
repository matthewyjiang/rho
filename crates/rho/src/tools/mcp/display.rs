//! Human-readable identity and argument display for MCP tool cards.
//!
//! Exported MCP tool names are wire identifiers (`mcp__server__tool`, with
//! `_rho_` hex escapes for unsafe components). Cards should show the decoded
//! tool name as the verb, the server as a provenance fact, and a best-guess
//! primary argument instead of a raw JSON blob.

use rho_tools::tool_card::{ToolFact, ToolHeader};
use serde_json::Value;

/// Decoded `server` + `tool` from one exported MCP tool name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct McpToolIdentity {
    pub(crate) server: String,
    pub(crate) tool: String,
}

/// Parse `mcp__<server>__<tool>` back into display components.
///
/// Components encoded by [`super::tool::namespaced_tool_name`] (`_rho_` + hex)
/// are decoded to their original text. Names from other producers (for
/// example claude-cli's own `mcp__server__tool` convention) pass through
/// unchanged; the first `__` after the prefix splits server from tool.
pub(crate) fn parse_exported_name(name: &str) -> Option<McpToolIdentity> {
    let rest = name.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    Some(McpToolIdentity {
        server: decode_component(server),
        tool: decode_component(tool),
    })
}

/// Reverse the `_rho_` + hex escape from `namespaced_tool_name`. Anything that
/// does not round-trip cleanly (odd length, non-hex, invalid UTF-8) is shown
/// as-is rather than guessed at.
fn decode_component(component: &str) -> String {
    let Some(hex) = component.strip_prefix("_rho_") else {
        return component.to_string();
    };
    if hex.is_empty() || hex.len() % 2 != 0 {
        return component.to_string();
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for pair in hex.as_bytes().chunks(2) {
        let hi = (pair[0] as char).to_digit(16);
        let lo = (pair[1] as char).to_digit(16);
        match (hi, lo) {
            (Some(hi), Some(lo)) => bytes.push((hi * 16 + lo) as u8),
            _ => return component.to_string(),
        }
    }
    String::from_utf8(bytes).unwrap_or_else(|_| component.to_string())
}

/// Header primary budget; matches the 80-char truncation web/fetch cards use.
const MAX_PRIMARY_CHARS: usize = 80;
/// Per-argument value budget inside the summary fact. Half a typical terminal
/// row so several `key value` pairs fit before the fact wraps.
const MAX_ARG_VALUE_CHARS: usize = 40;
/// Whole summary-fact budget; matches the error-summary truncation in
/// `push_error_output`.
const MAX_SUMMARY_CHARS: usize = 160;

/// Argument keys most likely to identify what a call operated on, in
/// preference order.
const PRIMARY_ARGUMENT_KEYS: &[&str] = &[
    "path",
    "file",
    "file_path",
    "url",
    "query",
    "pattern",
    "command",
    "name",
    "prompt",
    "action",
    "id",
];

/// Pick the argument worth promoting into the header: the first well-known key
/// with a string value, else the only string argument when exactly one exists.
///
/// Returns `(key, display_value)` so the summary fact can skip the promoted
/// key. Multiline values keep their first line with an ellipsis.
pub(crate) fn primary_argument(arguments: &Value) -> Option<(String, String)> {
    let object = arguments.as_object()?;
    for key in PRIMARY_ARGUMENT_KEYS {
        if let Some(Value::String(text)) = object.get(*key) {
            if let Some(display) = primary_display(text) {
                return Some(((*key).to_string(), display));
            }
        }
    }
    let mut strings = object.iter().filter_map(|(key, value)| match value {
        Value::String(text) => Some((key, text)),
        _ => None,
    });
    let (key, text) = strings.next()?;
    if strings.next().is_some() {
        return None;
    }
    let display = primary_display(text)?;
    Some((key.clone(), display))
}

fn primary_display(text: &str) -> Option<String> {
    let first = text.lines().next().unwrap_or("").trim();
    if first.is_empty() {
        return None;
    }
    let multiline = text.lines().nth(1).is_some();
    if !multiline {
        return Some(truncate(first, MAX_PRIMARY_CHARS));
    }
    // Reserve room for the continuation ellipsis so the budget holds.
    let mut display = truncate(first, MAX_PRIMARY_CHARS.saturating_sub(1));
    if !display.ends_with('…') {
        display.push('…');
    }
    Some(display)
}

/// One-line `key value · key value` summary of the remaining scalar arguments.
///
/// `skip` removes the argument already promoted to the header. Multiline
/// strings are omitted rather than mangled; nested objects and arrays collapse
/// to `{…}` / `[…]`. Returns `None` when nothing summarizable remains.
pub(crate) fn argument_summary(arguments: &Value, skip: Option<&str>) -> Option<String> {
    let object = arguments.as_object()?;
    let mut parts = Vec::new();
    for (key, value) in object {
        if Some(key.as_str()) == skip {
            continue;
        }
        let rendered = match value {
            Value::String(text) if text.contains('\n') => continue,
            Value::String(text) => truncate(text, MAX_ARG_VALUE_CHARS),
            Value::Bool(flag) => flag.to_string(),
            Value::Number(number) => number.to_string(),
            Value::Null => "null".into(),
            Value::Object(_) => "{…}".into(),
            Value::Array(_) => "[…]".into(),
        };
        parts.push(format!("{key} {rendered}"));
    }
    if parts.is_empty() {
        return None;
    }
    Some(truncate(&parts.join(" · "), MAX_SUMMARY_CHARS))
}

/// Dim provenance fact text: `mcp · <server>`.
pub(crate) fn server_fact_text(server: &str) -> String {
    format!("mcp · {server}")
}

/// The shared MCP card grammar both producers render: decoded tool verb with
/// a promoted primary argument, then a `mcp · server` provenance fact, then a
/// one-line summary of the remaining arguments.
///
/// `None` when `name` is not an exported MCP name; callers keep their own
/// generic fallback.
pub(crate) fn mcp_header_and_facts(
    name: &str,
    arguments: Option<&Value>,
) -> Option<(ToolHeader, Vec<ToolFact>)> {
    let identity = parse_exported_name(name)?;
    let primary = arguments.and_then(primary_argument);
    let header = ToolHeader::call(
        identity.tool,
        primary.as_ref().map(|(_, value)| value.clone()),
    );
    let mut facts = vec![ToolFact::Meta {
        text: server_fact_text(&identity.server),
    }];
    if let Some(text) = arguments
        .and_then(|value| argument_summary(value, primary.as_ref().map(|(key, _)| key.as_str())))
    {
        facts.push(ToolFact::Text { text });
    }
    Some((header, facts))
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
}

#[cfg(test)]
#[path = "display_tests.rs"]
mod tests;
