//! Codemode cards keep the script as highlighted source. During execution,
//! nested-call progress appears as plain facts above it, so tool output never
//! replaces the source or gets highlighted as Starlark.

use rho_tools::tool_card::{ToolBody, ToolCard, ToolFact, ToolFamily, ToolHeader, ToolStatus};
use serde_json::Value;

use super::{format::string_arg, ToolBodySyntax};

/// Starlark is Python-shaped, and the bundled syntax set has no Starlark grammar.
const SCRIPT_LANGUAGE: &str = "python";

pub(super) fn body_syntax() -> ToolBodySyntax {
    ToolBodySyntax::Code {
        language: SCRIPT_LANGUAGE.into(),
    }
}

/// Every codemode card: the script as source, with an optional header detail.
/// Streaming, started, running, and interrupted cards pass no detail.
pub(super) fn preview_card(
    arguments: &Value,
    status: ToolStatus,
    primary: Option<String>,
) -> ToolCard {
    let lines = string_arg(arguments, "script")
        .map(|script| script.trim_end().lines().map(str::to_string).collect())
        .unwrap_or_default();
    ToolCard::new(
        status,
        ToolFamily::Default,
        ToolHeader::call(crate::tools::code_mode::CODEMODE_TOOL_NAME, primary),
    )
    .with_body(ToolBody::Lines(lines))
}

/// Bridge updates are complete snapshots, not deltas. Rebuild the facts each
/// time while leaving the syntax-highlighted source in the body.
pub(super) fn progress_card(arguments: &Value, text: &str) -> ToolCard {
    let mut card = preview_card(arguments, ToolStatus::Running, /*primary*/ None);
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        card.push_fact(ToolFact::Meta {
            text: line.to_owned(),
        });
    }
    card
}

/// Finished card: the call count, the failure reason when the script failed,
/// and the script. Live structured output supplies the nested-call count and
/// full diagnostic; replayed history has only text and omits the count.
pub(super) fn finished_card(
    arguments: &Value,
    content: &str,
    ok: bool,
    data: Option<&Value>,
) -> ToolCard {
    let calls = data
        .and_then(|data| data.get("calls"))
        .and_then(Value::as_u64);
    let primary = match calls {
        None | Some(0) => None,
        Some(1) => Some("1 call".into()),
        Some(count) => Some(format!("{count} calls")),
    };
    let mut card = preview_card(arguments, ToolStatus::from_finished(ok), primary);
    if !ok {
        let reason = data
            .and_then(|data| data.get("error"))
            .and_then(Value::as_str)
            .filter(|error| !error.trim().is_empty())
            .map(|error| format!("script failed: {error}"))
            .or_else(|| failure_reason(content));
        if let Some(reason) = reason {
            card.push_fact(ToolFact::Error { text: reason });
        }
    }
    card
}

/// Historical output can contain printed lookalike markers. The last marker
/// owns the complete diagnostic suffix; without it, prints are not an error.
fn failure_reason(content: &str) -> Option<String> {
    let marker = "script failed: ";
    let start = content.rfind(marker)?;
    if content[start + marker.len()..].trim().is_empty() {
        return None;
    }
    Some(content[start..].trim_end().to_string())
}

#[cfg(test)]
#[path = "interactive_presenter_codemode_tests.rs"]
mod tests;
