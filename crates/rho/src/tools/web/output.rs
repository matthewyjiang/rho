//! Compact model-facing text for web_search, fetch_content, and get_search_content.

use super::storage::StoredItem;

pub(super) fn format_web_search(response_id: &str, summaries: &[String]) -> String {
    let mut out = format!("responseId: {response_id}");
    if !summaries.is_empty() {
        out.push('\n');
        out.push_str(&summaries.join("\n"));
    }
    out
}

pub(super) fn format_stored_item(item: &StoredItem, max_output_bytes: usize) -> String {
    fit_header_and_body(&item_header(item), &item.content, max_output_bytes)
}

pub(super) fn format_single_fetch(
    response_id: &str,
    item: &StoredItem,
    max_output_bytes: usize,
) -> String {
    let mut header = format!("responseId: {response_id}");
    push_header_field(&mut header, "url", item.url.as_deref());
    push_header_field(&mut header, "title", item.title.as_deref());
    let rendered = join_header_body(&header, &item.content);
    if rendered.len() <= max_output_bytes {
        return rendered;
    }
    header.push_str("\ntruncated");
    fit_header_and_body(&header, &item.content, max_output_bytes)
}

pub(super) fn format_multi_fetch(
    response_id: &str,
    items: &[StoredItem],
    max_output_bytes: usize,
) -> String {
    let mut out = format!("responseId: {response_id}");
    for (index, item) in items.iter().enumerate() {
        let label = item
            .url
            .as_deref()
            .or(item.title.as_deref())
            .unwrap_or("item");
        out.push_str(&format!("\n{index}. {label}"));
    }
    if out.len() <= max_output_bytes {
        return out;
    }
    truncate_chars(&out, max_output_bytes)
}

fn item_header(item: &StoredItem) -> String {
    let mut header = String::new();
    push_header_field(&mut header, "url", item.url.as_deref());
    push_header_field(&mut header, "title", item.title.as_deref());
    push_header_field(&mut header, "query", item.query.as_deref());
    header
}

fn push_header_field(header: &mut String, key: &str, value: Option<&str>) {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return;
    };
    if !header.is_empty() {
        header.push('\n');
    }
    header.push_str(key);
    header.push_str(": ");
    header.push_str(value);
}

fn join_header_body(header: &str, body: &str) -> String {
    if header.is_empty() {
        body.to_string()
    } else if body.is_empty() {
        header.to_string()
    } else {
        format!("{header}\n\n{body}")
    }
}

fn fit_header_and_body(header: &str, body: &str, max_output_bytes: usize) -> String {
    let rendered = join_header_body(header, body);
    if rendered.len() <= max_output_bytes {
        return rendered;
    }
    if header.is_empty() {
        return truncate_chars(body, max_output_bytes);
    }
    if header.len() >= max_output_bytes {
        return truncate_chars(header, max_output_bytes);
    }
    let separator = 2; // "\n\n"
    let budget = max_output_bytes.saturating_sub(header.len() + separator);
    if budget == 0 {
        return truncate_chars(header, max_output_bytes);
    }
    format!("{header}\n\n{}", truncate_chars(body, budget))
}

fn truncate_chars(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    text[..rho_sdk::floor_char_boundary(text, max_bytes)].to_string()
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod tests;
