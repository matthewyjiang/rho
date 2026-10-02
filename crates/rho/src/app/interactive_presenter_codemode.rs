//! Codemode cards keep the script as highlighted source. During execution,
//! a compact header shows current work and completed calls without competing
//! with the source for the collapsed child-row budget.

use rho_sdk::tool::ToolProgress;
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
/// Streaming, started, and interrupted cards pass no detail.
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

/// The bridge supplies a bounded summary and structured call counts. Keep
/// progress in the header so even wrapped status cannot hide the source.
/// Full nested-call snapshots remain available to protocol hosts.
pub(super) fn progress_card(arguments: &Value, progress: &ToolProgress) -> ToolCard {
    let mut details = Vec::new();
    if let (Some(completed), Some(started)) = (progress.completed_units(), progress.total_units()) {
        details.push(format!("{completed}/{started} completed"));
    }
    if let Some(summary) = progress.presentation().command_summary_text() {
        details.push(summary.to_owned());
    }
    let primary = (!details.is_empty()).then(|| details.join(" · "));
    preview_card(arguments, ToolStatus::Running, primary)
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
