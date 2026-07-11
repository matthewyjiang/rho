use std::collections::HashSet;

use serde_json::{json, Value};
use url::Url;

use crate::{
    auth::codex_oauth::{chatgpt_plan_from_id_token, ChatGptPlan},
    credentials::{
        load_codex_tokens, load_provider_api_key, CodexTokens, OsCredentialStore,
        WebSearchCredential,
    },
    model::openai::auth::{refresh_codex_token, CodexAuthSource},
    tool::ToolError,
};

use super::{SearchBackendConfig, SearchItem};

const OPENAI_RESPONSES_URL: &str = "https://api.openai.com/v1/responses";
const CODEX_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
const OPENAI_SEARCH_MODEL: &str = "gpt-5.6-luna";
const CODEX_SEARCH_MODEL: &str = "gpt-5.6-terra";

#[derive(Clone, Debug)]
enum OpenAiSearchAuth {
    Codex {
        tokens: CodexTokens,
        source: CodexAuthSource,
    },
    ApiKey(String),
}

impl OpenAiSearchAuth {
    fn endpoint(&self) -> &'static str {
        match self {
            Self::Codex { .. } => CODEX_RESPONSES_URL,
            Self::ApiKey(_) => OPENAI_RESPONSES_URL,
        }
    }

    fn model(&self) -> &'static str {
        match self {
            Self::Codex { tokens, .. } => match tokens
                .id_token
                .as_deref()
                .map(chatgpt_plan_from_id_token)
                .unwrap_or(ChatGptPlan::Unknown)
            {
                ChatGptPlan::Free | ChatGptPlan::Go | ChatGptPlan::Unknown => CODEX_SEARCH_MODEL,
                ChatGptPlan::Plus
                | ChatGptPlan::Pro
                | ChatGptPlan::ProLite
                | ChatGptPlan::Team
                | ChatGptPlan::SelfServeBusinessUsageBased
                | ChatGptPlan::Business
                | ChatGptPlan::EnterpriseCbpUsageBased
                | ChatGptPlan::Enterprise
                | ChatGptPlan::Edu => OPENAI_SEARCH_MODEL,
            },
            Self::ApiKey(_) => OPENAI_SEARCH_MODEL,
        }
    }

    fn bearer_token(&self) -> &str {
        match self {
            Self::Codex { tokens, .. } => &tokens.access_token,
            Self::ApiKey(key) => key,
        }
    }
}

pub(super) fn is_available(config: &SearchBackendConfig) -> bool {
    resolve_auth(config).is_ok()
}

fn resolve_auth(config: &SearchBackendConfig) -> Result<OpenAiSearchAuth, ToolError> {
    if let Ok(access_token) = std::env::var("CODEX_ACCESS_TOKEN") {
        return Ok(OpenAiSearchAuth::Codex {
            tokens: CodexTokens {
                access_token,
                refresh_token: None,
                id_token: None,
                account_id: std::env::var("CODEX_ACCOUNT_ID").ok(),
            },
            source: CodexAuthSource::Env,
        });
    }
    if let Ok(Some(tokens)) = load_codex_tokens(&OsCredentialStore) {
        return Ok(OpenAiSearchAuth::Codex {
            tokens,
            source: CodexAuthSource::Store,
        });
    }
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        return Ok(OpenAiSearchAuth::ApiKey(key));
    }
    if let Some(key) = config.credential(WebSearchCredential::OpenAi) {
        return Ok(OpenAiSearchAuth::ApiKey(key));
    }
    if let Ok(Some(key)) = load_provider_api_key(&OsCredentialStore, "openai") {
        return Ok(OpenAiSearchAuth::ApiKey(key));
    }
    Err(ToolError::Message(
        "OpenAI web search unavailable: sign in with /login openai-codex, /login openai, or set OPENAI_API_KEY"
            .into(),
    ))
}

pub(super) async fn search(
    client: &reqwest::Client,
    query: &str,
    num_results: usize,
    recency_filter: Option<&str>,
    domain_filter: Option<&[String]>,
    config: &SearchBackendConfig,
) -> Result<Vec<SearchItem>, ToolError> {
    let mut auth = resolve_auth(config)?;
    let body = openai_search_body(&auth, query, num_results, recency_filter, domain_filter);
    let (mut status, mut text) = send_openai_search_request(client, &auth, &body).await?;
    if status == reqwest::StatusCode::UNAUTHORIZED {
        if let OpenAiSearchAuth::Codex {
            tokens,
            source: CodexAuthSource::Store,
        } = &auth
        {
            if let Some(refresh_token) = tokens.refresh_token.as_deref() {
                let store = OsCredentialStore;
                let refreshed = refresh_codex_token(
                    client,
                    &store,
                    refresh_token,
                    CodexAuthSource::Store,
                    tokens,
                )
                .await
                .map_err(|err| {
                    ToolError::Message(format!("OpenAI web search token refresh failed: {err}"))
                })?;
                auth = OpenAiSearchAuth::Codex {
                    tokens: refreshed,
                    source: CodexAuthSource::Store,
                };
                let body =
                    openai_search_body(&auth, query, num_results, recency_filter, domain_filter);
                let retried = send_openai_search_request(client, &auth, &body).await?;
                status = retried.0;
                text = retried.1;
            }
        }
    }
    if !status.is_success() {
        return Err(ToolError::Message(format!(
            "OpenAI web search failed: HTTP {status}: {}",
            text.chars().take(300).collect::<String>()
        )));
    }

    let output = parse_openai_search_output(&text)?;
    let results = extract_openai_search_results(&output, num_results);
    if results.is_empty() {
        Err(ToolError::Message(
            "OpenAI web_search returned no sources".into(),
        ))
    } else {
        Ok(results)
    }
}

fn openai_search_body(
    auth: &OpenAiSearchAuth,
    query: &str,
    num_results: usize,
    recency_filter: Option<&str>,
    domain_filter: Option<&[String]>,
) -> Value {
    json!({
        "model": auth.model(),
        "instructions": openai_search_instructions(num_results, recency_filter, domain_filter),
        "input": [{"role": "user", "content": [{"type": "input_text", "text": query}]}],
        "tools": [openai_web_search_tool(domain_filter)],
        "include": ["web_search_call.action.sources"],
        "store": false,
        "stream": true,
        "tool_choice": "required",
        "parallel_tool_calls": true,
    })
}

async fn send_openai_search_request(
    client: &reqwest::Client,
    auth: &OpenAiSearchAuth,
    body: &Value,
) -> Result<(reqwest::StatusCode, String), ToolError> {
    let mut request = client
        .post(auth.endpoint())
        .bearer_auth(auth.bearer_token())
        .header("Content-Type", "application/json");
    if let OpenAiSearchAuth::Codex { tokens, .. } = auth {
        request = request
            .header("User-Agent", "codex-cli")
            .header("originator", "codex_cli_rs");
        if let Some(account_id) = tokens.account_id.as_deref() {
            request = request.header("ChatGPT-Account-ID", account_id);
        }
    }

    let response =
        request.json(body).send().await.map_err(|err| {
            ToolError::Message(format!("OpenAI web search request failed: {err}"))
        })?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|err| ToolError::Message(format!("OpenAI web search response failed: {err}")))?;
    Ok((status, text))
}

fn openai_search_instructions(
    num_results: usize,
    recency_filter: Option<&str>,
    domain_filter: Option<&[String]>,
) -> String {
    let mut lines = vec![
        "Search the web and return concise source-backed results.".to_string(),
        "Prefer clickable source citations when possible.".to_string(),
        format!("Prefer around {} distinct sources.", num_results.min(20)),
    ];
    if let Some(recency) = recency_filter.and_then(openai_recency_label) {
        lines.push(format!("Prefer sources from the {recency}."));
    }
    let filters = normalize_domain_filters(domain_filter);
    if !filters.allowed.is_empty() {
        lines.push(format!(
            "Only use sources from: {}.",
            filters.allowed.join(", ")
        ));
    }
    if !filters.blocked.is_empty() {
        lines.push(format!(
            "Do not use sources from: {}.",
            filters.blocked.join(", ")
        ));
    }
    lines.join(" ")
}

pub(super) fn openai_recency_label(recency_filter: &str) -> Option<&'static str> {
    match recency_filter {
        "day" => Some("past 24 hours"),
        "week" => Some("past week"),
        "month" => Some("past month"),
        "year" => Some("past year"),
        _ => None,
    }
}

fn openai_web_search_tool(domain_filter: Option<&[String]>) -> Value {
    let filters = normalize_domain_filters(domain_filter);
    let mut tool = serde_json::Map::from_iter([("type".into(), json!("web_search"))]);
    if !filters.allowed.is_empty() || !filters.blocked.is_empty() {
        tool.insert(
            "filters".into(),
            json!({
                "allowed_domains": filters.allowed,
                "blocked_domains": filters.blocked,
            }),
        );
    }
    Value::Object(tool)
}

#[derive(Default)]
pub(super) struct DomainFilters {
    pub(super) allowed: Vec<String>,
    pub(super) blocked: Vec<String>,
}

pub(super) fn normalize_domain_filters(domain_filter: Option<&[String]>) -> DomainFilters {
    let mut filters = DomainFilters::default();
    for raw in domain_filter.into_iter().flatten() {
        let Some(domain) = normalize_domain(raw) else {
            continue;
        };
        let target = if raw.trim().starts_with('-') {
            &mut filters.blocked
        } else {
            &mut filters.allowed
        };
        if !target.contains(&domain) {
            target.push(domain);
        }
    }
    filters.allowed.truncate(100);
    filters.blocked.truncate(100);
    filters
}

fn normalize_domain(raw: &str) -> Option<String> {
    let mut input = raw
        .trim()
        .trim_start_matches('-')
        .trim()
        .to_ascii_lowercase();
    if input.is_empty() {
        return None;
    }
    if let Ok(url) = Url::parse(&input).or_else(|_| Url::parse(&format!("https://{input}"))) {
        input = url.host_str()?.to_string();
    } else {
        input = input.split('/').next()?.split(':').next()?.to_string();
    }
    let input = input.trim_matches('.').to_string();
    crate::tools::web::util::is_valid_domain(&input).then_some(input)
}

fn parse_openai_search_output(text: &str) -> Result<Vec<Value>, ToolError> {
    let trimmed = text.trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        let value: Value = serde_json::from_str(trimmed).map_err(|err| {
            ToolError::Message(format!("OpenAI web search returned invalid JSON: {err}"))
        })?;
        return Ok(value
            .get("output")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default());
    }

    let mut output = Vec::new();
    let mut completed_output = None;
    for line in text.lines() {
        let Some(data) = line.strip_prefix("data: ") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) == Some("response.output_item.done") {
            if let Some(item) = value.get("item") {
                output.push(item.clone());
            }
        }
        if matches!(
            value.get("type").and_then(Value::as_str),
            Some("response.done" | "response.completed")
        ) {
            completed_output = value
                .get("response")
                .and_then(|response| response.get("output"))
                .and_then(Value::as_array)
                .cloned();
        }
    }
    Ok(completed_output.unwrap_or(output))
}

fn extract_openai_search_results(output: &[Value], num_results: usize) -> Vec<SearchItem> {
    let mut results = Vec::new();
    let mut seen = HashSet::new();

    for item in output {
        if item.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        for part in item
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let text = part.get("text").and_then(Value::as_str).unwrap_or_default();
            for annotation in part
                .get("annotations")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if annotation.get("type").and_then(Value::as_str) != Some("url_citation") {
                    continue;
                }
                add_openai_search_result(
                    &mut results,
                    &mut seen,
                    annotation.get("url").and_then(Value::as_str),
                    annotation.get("title").and_then(Value::as_str),
                    citation_snippet(
                        text,
                        annotation.get("start_index").and_then(Value::as_u64),
                        annotation.get("end_index").and_then(Value::as_u64),
                    ),
                );
            }
        }
    }

    for item in output {
        if item.get("type").and_then(Value::as_str) != Some("web_search_call") {
            continue;
        }
        for group in [
            item.get("action")
                .and_then(|action| action.get("sources"))
                .and_then(Value::as_array),
            item.get("sources").and_then(Value::as_array),
            item.get("results").and_then(Value::as_array),
        ]
        .into_iter()
        .flatten()
        {
            for source in group {
                add_openai_search_result(
                    &mut results,
                    &mut seen,
                    source
                        .get("url")
                        .or_else(|| source.get("source_website_url"))
                        .and_then(Value::as_str),
                    source
                        .get("title")
                        .or_else(|| source.get("caption"))
                        .and_then(Value::as_str),
                    String::new(),
                );
            }
        }
    }

    results.truncate(num_results.min(20));
    results
}

fn add_openai_search_result(
    results: &mut Vec<SearchItem>,
    seen: &mut HashSet<String>,
    url: Option<&str>,
    title: Option<&str>,
    snippet: String,
) {
    let Some(url) = url.filter(|url| !url.trim().is_empty()) else {
        return;
    };
    let url = clean_openai_source_url(url);
    if !seen.insert(url.clone()) {
        return;
    }
    results.push(SearchItem {
        title: title
            .filter(|title| !title.trim().is_empty())
            .map(str::to_string)
            .or_else(|| Some(url.clone())),
        url: Some(url),
        snippet,
    });
}

fn clean_openai_source_url(raw_url: &str) -> String {
    Url::parse(raw_url)
        .map(|mut url| {
            let query = url
                .query_pairs()
                .filter(|(key, value)| !(key == "utm_source" && value == "openai"))
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect::<Vec<_>>();
            url.set_query(None);
            if !query.is_empty() {
                url.query_pairs_mut().extend_pairs(query);
            }
            url.to_string()
        })
        .unwrap_or_else(|_| raw_url.replace("?utm_source=openai", ""))
}

fn citation_snippet(text: &str, start: Option<u64>, end: Option<u64>) -> String {
    let (Some(start), Some(end)) = (start, end) else {
        return String::new();
    };
    let start = start as usize;
    let end = end.max(start as u64) as usize;
    let before = start.saturating_sub(100);
    let after = end.saturating_add(100);
    text.chars()
        .skip(before)
        .take(after.saturating_sub(before))
        .collect::<String>()
        .replace(['[', ']', '(', ')'], "")
        .trim()
        .chars()
        .take(300)
        .collect()
}
