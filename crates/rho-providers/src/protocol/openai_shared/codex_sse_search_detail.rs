//! Bare detail strings for hosted search activity cards.

/// Max chars for a single search/url detail string in activity previews.
pub(super) const DETAIL_MAX_CHARS: usize = 80;
/// Max chars per query when rendering multi-query search previews.
const QUERY_MAX_CHARS: usize = 48;

/// Bare semantic detail for a hosted search card (shown as a child fact).
///
/// Returns payload values only — no English chrome like `for` / `opened` /
/// `found`. The TUI owns presentation (verb header + optional detail fact +
/// `finished`).
///
/// Codex-style items put fields under `action`. xAI emits hosted X tools as
/// `custom_tool_call` with JSON in `input`. Older shapes may use `x_search_call`
/// and `arguments`. Empty detail is fine: the activity still emits.
pub(super) fn extract_codex_search_detail(item: &serde_json::Value) -> Option<String> {
    item.get("action")
        .and_then(detail_from_search_action)
        .or_else(|| detail_from_search_arguments(item))
}

fn detail_from_search_action(action: &serde_json::Value) -> Option<String> {
    detail_from_search_payload(action)
        .or_else(|| first_nonempty_detail_field(action, &["url", "pattern"]))
}

fn detail_from_search_arguments(item: &serde_json::Value) -> Option<String> {
    let arguments = item
        .get("arguments")
        .or_else(|| item.get("input"))
        .and_then(|value| value.as_str())?;
    let args: serde_json::Value = serde_json::from_str(arguments).ok()?;
    if let Some(detail) = detail_from_search_payload(&args) {
        return Some(detail);
    }
    first_nonempty_detail_field(&args, &["post_id", "username", "url", "prompt"])
}

fn first_nonempty_detail_field(payload: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        payload
            .get(*key)
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .map(|value| truncate_detail(value, DETAIL_MAX_CHARS))
    })
}

fn detail_from_search_payload(payload: &serde_json::Value) -> Option<String> {
    if let Some(query) = payload
        .get("query")
        .and_then(|query| query.as_str())
        .filter(|query| !query.is_empty())
    {
        return Some(truncate_detail(query, DETAIL_MAX_CHARS));
    }
    let queries = payload
        .get("queries")
        .and_then(|queries| queries.as_array())?
        .iter()
        .filter_map(|query| query.as_str())
        .filter(|query| !query.is_empty())
        .collect::<Vec<_>>();
    if queries.is_empty() {
        return None;
    }
    let mut rendered = queries
        .iter()
        .take(3)
        .map(|query| truncate_detail(query, QUERY_MAX_CHARS))
        .collect::<Vec<_>>();
    if queries.len() > rendered.len() {
        rendered.push(format!("{} more", queries.len() - rendered.len()));
    }
    Some(rendered.join(" · "))
}

pub(super) fn truncate_detail(value: &str, max_chars: usize) -> String {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.chars().count() <= max_chars {
        return value;
    }
    let mut truncated = value
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}
