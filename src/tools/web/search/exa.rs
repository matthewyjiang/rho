use serde_json::{json, Value};

use crate::{credentials::WebSearchCredential, tool::ToolError};

use super::{
    openai::normalize_domain_filters, openai::openai_recency_label, SearchBackendConfig, SearchItem,
};
use crate::tools::web::util::http_client;

const EXA_ANSWER_URL: &str = "https://api.exa.ai/answer";
const EXA_SEARCH_URL: &str = "https://api.exa.ai/search";
const EXA_MCP_URL: &str = "https://mcp.exa.ai/mcp";

pub(super) async fn search(
    query: &str,
    num_results: usize,
    recency_filter: Option<&str>,
    domain_filter: Option<&[String]>,
    config: &SearchBackendConfig,
) -> Result<Vec<SearchItem>, ToolError> {
    if let Some(key) = std::env::var("EXA_API_KEY")
        .ok()
        .or_else(|| config.credential(WebSearchCredential::Exa))
    {
        exa_api_search(query, num_results, recency_filter, domain_filter, &key).await
    } else {
        exa_mcp_search(query, num_results, recency_filter, domain_filter).await
    }
}

async fn exa_api_search(
    query: &str,
    num_results: usize,
    recency_filter: Option<&str>,
    domain_filter: Option<&[String]>,
    key: &str,
) -> Result<Vec<SearchItem>, ToolError> {
    let use_search = recency_filter.is_some() || domain_filter.is_some() || num_results != 5;
    let domain_filters = exa_domain_filters(domain_filter);
    let mut body = if use_search {
        json!({
            "query": query,
            "type": "auto",
            "numResults": num_results.min(20),
            "contents": {"text": {"maxCharacters": 3000}, "highlights": true},
        })
    } else {
        json!({"query": query, "text": true})
    };
    if let Some(start) = recency_filter.and_then(exa_start_published_date) {
        body["startPublishedDate"] = json!(start);
    }
    if let Some(include) = domain_filters.get("includeDomains") {
        body["includeDomains"] = include.clone();
    }
    if let Some(exclude) = domain_filters.get("excludeDomains") {
        body["excludeDomains"] = exclude.clone();
    }

    let response = http_client()
        .post(if use_search {
            EXA_SEARCH_URL
        } else {
            EXA_ANSWER_URL
        })
        .header("x-api-key", key)
        .json(&body)
        .send()
        .await
        .map_err(|err| ToolError::Message(format!("Exa request failed: {err}")))?;
    let status = response.status();
    let value: Value = response
        .json()
        .await
        .map_err(|err| ToolError::Message(format!("Exa response was not JSON: {err}")))?;
    if !status.is_success() {
        return Err(ToolError::Message(format!(
            "Exa search failed: HTTP {status}: {value}"
        )));
    }
    Ok(value
        .get(if use_search { "results" } else { "citations" })
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(num_results.min(20))
        .filter_map(|item| {
            let url = item.get("url").and_then(Value::as_str)?.to_string();
            let snippet = item
                .get("text")
                .and_then(Value::as_str)
                .or_else(|| item.get("snippet").and_then(Value::as_str))
                .unwrap_or_default()
                .chars()
                .take(500)
                .collect();
            Some(SearchItem {
                title: item
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                url: Some(url),
                snippet,
            })
        })
        .collect())
}

async fn exa_mcp_search(
    query: &str,
    num_results: usize,
    recency_filter: Option<&str>,
    domain_filter: Option<&[String]>,
) -> Result<Vec<SearchItem>, ToolError> {
    let response = http_client()
        .post(EXA_MCP_URL)
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "web_search_exa",
                "arguments": {
                    "query": exa_mcp_query(query, recency_filter, domain_filter),
                    "numResults": num_results.min(20),
                    "livecrawl": "fallback",
                    "type": "auto",
                    "contextMaxCharacters": 3000,
                }
            }
        }))
        .send()
        .await
        .map_err(|err| ToolError::Message(format!("Exa MCP request failed: {err}")))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|err| ToolError::Message(format!("Exa MCP response failed: {err}")))?;
    if !status.is_success() {
        return Err(ToolError::Message(format!(
            "Exa MCP failed: HTTP {status}: {}",
            text.chars().take(300).collect::<String>()
        )));
    }
    let text = parse_exa_mcp_text(&text)?;
    Ok(parse_exa_mcp_results(&text)
        .into_iter()
        .take(num_results.min(20))
        .collect())
}

fn parse_exa_mcp_text(body: &str) -> Result<String, ToolError> {
    for payload in body
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim))
        .chain(std::iter::once(body.trim()))
    {
        if payload.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        if let Some(error) = value.get("error") {
            return Err(ToolError::Message(format!("Exa MCP error: {error}")));
        }
        if let Some(text) = value
            .get("result")
            .and_then(|result| result.get("content"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find_map(|item| item.get("text").and_then(Value::as_str))
        {
            return Ok(text.to_string());
        }
    }
    Err(ToolError::Message("Exa MCP returned empty content".into()))
}

fn parse_exa_mcp_results(text: &str) -> Vec<SearchItem> {
    text.split("\n---")
        .filter_map(|block| {
            let title = block
                .lines()
                .find_map(|line| line.strip_prefix("Title: "))
                .unwrap_or("result")
                .trim();
            let url = block
                .lines()
                .find_map(|line| line.strip_prefix("URL: "))
                .map(str::trim)?;
            let content = block
                .split("\nText: ")
                .nth(1)
                .or_else(|| block.split("\nHighlights:\n").nth(1))
                .unwrap_or_default()
                .trim()
                .chars()
                .take(500)
                .collect();
            Some(SearchItem {
                title: Some(title.to_string()),
                url: Some(url.to_string()),
                snippet: content,
            })
        })
        .collect()
}

fn exa_mcp_query(
    query: &str,
    recency_filter: Option<&str>,
    domain_filter: Option<&[String]>,
) -> String {
    let mut parts = vec![query.to_string()];
    if let Some(filters) = domain_filter {
        parts.extend(filters.iter().map(|domain| {
            if let Some(domain) = domain.strip_prefix('-') {
                format!("-site:{domain}")
            } else {
                format!("site:{domain}")
            }
        }));
    }
    if let Some(recency) = recency_filter.and_then(openai_recency_label) {
        parts.push(recency.to_string());
    }
    parts.join(" ")
}

fn exa_domain_filters(domain_filter: Option<&[String]>) -> serde_json::Map<String, Value> {
    let filters = normalize_domain_filters(domain_filter);
    let mut map = serde_json::Map::new();
    if !filters.allowed.is_empty() {
        map.insert("includeDomains".into(), json!(filters.allowed));
    }
    if !filters.blocked.is_empty() {
        map.insert("excludeDomains".into(), json!(filters.blocked));
    }
    map
}

fn exa_start_published_date(_recency_filter: &str) -> Option<String> {
    None
}
