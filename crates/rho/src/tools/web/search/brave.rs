use serde_json::Value;

use rho_tools::tool::ToolError;

use super::{apply_site_filters, read_bounded_text, search_error, SearchBackendConfig, SearchItem};

pub(super) async fn search(
    client: &reqwest::Client,
    query: &str,
    num_results: usize,
    recency_filter: Option<&str>,
    domain_filter: Option<&[String]>,
    key: &str,
    config: &SearchBackendConfig,
) -> Result<Vec<SearchItem>, ToolError> {
    let secrets = [key];
    let filtered_query = apply_site_filters(query, domain_filter);
    let count = num_results.to_string();
    let mut request = client
        .get(config.destination_url("res/v1/web/search")?)
        .query(&[("q", filtered_query.as_str()), ("count", count.as_str())]);
    if let Some(freshness) = brave_freshness(recency_filter) {
        request = request.query(&[("freshness", freshness)]);
    }
    let response = request
        .header("Accept", "application/json")
        .header("X-Subscription-Token", key)
        .send()
        .await
        .map_err(|err| search_error(format!("Brave search request failed: {err}"), &secrets))?;
    let (status, text) = read_bounded_text(response, &secrets).await?;
    if !status.is_success() {
        return Err(search_error(
            format!(
                "Brave search failed: HTTP {status}: {}",
                super::redact_secrets(&text, &secrets)
                    .chars()
                    .take(300)
                    .collect::<String>()
            ),
            &secrets,
        ));
    }
    let response: Value = serde_json::from_str(&text).map_err(|err| {
        search_error(
            format!("Brave search response was not JSON: {err}"),
            &secrets,
        )
    })?;

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

fn brave_freshness(recency_filter: Option<&str>) -> Option<&'static str> {
    match recency_filter {
        Some("day") => Some("pd"),
        Some("week") => Some("pw"),
        Some("month") => Some("pm"),
        Some("year") => Some("py"),
        Some(_) | None => None,
    }
}
