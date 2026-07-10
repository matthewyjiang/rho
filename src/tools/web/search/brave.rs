use serde_json::Value;

use crate::{credentials::WebSearchCredential, tool::ToolError};

use super::{SearchBackendConfig, SearchItem};
use crate::tools::web::util::http_client;

pub(in crate::tools::web) fn resolve_api_key(config: &SearchBackendConfig) -> Option<String> {
    std::env::var("BRAVE_SEARCH_API_KEY")
        .or_else(|_| std::env::var("BRAVE_API_KEY"))
        .ok()
        .or_else(|| config.credential(WebSearchCredential::Brave))
}

pub(super) async fn search(
    query: &str,
    num_results: usize,
    recency_filter: Option<&str>,
    domain_filter: Option<&[String]>,
    config: &SearchBackendConfig,
) -> Result<Vec<SearchItem>, ToolError> {
    let key = resolve_api_key(config)
        .ok_or_else(|| ToolError::Message("BRAVE_SEARCH_API_KEY is not set".into()))?;
    let filtered_query = apply_domain_filter(query, domain_filter);
    let count = num_results.to_string();
    let mut request = http_client()
        .get("https://api.search.brave.com/res/v1/web/search")
        .query(&[("q", filtered_query.as_str()), ("count", count.as_str())]);
    if let Some(freshness) = brave_freshness(recency_filter) {
        request = request.query(&[("freshness", freshness)]);
    }
    let response: Value = request
        .header("Accept", "application/json")
        .header("X-Subscription-Token", key)
        .send()
        .await
        .map_err(|err| ToolError::Message(format!("Brave search request failed: {err}")))?
        .error_for_status()
        .map_err(|err| ToolError::Message(format!("Brave search failed: {err}")))?
        .json()
        .await
        .map_err(|err| ToolError::Message(format!("Brave search response was not JSON: {err}")))?;

    let results = response
        .get("web")
        .and_then(|web| web.get("results"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(num_results)
        .map(|item| SearchItem {
            title: item
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string),
            url: item.get("url").and_then(Value::as_str).map(str::to_string),
            snippet: item
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        })
        .collect();
    Ok(results)
}

fn apply_domain_filter(query: &str, domain_filter: Option<&[String]>) -> String {
    let Some(filters) = domain_filter else {
        return query.to_string();
    };
    let filters = filters
        .iter()
        .map(|domain| domain.trim())
        .filter(|domain| !domain.is_empty())
        .map(|domain| {
            domain
                .strip_prefix('-')
                .map(|domain| format!("-site:{domain}"))
                .unwrap_or_else(|| format!("site:{domain}"))
        })
        .collect::<Vec<_>>();
    if filters.is_empty() {
        query.to_string()
    } else {
        format!("{} {}", query, filters.join(" "))
    }
}

fn brave_freshness(recency_filter: Option<&str>) -> Option<&'static str> {
    match recency_filter {
        Some("day") => Some("pd"),
        Some("week") => Some("pw"),
        Some("month") => Some("pm"),
        Some("year") => Some("py"),
        Some(_) | None => None,
    }
}
