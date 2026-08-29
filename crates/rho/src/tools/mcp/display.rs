//! Human-readable identity and argument display for MCP tool cards.
//!
//! Exported MCP tool names are wire identifiers (`mcp__server__tool`). Cards
//! show the parsed tool name as the verb, the server as a provenance fact, and
//! a best-guess primary argument instead of a raw JSON blob. The caller selects
//! whether Rho's private component encoding applies.

use rho_tools::tool_card::{ToolFact, ToolHeader};
use serde_json::Value;

use super::exported_name::{parse_exported_name, ExportedNameDialect};

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
    // Take the first LF-separated line, then flatten leftover controls so a
    // `\r` or tab cannot split the header the same way a second line would.
    let first = one_line(text.lines().next().unwrap_or(""));
    let first = first.trim();
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
            Value::String(text) => truncate(&one_line(text), MAX_ARG_VALUE_CHARS),
            Value::Bool(flag) => flag.to_string(),
            Value::Number(number) => number.to_string(),
            Value::Null => "null".into(),
            Value::Object(_) => "{…}".into(),
            Value::Array(_) => "[…]".into(),
        };
        parts.push(format!("{} {rendered}", one_line(key)));
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

/// The shared MCP card grammar both producers render: parsed tool verb with
/// a promoted primary argument, then a `mcp · server` provenance fact, then a
/// one-line summary of the remaining arguments.
///
/// `None` when `name` is not an exported MCP name; callers keep their own
/// generic fallback.
pub(crate) fn mcp_header_and_facts(
    name: &str,
    arguments: Option<&Value>,
    dialect: ExportedNameDialect,
) -> Option<(ToolHeader, Vec<ToolFact>)> {
    let identity = parse_exported_name(name, dialect)?;
    let primary = arguments.and_then(primary_argument);
    let header = ToolHeader::call(
        one_line(&identity.tool),
        primary.as_ref().map(|(_, value)| value.clone()),
    );
    let mut facts = vec![ToolFact::Meta {
        text: server_fact_text(&one_line(&identity.server)),
    }];
    if let Some(text) = arguments
        .and_then(|value| argument_summary(value, primary.as_ref().map(|(key, _)| key.as_str())))
    {
        facts.push(ToolFact::Text { text });
    }
    Some((header, facts))
}

/// Flatten control characters so untrusted MCP keys and values cannot split a
/// header or fact into extra terminal rows. Truncation runs after this so
/// budgets still hold.
fn one_line(text: &str) -> String {
    if !text.contains(char::is_control) {
        return text.to_string();
    }
    text.chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect()
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
