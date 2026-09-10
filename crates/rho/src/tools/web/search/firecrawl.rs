use serde_json::{json, Value};

use rho_tools::tool::ToolError;

use super::{
    normalize_domain_filters, read_bounded_text, search_error, SearchBackendConfig, SearchItem,
};

pub(super) async fn search(
    client: &reqwest::Client,
    query: &str,
    num_results: usize,
    recency_filter: Option<&str>,
    domain_filter: Option<&[String]>,
    key: Option<&str>,
    config: &SearchBackendConfig,
) -> Result<Vec<SearchItem>, ToolError> {
    let url = config.destination_url("v2/search")?;
    let secrets: Vec<&str> = key.into_iter().collect();
    let filters = normalize_domain_filters(domain_filter);
    // Firecrawl cannot combine includeDomains and excludeDomains. Preserve
    // mixed filters with explicit exclusions in the query and an allowlist.
    let query = if !filters.allowed.is_empty() && !filters.blocked.is_empty() {
        format!(
            "{query} {}",
            filters
                .blocked
                .iter()
                .map(|domain| format!("-site:{domain}"))
                .collect::<Vec<_>>()
                .join(" ")
        )
    } else {
        query.to_string()
    };
    let mut body = json!({
        "query": query,
        "limit": num_results.clamp(1, 20),
        "sources": ["web"],
    });
    if !filters.allowed.is_empty() {
        body["includeDomains"] = json!(filters.allowed);
    } else if !filters.blocked.is_empty() {
        body["excludeDomains"] = json!(filters.blocked);
    }
    if let Some(tbs) = firecrawl_tbs(recency_filter) {
        body["tbs"] = json!(tbs);
    }

    let request = authenticated_request(client, &url, &body, key);

    let response = request.send().await.map_err(|error| {
        search_error(
            format!("Firecrawl search request failed: {error}"),
            &secrets,
        )
    })?;
    let (status, text) = read_bounded_text(response, &secrets).await?;
    if !status.is_success() {
        return Err(search_error(
            format!(
                "Firecrawl search failed: HTTP {status}: {}",
                super::redact_secrets(&text, &secrets)
                    .chars()
                    .take(300)
                    .collect::<String>()
            ),
            &secrets,
        ));
    }

    let value: Value = serde_json::from_str(&text).map_err(|error| {
        search_error(
            format!("Firecrawl search returned malformed JSON: {error}"),
            &secrets,
        )
    })?;
    if value.get("success").and_then(Value::as_bool) == Some(false) {
        return Err(search_error(
            format!(
                "Firecrawl search failed: {}",
                value
                    .get("error")
                    .or_else(|| value.get("message"))
                    .unwrap_or(&value)
            ),
            &secrets,
        ));
    }

    if value.get("success").and_then(Value::as_bool) != Some(true) {
        return Err(ToolError::Message(
            "Firecrawl search returned malformed status".into(),
        ));
    }

    let results = value
        .get("data")
        .and_then(|data| data.get("web"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            search_error(
                "Firecrawl search returned malformed results".into(),
                &secrets,
            )
        })?;

    Ok(results
        .iter()
        .take(num_results.min(20))
        .filter_map(|item| {
            let url = item.get("url").and_then(Value::as_str)?.to_string();
            Some(SearchItem {
                title: item
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                url: Some(url),
                snippet: item
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
        })
        .collect())
}

/// Keep auth optional for selfhost, using the same bearer contract as cloud.
pub(super) fn authenticated_request(
    client: &reqwest::Client,
    url: &str,
    body: &Value,
    key: Option<&str>,
) -> reqwest::RequestBuilder {
    let request = client.post(url).json(body);
    match key {
        Some(key) => request.bearer_auth(key),
        None => request,
    }
}

fn firecrawl_tbs(recency_filter: Option<&str>) -> Option<&'static str> {
    match recency_filter {
        Some("day") => Some("qdr:d"),
        Some("week") => Some("qdr:w"),
        Some("month") => Some("qdr:m"),
        Some("year") => Some("qdr:y"),
        Some(_) | None => None,
    }
}
