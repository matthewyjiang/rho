//! Codemode cards: the script while it streams and runs, then the nested
//! calls and the script output. Nested calls never reach the model as tool
//! calls, so their rows live on the codemode card instead.

use rho_tools::tool_card::{ToolBody, ToolCard, ToolFact, ToolFamily, ToolHeader, ToolStatus};
use serde_json::Value;

use super::format::{push_error_output, split_body_lines, string_arg};
use crate::tools::code_mode::{NestedCallRecord, NestedCallStatus};

/// Latest call rows shown as facts, matching Pi's collapsed codemode card
/// (`CALL_PREVIEW_COUNT = 8`). Earlier rows move into the body.
const CALL_ROWS: usize = 8;

const SCRIPT_RULE: &str = "─── script ───";
const CALLS_RULE: &str = "─── all calls ───";

fn card(status: ToolStatus, primary: Option<String>) -> ToolCard {
    ToolCard::new(
        status,
        ToolFamily::Default,
        ToolHeader::call(crate::tools::code_mode::CODEMODE_TOOL_NAME, primary),
    )
}

fn script_lines(arguments: &Value) -> Vec<String> {
    string_arg(arguments, "script")
        .map(|script| split_body_lines(script.trim_end()))
        .unwrap_or_default()
}

/// Streaming and started card: the script as code, not escaped JSON.
pub(super) fn preview_card(arguments: &Value, status: ToolStatus) -> ToolCard {
    let mut card = card(status, None);
    card.body = ToolBody::Lines(script_lines(arguments));
    card
}

/// Live card: the latest call rows above the script.
pub(super) fn progress_card(arguments: &Value, rows: &str) -> ToolCard {
    let rows: Vec<&str> = rows.lines().filter(|row| !row.trim().is_empty()).collect();
    let mut card = preview_card(arguments, ToolStatus::Running);
    push_row_facts(
        &mut card,
        rows.iter().map(|row| (*row, row.starts_with('✗'))),
    );
    card
}

/// Finished card. `data` is the tool's structured output; replayed history has
/// none, so it falls back to the model text.
pub(super) fn finished_card(
    arguments: &Value,
    content: &str,
    ok: bool,
    data: Option<&Value>,
) -> ToolCard {
    let status = ToolStatus::from_finished(ok);
    let Some(output) = data.and_then(ScriptResult::parse) else {
        let mut card = card(status, None);
        if ok {
            card.body = ToolBody::Lines(split_body_lines(content));
        } else {
            push_error_output(&mut card, content);
        }
        append_script(&mut card, arguments);
        return card;
    };
    let primary = match output.calls.len() {
        0 => None,
        1 => Some("1 call".into()),
        count => Some(format!("{count} calls")),
    };
    let mut card = card(status, primary);
    let rows: Vec<String> = output.calls.iter().map(NestedCallRecord::row).collect();
    push_row_facts(
        &mut card,
        rows.iter()
            .zip(&output.calls)
            .map(|(row, call)| (row.as_str(), call.status == NestedCallStatus::Error)),
    );
    if let Some(error) = &output.error {
        card.push_fact(ToolFact::Error {
            text: error.lines().next().unwrap_or_default().to_string(),
        });
    }
    let mut body = output.prints;
    if !output.return_value.is_null() {
        let text = serde_json::to_string_pretty(&output.return_value).unwrap_or_default();
        body.extend(split_body_lines(&text));
    }
    if rows.len() > CALL_ROWS {
        body.push(CALLS_RULE.into());
        body.extend(rows);
    }
    card.body = ToolBody::Lines(body);
    append_script(&mut card, arguments);
    card
}

/// Latest rows as tree facts; a leading count names the rows left out.
fn push_row_facts<'a>(card: &mut ToolCard, rows: impl ExactSizeIterator<Item = (&'a str, bool)>) {
    let earlier = rows.len().saturating_sub(CALL_ROWS);
    if earlier > 0 {
        card.push_fact(ToolFact::Meta {
            text: format!("… {earlier} earlier calls"),
        });
    }
    for (row, failed) in rows.skip(earlier) {
        card.push_fact(if failed {
            ToolFact::Error { text: row.into() }
        } else {
            ToolFact::Text { text: row.into() }
        });
    }
}

/// The script goes last, so collapsed cards lead with calls and output.
fn append_script(card: &mut ToolCard, arguments: &Value) {
    let script = script_lines(arguments);
    if script.is_empty() {
        return;
    }
    let ToolBody::Lines(body) = &mut card.body else {
        card.body = ToolBody::Lines(script);
        return;
    };
    if !body.is_empty() {
        body.push(SCRIPT_RULE.into());
    }
    body.extend(script);
}

struct ScriptResult {
    return_value: Value,
    prints: Vec<String>,
    calls: Vec<NestedCallRecord>,
    error: Option<String>,
}

impl ScriptResult {
    fn parse(data: &Value) -> Option<Self> {
        Some(Self {
            return_value: data.get("return_value").cloned().unwrap_or(Value::Null),
            prints: serde_json::from_value(data.get("prints")?.clone()).ok()?,
            calls: serde_json::from_value(data.get("calls")?.clone()).ok()?,
            error: data
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }
}

#[cfg(test)]
#[path = "interactive_presenter_codemode_tests.rs"]
mod tests;
