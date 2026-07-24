use serde_json::{json, Value};

use rho_tools::tool::truncate;

use super::{storage::StoredItem, util::to_pretty_json};

const FETCH_CONTENT_TOOL: &str = "fetch_content";

/// Builds agent-facing fetch_content JSON.
///
/// Single targets inline as much body text as fits `max_output_bytes`. Multi-target
/// calls keep short previews and point at `get_search_content` with urlIndex.
pub(super) fn build_fetch_content_output(
    response_id: &str,
    items: &[StoredItem],
    previews: &[Value],
    max_output_bytes: usize,
) -> String {
    if let (1, Some(item)) = (items.len(), items.first()) {
        return build_single_fetch_output(response_id, item, max_output_bytes);
    }

    let content = json!({
        "responseId": response_id,
        "type": FETCH_CONTENT_TOOL,
        "items": previews,
        "itemCount": items.len(),
        "fullContentAvailable": true,
        "contentTruncated": true,
        "note": "Multiple targets were fetched. Full bodies are stored under responseId. Call get_search_content with only responseId or urlIndex/url; do not invent query keywords or read cache paths."
    });
    truncate(to_pretty_json(&content), max_output_bytes)
}

fn build_single_fetch_output(
    response_id: &str,
    item: &StoredItem,
    max_output_bytes: usize,
) -> String {
    let mut body = item.content.clone();
    let mut truncated = false;

    for _ in 0..8 {
        let note = if truncated {
            Some(
                "Content truncated to the tool output limit. Full body is stored under responseId. Call get_search_content with only responseId (no free-text query).",
            )
        } else {
            None
        };
        let payload = json!({
            "responseId": response_id,
            "type": FETCH_CONTENT_TOOL,
            "url": item.url,
            "title": item.title,
            "content": body,
            "contentTruncated": truncated,
            "fullContentAvailable": true,
            "note": note,
        });
        let rendered = to_pretty_json(&payload);
        if rendered.len() <= max_output_bytes {
            return rendered;
        }

        let overflow = rendered.len() - max_output_bytes;
        let next_len = body.len().saturating_sub(overflow.saturating_add(64));
        if next_len == 0 || next_len >= body.len() {
            return truncate(rendered, max_output_bytes);
        }
        body = truncate(body, next_len);
        truncated = true;
    }

    truncate(
        to_pretty_json(&json!({
            "responseId": response_id,
            "type": FETCH_CONTENT_TOOL,
            "url": item.url,
            "title": item.title,
            "content": body,
            "contentTruncated": true,
            "fullContentAvailable": true,
            "note": "Content truncated to the tool output limit. Full body is stored under responseId. Call get_search_content with only responseId (no free-text query).",
        })),
        max_output_bytes,
    )
}

#[cfg(test)]
#[path = "fetch_response_tests.rs"]
mod tests;
