//! `tool_search` cards: the query, a hit count, and one `name · description`
//! row per hit. The raw output carries full parameter and return schemas the
//! model needs but a reader does not.

use rho_tools::tool_card::{ToolBody, ToolCard, ToolFact, ToolFamily, ToolHeader, ToolStatus};
use serde_json::Value;

use super::format::{push_error_output, quoted, split_body_lines, string_arg, truncate};
use crate::tools::code_mode::{ToolCatalogEntry, TOOL_SEARCH_NAME};

/// Header query budget, matching `get_search_content`.
const QUERY_MAX_CHARS: usize = 80;
/// Description budget per hit row. A 100-column pane leaves ~96 columns after
/// the card indent; MCP names run ~25-30 chars, so 60 keeps a hit on one row.
const DESCRIPTION_MAX_CHARS: usize = 60;

pub(super) fn preview_card(arguments: &Value, status: ToolStatus) -> ToolCard {
    let primary = string_arg(arguments, "query")
        .filter(|query| !query.trim().is_empty())
        .map(|query| quoted(&query, QUERY_MAX_CHARS));
    ToolCard::new(
        status,
        ToolFamily::Default,
        ToolHeader::call(TOOL_SEARCH_NAME, primary),
    )
}

/// Hits arrive as JSON text, live and replayed alike. Anything else (the
/// tool's own miss notice) is shown as written rather than reinterpreted.
pub(super) fn finished_card(arguments: &Value, content: &str, ok: bool) -> ToolCard {
    let mut card = preview_card(arguments, ToolStatus::from_finished(ok));
    if !ok {
        push_error_output(&mut card, content);
        return card;
    }
    match serde_json::from_str::<Vec<ToolCatalogEntry>>(content) {
        Ok(hits) if !hits.is_empty() => {
            let count = hits.len() as u64;
            card.push_fact(ToolFact::Count {
                label: if count == 1 { "tool" } else { "tools" }.into(),
                value: count,
                detail: None,
            });
            card.body = ToolBody::Lines(hits.iter().map(hit_row).collect());
        }
        Ok(_) | Err(_) => card.body = ToolBody::Lines(split_body_lines(content.trim())),
    }
    card
}

/// `name · first sentence`, so long tool docs stay one row.
fn hit_row(hit: &ToolCatalogEntry) -> String {
    let first = hit
        .description
        .split_once(". ")
        .map_or(hit.description.as_str(), |(sentence, _)| sentence)
        .trim_end_matches('.');
    let summary = truncate(first, DESCRIPTION_MAX_CHARS);
    if summary.is_empty() {
        hit.name.clone()
    } else {
        format!("{} · {summary}", hit.name)
    }
}

#[cfg(test)]
#[path = "interactive_presenter_tool_search_tests.rs"]
mod tests;
