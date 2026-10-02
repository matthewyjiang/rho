//! Codemode cards show the script as highlighted source: what the model ran,
//! not what it printed. While the script runs, the latest nested-call rows sit
//! above it as live progress; nested calls never reach the model as tool calls,
//! so those rows exist only on this card.

use rho_tools::tool_card::{ToolBody, ToolCard, ToolFact, ToolFamily, ToolHeader, ToolStatus};
use serde_json::Value;

use super::format::string_arg;

/// Latest call rows shown while running, matching Pi's collapsed codemode card
/// (`CALL_PREVIEW_COUNT = 8`).
const CALL_ROWS: usize = 8;

/// Starlark is Python-shaped, and the bundled syntax set has no Starlark grammar.
const SCRIPT_LANGUAGE: &str = "python";

fn card(status: ToolStatus, primary: Option<String>, arguments: &Value) -> ToolCard {
    let lines = string_arg(arguments, "script")
        .map(|script| script.trim_end().lines().map(str::to_string).collect())
        .unwrap_or_default();
    ToolCard::new(
        status,
        ToolFamily::Default,
        ToolHeader::call(crate::tools::code_mode::CODEMODE_TOOL_NAME, primary),
    )
    .with_body(ToolBody::Code {
        language: SCRIPT_LANGUAGE.into(),
        lines,
    })
}

/// Streaming, started, and interrupted card: the script as source.
pub(super) fn preview_card(arguments: &Value, status: ToolStatus) -> ToolCard {
    card(status, None, arguments)
}

/// Live card: the latest call rows above the script. `rows` is the bridge's
/// progress text, one rendered row per nested call in start order.
pub(super) fn progress_card(arguments: &Value, rows: &str) -> ToolCard {
    let rows: Vec<&str> = rows.lines().filter(|row| !row.trim().is_empty()).collect();
    let mut card = card(ToolStatus::Running, None, arguments);
    let earlier = rows.len().saturating_sub(CALL_ROWS);
    if earlier > 0 {
        card.push_fact(ToolFact::Meta {
            text: format!("… {earlier} earlier calls"),
        });
    }
    for row in &rows[earlier..] {
        card.push_fact(if row.starts_with('✗') {
            ToolFact::Error {
                text: (*row).into(),
            }
        } else {
            ToolFact::Text {
                text: (*row).into(),
            }
        });
    }
    card
}

/// Finished card: the call count, the failure reason when the script failed,
/// and the script. `calls` is the nested-call count from structured output;
/// replayed history has none, so the header omits it there.
pub(super) fn finished_card(
    arguments: &Value,
    content: &str,
    ok: bool,
    calls: Option<usize>,
) -> ToolCard {
    let primary = match calls {
        None | Some(0) => None,
        Some(1) => Some("1 call".into()),
        Some(count) => Some(format!("{count} calls")),
    };
    let mut card = card(ToolStatus::from_finished(ok), primary, arguments);
    if !ok {
        if let Some(reason) = failure_reason(content) {
            card.push_fact(ToolFact::Error { text: reason });
        }
    }
    card
}

/// Script failures end the model text with `script failed: ...` after any
/// prints; other failures (bad arguments, cancellation) are the whole text.
fn failure_reason(content: &str) -> Option<String> {
    let line = content
        .lines()
        .rfind(|line| line.starts_with("script failed: "))
        .or_else(|| content.lines().find(|line| !line.trim().is_empty()))?;
    Some(line.trim().to_string())
}

#[cfg(test)]
#[path = "interactive_presenter_codemode_tests.rs"]
mod tests;
