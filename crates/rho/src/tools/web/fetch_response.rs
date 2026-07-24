use serde_json::{json, Value};

use super::{storage::StoredItem, util::to_pretty_json};

pub(super) const FETCH_CONTENT_TOOL: &str = "fetch_content";

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
    fit_json(content, max_output_bytes)
}

fn build_single_fetch_output(
    response_id: &str,
    item: &StoredItem,
    max_output_bytes: usize,
) -> String {
    let full = single_payload(response_id, item, &item.content, false, None);
    let rendered = to_pretty_json(&full);
    if rendered.len() <= max_output_bytes {
        return rendered;
    }

    let note = "Content truncated to the tool output limit. Full body is stored under responseId. Call get_search_content with only responseId (no free-text query).";
    let empty = to_pretty_json(&single_payload(response_id, item, "", true, Some(note)));
    let overhead = empty.len();
    let mut budget = max_output_bytes.saturating_sub(overhead);
    // JSON string escaping can expand content; leave a small cushion and retry once.
    for _ in 0..2 {
        let body = utf8_prefix(&item.content, budget);
        let rendered = to_pretty_json(&single_payload(response_id, item, &body, true, Some(note)));
        if rendered.len() <= max_output_bytes {
            return rendered;
        }
        let overflow = rendered.len() - max_output_bytes;
        budget = body.len().saturating_sub(overflow.saturating_add(8));
        if budget == 0 {
            break;
        }
    }

    // Last resort: keep a valid JSON envelope with an empty body rather than
    // slicing JSON bytes mid-document.
    empty
}

fn single_payload(
    response_id: &str,
    item: &StoredItem,
    body: &str,
    truncated: bool,
    note: Option<&str>,
) -> Value {
    json!({
        "responseId": response_id,
        "type": FETCH_CONTENT_TOOL,
        "url": item.url,
        "title": item.title,
        "content": body,
        "itemCount": 1,
        "contentTruncated": truncated,
        "fullContentAvailable": true,
        "note": note,
    })
}

fn fit_json(value: Value, max_output_bytes: usize) -> String {
    let rendered = to_pretty_json(&value);
    if rendered.len() <= max_output_bytes {
        return rendered;
    }
    let response_id = value.get("responseId").cloned().unwrap_or(Value::Null);
    let item_count = value.get("itemCount").cloned().unwrap_or(json!(0));
    // Multi-target previews are already short; if the envelope itself is too
    // large, drop preview payloads rather than emit invalid JSON.
    let mut fallback = value;
    if let Some(object) = fallback.as_object_mut() {
        object.insert("items".into(), json!([]));
        object.insert(
            "note".into(),
            json!("Tool output limit reached. Call get_search_content with responseId or urlIndex/url for full bodies."),
        );
    }
    let rendered = to_pretty_json(&fallback);
    if rendered.len() <= max_output_bytes {
        rendered
    } else {
        // Extremely small limits: still return valid JSON.
        to_pretty_json(&json!({
            "responseId": response_id,
            "type": FETCH_CONTENT_TOOL,
            "itemCount": item_count,
            "fullContentAvailable": true,
            "contentTruncated": true,
            "note": "Tool output limit reached. Call get_search_content with responseId.",
        }))
    }
}

fn utf8_prefix(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let mut end = max_bytes.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

#[cfg(test)]
#[path = "fetch_response_tests.rs"]
mod tests;
