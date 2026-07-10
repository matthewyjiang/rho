use serde_json::Value;

use crate::tool::ToolResult;

pub(super) fn web_search(args: &Value, result: &ToolResult) -> String {
    let Ok(content) = serde_json::from_str::<Value>(&result.content) else {
        return with_search_terms("web search finished".into(), args);
    };
    let answer = content
        .get("answer")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let result_count = answer
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    let status = if answer.starts_with("No configured search provider") {
        "no live results".to_string()
    } else {
        format!("{} stored", pluralize(result_count, "result"))
    };
    with_search_terms(format!("web search: {status}"), args)
}

pub(super) fn fetch_content(result: &ToolResult) -> String {
    let Ok(content) = serde_json::from_str::<Value>(&result.content) else {
        return "fetch content finished".into();
    };
    let item_count = content
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    format!("fetch content: fetched {}", pluralize(item_count, "item"))
}

pub(super) fn get_search_content(result: &ToolResult) -> String {
    let Ok(content) = serde_json::from_str::<Value>(&result.content) else {
        return "retrieved stored content".into();
    };
    if let Some(query) = content.get("query").and_then(Value::as_str) {
        return format!("retrieved content for {}", quoted_display_value(query, 80));
    }
    let label = content
        .get("title")
        .and_then(Value::as_str)
        .or_else(|| content.get("url").and_then(Value::as_str))
        .map(|value| truncate_display_value(value, 80))
        .unwrap_or_else(|| "stored content".into());
    format!("retrieved content: {label}")
}

fn with_search_terms(message: String, args: &Value) -> String {
    search_terms_display(args)
        .map(|terms| format!("{message} for {terms}"))
        .unwrap_or(message)
}

fn search_terms_display(args: &Value) -> Option<String> {
    let terms = args
        .get("queries")
        .and_then(Value::as_array)
        .map(|queries| queries.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .filter(|queries| !queries.is_empty())
        .or_else(|| {
            args.get("query")
                .and_then(Value::as_str)
                .map(|query| vec![query])
        })?;
    let mut rendered = terms
        .iter()
        .take(3)
        .map(|term| quoted_display_value(term, 48))
        .collect::<Vec<_>>();
    if terms.len() > rendered.len() {
        rendered.push(format!("{} more", terms.len() - rendered.len()));
    }
    Some(rendered.join(", "))
}

fn pluralize(count: usize, label: &str) -> String {
    if count == 1 {
        format!("1 {label}")
    } else {
        format!("{count} {label}s")
    }
}

fn quoted_display_value(value: &str, max_chars: usize) -> String {
    format!("\"{}\"", truncate_display_value(value, max_chars))
}

fn truncate_display_value(value: &str, max_chars: usize) -> String {
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
