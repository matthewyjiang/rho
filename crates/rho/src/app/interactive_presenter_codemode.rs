//! Codemode cards show the script as highlighted source: what the model ran,
//! not what it printed. The card keeps one shape from start to finish; nested
//! calls are usually too fast to watch, and rows that vanish on completion make
//! the transcript jump, so the TUI ignores the bridge's progress rows (ACP and
//! the automation protocol still forward them).

use rho_tools::tool_card::{ToolBody, ToolCard, ToolFact, ToolFamily, ToolHeader, ToolStatus};
use serde_json::Value;

use super::format::string_arg;

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

/// Streaming, started, running, and interrupted card: the script as source.
pub(super) fn preview_card(arguments: &Value, status: ToolStatus) -> ToolCard {
    card(status, None, arguments)
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
