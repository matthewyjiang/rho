use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    thread,
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use futures_util::StreamExt;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use url::Url;
use uuid::Uuid;

use crate::{
    credentials::{load_codex_tokens, CodexTokens, OsCredentialStore},
    tool::*,
};

const LARGE_REPO_THRESHOLD_KB: u64 = 350 * 1024;
const PREVIEW_BYTES: usize = 8_000;
const MAX_FETCH_BYTES: usize = 2 * 1024 * 1024;
const HTTP_TIMEOUT_SECS: u64 = 30;
const COMMAND_TIMEOUT_SECS: u64 = 60;
const OPENAI_RESPONSES_URL: &str = "https://api.openai.com/v1/responses";
const CODEX_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
const OPENAI_SEARCH_MODEL: &str = "gpt-4.1-mini";
const CODEX_SEARCH_MODEL: &str = "gpt-5.4";

static CONTENT_STORE: OnceLock<Mutex<HashMap<String, StoredContent>>> = OnceLock::new();

#[derive(Clone, Debug, Deserialize, Serialize)]
struct StoredContent {
    kind: String,
    items: Vec<StoredItem>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct StoredItem {
    url: Option<String>,
    query: Option<String>,
    title: Option<String>,
    content: String,
    metadata: Value,
}

pub struct WebSearch;
pub struct FetchContent;
pub struct GetSearchContent;

pub fn is_web_search_available() -> bool {
    resolve_openai_search_auth().is_ok()
        || std::env::var("BRAVE_SEARCH_API_KEY").is_ok()
        || std::env::var("BRAVE_API_KEY").is_ok()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebSearchArgs {
    query: Option<String>,
    queries: Option<Vec<String>>,
    num_results: Option<usize>,
    recency_filter: Option<String>,
    domain_filter: Option<Vec<String>>,
    provider: Option<String>,
    include_content: Option<bool>,
    workflow: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FetchContentArgs {
    url: Option<String>,
    urls: Option<Vec<String>>,
    prompt: Option<String>,
    timestamp: Option<String>,
    frames: Option<usize>,
    force_clone: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetSearchContentArgs {
    response_id: String,
    query: Option<String>,
    query_index: Option<usize>,
    url: Option<String>,
    url_index: Option<usize>,
}

#[async_trait::async_trait]
impl Tool for WebSearch {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "web_search".into(),
            description: "Search the web through a zero-config interface with optional provider credentials. Returns a concise summary, stored snippets by default, and a responseId; use get_search_content for stored snippets or fetched source content.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Single search query."},
                    "queries": {"type": "array", "items": {"type": "string"}, "description": "Batch of search queries."},
                    "numResults": {"type": "integer", "minimum": 1, "maximum": 20, "description": "Results per query."},
                    "recencyFilter": {"type": "string", "enum": ["day", "week", "month", "year"]},
                    "domainFilter": {"type": "array", "items": {"type": "string"}},
                    "provider": {"type": "string", "enum": ["auto", "openai", "brave", "parallel", "tavily", "exa", "perplexity", "gemini"]},
                    "includeContent": {"type": "boolean", "description": "Try to fetch and store result pages when the selected provider returns URLs."},
                    "workflow": {"type": "string", "enum": ["none", "summary-review", "auto-summary"]}
                },
                "anyOf": [{"required": ["query"]}, {"required": ["queries"]}]
            }),
        }
    }

    fn display_lines(&self, args: &Value, _ctx: &ToolContext, result: &ToolResult) -> Vec<String> {
        vec![web_search_display_line(args, result)]
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        let args: WebSearchArgs = serde_json::from_value(args)?;
        let queries = collect_queries(args.query, args.queries)?;
        let num_results = args.num_results.unwrap_or(5).clamp(1, 20);
        let provider = args.provider.unwrap_or_else(|| "auto".into());
        let workflow = args.workflow.unwrap_or_else(|| "summary-review".into());
        let include_content = args.include_content.unwrap_or(false);
        let response_id = new_response_id();
        let mut items = Vec::new();
        let mut summaries = Vec::new();

        for query in queries {
            let result = run_search_query(
                &query,
                num_results,
                &provider,
                args.recency_filter.as_deref(),
                args.domain_filter.as_deref(),
            )
            .await;
            match result {
                Ok(search_items) if !search_items.is_empty() => {
                    for (index, item) in search_items.into_iter().enumerate() {
                        let (content, content_kind) =
                            search_item_content(&item, include_content).await;
                        summaries.push(format!(
                            "{}. [{}] {}{}",
                            index + 1,
                            item.title.as_deref().unwrap_or("result"),
                            item.url.as_deref().unwrap_or("no url"),
                            item.snippet
                                .is_empty()
                                .then(String::new)
                                .unwrap_or_else(|| format!(" - {}", item.snippet))
                        ));
                        items.push(StoredItem {
                            url: item.url,
                            query: Some(query.clone()),
                            title: item.title,
                            content,
                            metadata: json!({"provider": provider, "workflow": workflow, "contentKind": content_kind}),
                        });
                    }
                }
                Ok(_) | Err(_) => {
                    let message = format!(
                        "No configured search provider returned live results for '{query}'. Set a provider API key or use fetch_content on known URLs."
                    );
                    summaries.push(message.clone());
                    items.push(StoredItem {
                        url: None,
                        query: Some(query),
                        title: Some("search unavailable".into()),
                        content: message,
                        metadata: json!({"provider": provider, "workflow": workflow, "status": "unavailable", "contentKind": "provider_unavailable"}),
                    });
                }
            }
        }

        let source_content_available = source_content_available(&items);
        let snippet_content_available = snippet_content_available(&items);
        let stored_content_available = !items.is_empty();
        store_content(
            response_id.clone(),
            StoredContent {
                kind: "web_search".into(),
                items,
            },
        );

        let content = json!({
            "responseId": response_id,
            "type": "web_search",
            "provider": provider,
            "workflow": workflow,
            "answer": summaries.join("\n"),
            "storedContentAvailable": stored_content_available,
            "snippetContentAvailable": snippet_content_available,
            "sourceContentAvailable": source_content_available,
            "fullContentAvailable": source_content_available,
            "note": "Tool output is intentionally concise. get_search_content returns stored snippets by default; fetched full page content is available only when includeContent succeeds for at least one result."
        });

        Ok(ToolResult {
            id,
            ok: true,
            content: truncate(to_pretty_json(&content), ctx.max_output_bytes),
        })
    }
}

#[async_trait::async_trait]
impl Tool for FetchContent {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "fetch_content".into(),
            description: "Fetch URLs, GitHub repos/files, YouTube/local videos, PDFs, local files, or web pages. Returns previews, artifacts, and responseId handles instead of dumping large content.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "URL or local path."},
                    "urls": {"type": "array", "items": {"type": "string"}},
                    "prompt": {"type": "string", "description": "Question for video or page analysis."},
                    "timestamp": {"type": "string", "description": "Video frame timestamp or range, e.g. 23:41 or 23:41-25:00."},
                    "frames": {"type": "integer", "minimum": 1, "maximum": 12},
                    "forceClone": {"type": "boolean", "description": "Clone GitHub repos even over the 350MB threshold."}
                },
                "anyOf": [{"required": ["url"]}, {"required": ["urls"]}]
            }),
        }
    }

    fn display_lines(&self, _args: &Value, _ctx: &ToolContext, result: &ToolResult) -> Vec<String> {
        vec![fetch_content_display_line(result)]
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        let args: FetchContentArgs = serde_json::from_value(args)?;
        let urls = collect_urls(args.url, args.urls)?;
        let frames = args.frames.unwrap_or(6).clamp(1, 12);
        let response_id = new_response_id();
        let mut items = Vec::new();
        let mut previews = Vec::new();

        for target in urls {
            let fetched = fetch_target(
                &target,
                &ctx,
                args.prompt.as_deref(),
                args.timestamp.as_deref(),
                frames,
                args.force_clone.unwrap_or(false),
            )
            .await?;
            previews.push(fetched.preview.clone());
            items.push(StoredItem {
                url: Some(target),
                query: args.prompt.clone(),
                title: fetched.title,
                content: fetched.content,
                metadata: fetched.metadata,
            });
        }

        store_content(
            response_id.clone(),
            StoredContent {
                kind: "fetch_content".into(),
                items,
            },
        );

        let content = json!({
            "responseId": response_id,
            "type": "fetch_content",
            "items": previews,
            "fullContentAvailable": true,
            "note": "Large fetched content is stored out-of-band. Use get_search_content with responseId to retrieve it."
        });

        Ok(ToolResult {
            id,
            ok: true,
            content: truncate(to_pretty_json(&content), ctx.max_output_bytes),
        })
    }
}

#[async_trait::async_trait]
impl Tool for GetSearchContent {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "get_search_content".into(),
            description: "Retrieve stored snippets, fetched source content, or fetch_content artifacts from a previous responseId by query, URL, or index.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "responseId": {"type": "string", "pattern": "^[0-9a-f]{32}$"},
                    "query": {"type": "string"},
                    "queryIndex": {"type": "integer", "minimum": 0},
                    "url": {"type": "string"},
                    "urlIndex": {"type": "integer", "minimum": 0}
                },
                "required": ["responseId"]
            }),
        }
    }

    fn display_lines(&self, _args: &Value, _ctx: &ToolContext, result: &ToolResult) -> Vec<String> {
        vec![get_search_content_display_line(result)]
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        let args: GetSearchContentArgs = serde_json::from_value(args)?;
        validate_response_id(&args.response_id)?;
        let store = content_store().lock().expect("content store lock poisoned");
        let stored = store
            .get(&args.response_id)
            .cloned()
            .map(Ok)
            .unwrap_or_else(|| read_stored_content(&args.response_id))?;
        let item = select_stored_item(&stored, &args)?;
        let content = json!({
            "responseId": args.response_id,
            "type": stored.kind,
            "title": item.title,
            "url": item.url,
            "query": item.query,
            "metadata": item.metadata,
            "content": item.content,
        });
        Ok(ToolResult {
            id,
            ok: true,
            content: truncate(to_pretty_json(&content), ctx.max_output_bytes),
        })
    }
}

fn web_search_display_line(args: &Value, result: &ToolResult) -> String {
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

fn fetch_content_display_line(result: &ToolResult) -> String {
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

fn get_search_content_display_line(result: &ToolResult) -> String {
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

struct SearchItem {
    title: Option<String>,
    url: Option<String>,
    snippet: String,
}

async fn search_item_content(item: &SearchItem, include_content: bool) -> (String, &'static str) {
    if !include_content {
        return (item.snippet.clone(), "snippet");
    }
    let Some(url) = item.url.as_deref() else {
        return (item.snippet.clone(), "snippet");
    };
    match fetch_url_text(url).await {
        Ok(content) => (content, "source_page"),
        Err(err) => {
            let warning = format!("content fetch failed for {url}: {err}");
            if item.snippet.is_empty() {
                (warning, "fetch_failed")
            } else {
                (
                    format!("{}\n\n{warning}", item.snippet),
                    "snippet_with_fetch_warning",
                )
            }
        }
    }
}

fn snippet_content_available(items: &[StoredItem]) -> bool {
    items.iter().any(|item| {
        matches!(
            item.metadata.get("contentKind").and_then(Value::as_str),
            Some("snippet") | Some("snippet_with_fetch_warning")
        )
    })
}

fn source_content_available(items: &[StoredItem]) -> bool {
    items
        .iter()
        .any(|item| item.metadata.get("contentKind").and_then(Value::as_str) == Some("source_page"))
}

struct FetchedTarget {
    title: Option<String>,
    content: String,
    preview: Value,
    metadata: Value,
}

async fn run_search_query(
    query: &str,
    num_results: usize,
    provider: &str,
    recency_filter: Option<&str>,
    domain_filter: Option<&[String]>,
) -> Result<Vec<SearchItem>, ToolError> {
    match provider {
        "auto" => match openai_search(query, num_results, recency_filter, domain_filter).await {
            Ok(results) => Ok(results),
            Err(_) => brave_search(query, num_results, recency_filter, domain_filter).await,
        },
        "openai" => openai_search(query, num_results, recency_filter, domain_filter).await,
        "brave" => brave_search(query, num_results, recency_filter, domain_filter).await,
        "parallel" | "tavily" | "exa" | "perplexity" | "gemini" => Err(ToolError::Message(
            format!("provider '{provider}' is not configured in this local MVP"),
        )),
        other => Err(ToolError::Message(format!(
            "unknown search provider: {other}"
        ))),
    }
}

#[derive(Clone, Debug)]
enum OpenAiSearchAuth {
    Codex(CodexTokens),
    ApiKey(String),
}

impl OpenAiSearchAuth {
    fn endpoint(&self) -> &'static str {
        match self {
            Self::Codex(_) => CODEX_RESPONSES_URL,
            Self::ApiKey(_) => OPENAI_RESPONSES_URL,
        }
    }

    fn model(&self) -> &'static str {
        match self {
            Self::Codex(_) => CODEX_SEARCH_MODEL,
            Self::ApiKey(_) => OPENAI_SEARCH_MODEL,
        }
    }

    fn bearer_token(&self) -> &str {
        match self {
            Self::Codex(tokens) => &tokens.access_token,
            Self::ApiKey(key) => key,
        }
    }
}

fn resolve_openai_search_auth() -> Result<OpenAiSearchAuth, ToolError> {
    if let Ok(access_token) = std::env::var("CODEX_ACCESS_TOKEN") {
        return Ok(OpenAiSearchAuth::Codex(CodexTokens {
            access_token,
            refresh_token: None,
            id_token: None,
            account_id: std::env::var("CODEX_ACCOUNT_ID").ok(),
        }));
    }
    if let Ok(Some(tokens)) = load_codex_tokens(&OsCredentialStore) {
        return Ok(OpenAiSearchAuth::Codex(tokens));
    }
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        return Ok(OpenAiSearchAuth::ApiKey(key));
    }
    Err(ToolError::Message(
        "OpenAI web search unavailable: sign in with /login openai-codex or set OPENAI_API_KEY"
            .into(),
    ))
}

async fn openai_search(
    query: &str,
    num_results: usize,
    recency_filter: Option<&str>,
    domain_filter: Option<&[String]>,
) -> Result<Vec<SearchItem>, ToolError> {
    let auth = resolve_openai_search_auth()?;
    let mut request = http_client()
        .post(auth.endpoint())
        .bearer_auth(auth.bearer_token())
        .header("Content-Type", "application/json");
    if let OpenAiSearchAuth::Codex(tokens) = &auth {
        request = request
            .header("User-Agent", "codex-cli")
            .header("originator", "codex_cli_rs");
        if let Some(account_id) = tokens.account_id.as_deref() {
            request = request.header("ChatGPT-Account-ID", account_id);
        }
    }

    let body = json!({
        "model": auth.model(),
        "instructions": openai_search_instructions(num_results, recency_filter, domain_filter),
        "input": [{"role": "user", "content": [{"type": "input_text", "text": query}]}],
        "tools": [openai_web_search_tool(domain_filter)],
        "include": ["web_search_call.action.sources"],
        "store": false,
        "stream": true,
        "tool_choice": "required",
        "parallel_tool_calls": true,
    });

    let response =
        request.json(&body).send().await.map_err(|err| {
            ToolError::Message(format!("OpenAI web search request failed: {err}"))
        })?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|err| ToolError::Message(format!("OpenAI web search response failed: {err}")))?;
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

fn openai_recency_label(recency_filter: &str) -> Option<&'static str> {
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
struct DomainFilters {
    allowed: Vec<String>,
    blocked: Vec<String>,
}

fn normalize_domain_filters(domain_filter: Option<&[String]>) -> DomainFilters {
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
    Regex::new(r"^[a-z0-9][a-z0-9.-]*\.[a-z]{2,}$")
        .ok()?
        .is_match(&input)
        .then_some(input)
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
    let mut seen = std::collections::HashSet::new();

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
    seen: &mut std::collections::HashSet<String>,
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
    let before = start.saturating_sub(100) as usize;
    let after = (end as usize).saturating_add(100).min(text.len());
    text.get(before..after)
        .unwrap_or_default()
        .replace(['[', ']', '(', ')'], "")
        .trim()
        .chars()
        .take(300)
        .collect()
}

async fn brave_search(
    query: &str,
    num_results: usize,
    recency_filter: Option<&str>,
    domain_filter: Option<&[String]>,
) -> Result<Vec<SearchItem>, ToolError> {
    let key = std::env::var("BRAVE_SEARCH_API_KEY")
        .or_else(|_| std::env::var("BRAVE_API_KEY"))
        .map_err(|_| ToolError::Message("BRAVE_SEARCH_API_KEY is not set".into()))?;
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

async fn fetch_target(
    target: &str,
    ctx: &ToolContext,
    prompt: Option<&str>,
    timestamp: Option<&str>,
    frames: usize,
    force_clone: bool,
) -> Result<FetchedTarget, ToolError> {
    if let Some(github) = parse_github_url(target) {
        return fetch_github_target(&github, force_clone).await;
    }

    if is_youtube_url(target) {
        let content = format!(
            "YouTube video analysis requires optional video extraction dependencies. prompt: {}; timestamp: {}; frames: {frames}",
            prompt.unwrap_or("none"),
            timestamp.unwrap_or("none")
        );
        return Ok(FetchedTarget {
            title: Some("youtube video".into()),
            content: content.clone(),
            preview: json!({"type": "youtube_video", "warning": content}),
            metadata: json!({"mode": "video_placeholder", "timestamp": timestamp, "frames": frames}),
        });
    }

    if let Ok(url) = Url::parse(target) {
        if content_type_from_path(url.path()) == "pdf" {
            return Ok(remote_pdf_fallback(target));
        }
        let content = fetch_url_text(url.as_str()).await?;
        let title = extract_title(&content);
        let markdown = html_to_text(&content);
        return Ok(FetchedTarget {
            title: title.clone(),
            content: markdown.clone(),
            preview: json!({
                "type": content_type_from_path(url.path()),
                "url": target,
                "title": title,
                "preview": truncate(markdown.clone(), PREVIEW_BYTES)
            }),
            metadata: json!({"mode": "http_fetch", "prompt": prompt}),
        });
    }

    fetch_local_path(target, &ctx.cwd, prompt, timestamp, frames)
}

async fn fetch_url_text(url: &str) -> Result<String, ToolError> {
    fetch_url_text_with_auth(url, None).await
}

async fn fetch_url_text_with_auth(
    url: &str,
    bearer_token: Option<&str>,
) -> Result<String, ToolError> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(ToolError::Message(
            "only http and https URLs are supported".into(),
        ));
    }
    let mut request = http_client()
        .get(url)
        .header("User-Agent", "rho-coding-agent");
    if let Some(token) = bearer_token {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .await
        .map_err(|err| ToolError::Message(format!("request failed: {err}")))?
        .error_for_status()
        .map_err(|err| ToolError::Message(format!("request failed: {err}")))?;
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| ToolError::Message(format!("request failed: {err}")))?;
        let remaining = MAX_FETCH_BYTES.saturating_sub(bytes.len());
        bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        if bytes.len() >= MAX_FETCH_BYTES {
            break;
        }
    }
    String::from_utf8(bytes).map_err(ToolError::Utf8)
}

fn fetch_local_path(
    target: &str,
    cwd: &Path,
    prompt: Option<&str>,
    timestamp: Option<&str>,
    frames: usize,
) -> Result<FetchedTarget, ToolError> {
    let path = resolve_path(cwd, target);
    let metadata = fs::metadata(&path)?;
    let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
    if is_video_extension(extension) {
        let content = format!(
            "Local video detected at {}. Visual analysis requires optional video extraction dependencies. prompt: {}; timestamp: {}; frames: {frames}",
            path.display(),
            prompt.unwrap_or("none"),
            timestamp.unwrap_or("none")
        );
        return Ok(FetchedTarget {
            title: path
                .file_name()
                .map(|name| name.to_string_lossy().to_string()),
            content: content.clone(),
            preview: json!({"type": "local_video", "path": path, "warning": content}),
            metadata: json!({"mode": "video_placeholder", "bytes": metadata.len()}),
        });
    }
    if extension.eq_ignore_ascii_case("pdf") {
        let content = format!(
            "PDF detected at {} ({} bytes). PDF text extraction is not available in this local MVP.",
            path.display(),
            metadata.len()
        );
        return Ok(FetchedTarget {
            title: path
                .file_name()
                .map(|name| name.to_string_lossy().to_string()),
            content: content.clone(),
            preview: json!({"type": "pdf", "path": path, "warning": content}),
            metadata: json!({"mode": "pdf_placeholder", "bytes": metadata.len()}),
        });
    }

    let content = fs::read_to_string(&path)?;
    Ok(FetchedTarget {
        title: path
            .file_name()
            .map(|name| name.to_string_lossy().to_string()),
        content: content.clone(),
        preview: json!({
            "type": "local_file",
            "path": path,
            "preview": truncate(content, PREVIEW_BYTES)
        }),
        metadata: json!({"mode": "local_file", "bytes": metadata.len()}),
    })
}

async fn fetch_github_target(
    github: &GitHubTarget,
    force_clone: bool,
) -> Result<FetchedTarget, ToolError> {
    if github.kind == GitHubKind::Commit {
        return github_api_fallback(github, None).await;
    }

    let repo_api = format!(
        "https://api.github.com/repos/{}/{}",
        github.owner, github.repo
    );
    let repo_size_kb = github_api_json(&repo_api)
        .await
        .ok()
        .and_then(|value| value.get("size").and_then(Value::as_u64));
    let oversized = repo_size_kb.is_some_and(|size| size > LARGE_REPO_THRESHOLD_KB);
    if oversized && !force_clone {
        return github_api_fallback(github, repo_size_kb).await;
    }

    match ensure_github_clone(github).await {
        Ok(local_path) => read_github_clone(github, &local_path),
        Err(_) => github_api_fallback(github, repo_size_kb).await,
    }
}

async fn github_api_fallback(
    github: &GitHubTarget,
    repo_size_kb: Option<u64>,
) -> Result<FetchedTarget, ToolError> {
    let api_url = github_api_content_url(github);
    let content = match github.kind {
        GitHubKind::Blob => github_api_file_content(&api_url).await?,
        GitHubKind::Root | GitHubKind::Tree | GitHubKind::Commit => {
            to_pretty_json(&github_api_json(&api_url).await?)
        }
    };
    Ok(FetchedTarget {
        title: Some(format!("{}/{}", github.owner, github.repo)),
        preview: json!({
            "type": "github_api_fallback",
            "repo": format!("{}/{}", github.owner, github.repo),
            "reason": repo_size_kb.map(|size| format!("repo size {size}KB exceeds 350MB threshold")).unwrap_or_else(|| "clone unavailable".into()),
            "canForceClone": true,
            "preview": truncate(content.clone(), PREVIEW_BYTES)
        }),
        content,
        metadata: json!({"mode": "api_fallback", "repoSizeKb": repo_size_kb}),
    })
}

fn github_api_content_url(github: &GitHubTarget) -> String {
    match github.kind {
        GitHubKind::Root | GitHubKind::Tree | GitHubKind::Blob => format!(
            "https://api.github.com/repos/{}/{}/contents/{}{}",
            github.owner,
            github.repo,
            github.path,
            github
                .ref_name
                .as_ref()
                .map(|ref_name| format!("?ref={ref_name}"))
                .unwrap_or_default()
        ),
        GitHubKind::Commit => format!(
            "https://api.github.com/repos/{}/{}/commits/{}",
            github.owner,
            github.repo,
            github.ref_name.as_deref().unwrap_or("HEAD")
        ),
    }
}

async fn github_api_file_content(url: &str) -> Result<String, ToolError> {
    let value = github_api_json(url).await?;
    let encoding = value.get("encoding").and_then(Value::as_str);
    let content = value
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ToolError::Message("GitHub API response did not include file content".into())
        })?;
    if encoding == Some("base64") {
        let compact = content.lines().collect::<String>();
        let bytes = BASE64_STANDARD.decode(compact).map_err(|err| {
            ToolError::Message(format!("GitHub file content was not base64: {err}"))
        })?;
        return String::from_utf8(bytes).map_err(ToolError::Utf8);
    }
    Ok(content.to_string())
}

async fn github_api_json(url: &str) -> Result<Value, ToolError> {
    let mut request = http_client()
        .get(url)
        .header("User-Agent", "rho-coding-agent");
    if let Ok(token) = github_token() {
        request = request.bearer_auth(token);
    }
    request
        .send()
        .await
        .map_err(|err| ToolError::Message(format!("GitHub API request failed: {err}")))?
        .error_for_status()
        .map_err(|err| ToolError::Message(format!("GitHub API request failed: {err}")))?
        .json()
        .await
        .map_err(|err| ToolError::Message(format!("GitHub API response was not JSON: {err}")))
}

fn github_token() -> Result<String, std::env::VarError> {
    std::env::var("GITHUB_TOKEN").or_else(|_| std::env::var("GH_TOKEN"))
}

fn run_command_with_timeout(command: &mut Command, description: &str) -> Result<(), ToolError> {
    command.stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = command.spawn()?;
    let deadline = Instant::now() + Duration::from_secs(COMMAND_TIMEOUT_SECS);
    loop {
        if let Some(status) = child.try_wait()? {
            if status.success() {
                return Ok(());
            }
            return Err(ToolError::Message(format!(
                "{description} failed with status {status}"
            )));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ToolError::Message(format!(
                "{description} timed out after {COMMAND_TIMEOUT_SECS}s"
            )));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

async fn ensure_github_clone(github: &GitHubTarget) -> Result<PathBuf, ToolError> {
    let cache_root = web_access_cache_root()
        .join(std::process::id().to_string())
        .join("github")
        .join(safe_path_component(&github.owner))
        .join(safe_path_component(&github.repo));
    let ref_key = github.ref_name.as_deref().unwrap_or("HEAD");
    let local_path = cache_root.join(safe_path_component(ref_key));
    create_private_dir_all(&cache_root)?;
    if local_path.join(".git").is_dir() {
        checkout_github_ref(github, &local_path)?;
        return Ok(local_path);
    }

    let repo_slug = format!("{}/{}", github.owner, github.repo);
    let clone_url = format!("https://github.com/{repo_slug}.git");
    let mut command = if let Ok(token) = github_token() {
        let mut command = Command::new("gh");
        command
            .arg("repo")
            .arg("clone")
            .arg(&repo_slug)
            .arg(&local_path)
            .arg("--")
            .arg("--depth")
            .arg("1");
        if std::env::var_os("GH_TOKEN").is_none() {
            command.env("GH_TOKEN", token);
        }
        command
    } else {
        let mut command = Command::new("git");
        command
            .arg("clone")
            .arg("--depth")
            .arg("1")
            .arg(clone_url)
            .arg(&local_path);
        command
    };
    run_command_with_timeout(
        &mut command,
        &format!("git clone for {}/{}", github.owner, github.repo),
    )?;
    checkout_github_ref(github, &local_path)?;
    Ok(local_path)
}

fn checkout_github_ref(github: &GitHubTarget, local_path: &Path) -> Result<(), ToolError> {
    let Some(ref_name) = github.ref_name.as_deref() else {
        return Ok(());
    };
    let mut fetch = Command::new("git");
    fetch
        .arg("-C")
        .arg(local_path)
        .arg("fetch")
        .arg("--depth")
        .arg("1")
        .arg("origin")
        .arg(ref_name);
    run_command_with_timeout(
        &mut fetch,
        &format!(
            "git fetch for {}/{} ref {ref_name}",
            github.owner, github.repo
        ),
    )?;
    let mut checkout = Command::new("git");
    checkout
        .arg("-C")
        .arg(local_path)
        .arg("checkout")
        .arg("--detach")
        .arg("FETCH_HEAD");
    run_command_with_timeout(
        &mut checkout,
        &format!(
            "git checkout for {}/{} ref {ref_name}",
            github.owner, github.repo
        ),
    )?;
    Ok(())
}

fn read_github_clone(github: &GitHubTarget, local_path: &Path) -> Result<FetchedTarget, ToolError> {
    let target_path = local_path.join(&github.path);
    let commit = Command::new("git")
        .arg("-C")
        .arg(local_path)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string());

    match github.kind {
        GitHubKind::Root | GitHubKind::Tree => {
            let dir = if github.kind == GitHubKind::Root {
                local_path
            } else {
                &target_path
            };
            let tree = directory_listing(dir)?;
            let readme = find_readme(dir).and_then(|path| fs::read_to_string(path).ok());
            let content = format!(
                "localPath: {}\ncommit: {}\n\n{}{}",
                local_path.display(),
                commit.clone().unwrap_or_else(|| "unknown".into()),
                tree,
                readme
                    .as_ref()
                    .map(|readme| format!("\n\nREADME:\n{readme}"))
                    .unwrap_or_default()
            );
            Ok(FetchedTarget {
                title: Some(format!("{}/{}", github.owner, github.repo)),
                preview: json!({
                    "type": "github_repo",
                    "localPath": local_path,
                    "commit": commit,
                    "tree": tree,
                    "readmePreview": readme.map(|readme| truncate(readme, PREVIEW_BYTES))
                }),
                content,
                metadata: json!({"mode": "clone", "localPath": local_path, "commit": commit}),
            })
        }
        GitHubKind::Blob => {
            let content = fs::read_to_string(&target_path)?;
            Ok(FetchedTarget {
                title: Some(github.path.clone()),
                preview: json!({
                    "type": "github_file",
                    "localPath": target_path,
                    "commit": commit,
                    "preview": truncate(content.clone(), PREVIEW_BYTES)
                }),
                content,
                metadata: json!({"mode": "clone", "localPath": local_path, "commit": commit}),
            })
        }
        GitHubKind::Commit => github_api_fallback_sync_notice(github, local_path, commit),
    }
}

fn github_api_fallback_sync_notice(
    github: &GitHubTarget,
    local_path: &Path,
    commit: Option<String>,
) -> Result<FetchedTarget, ToolError> {
    let content = format!(
        "Commit URLs are handled via GitHub API in fetch_content. Clone is available at {} with HEAD {}.",
        local_path.display(),
        commit.as_deref().unwrap_or("unknown")
    );
    Ok(FetchedTarget {
        title: Some(format!("{}/{} commit", github.owner, github.repo)),
        preview: json!({"type": "github_commit", "warning": content}),
        content,
        metadata: json!({"mode": "commit_notice", "localPath": local_path, "commit": commit}),
    })
}

fn directory_listing(path: &Path) -> Result<String, ToolError> {
    let mut entries = fs::read_dir(path)?
        .map(|entry| {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let suffix = if file_type.is_dir() { "/" } else { "" };
            Ok(format!("{}{}", entry.file_name().to_string_lossy(), suffix))
        })
        .collect::<Result<Vec<_>, std::io::Error>>()?;
    entries.sort();
    Ok(entries.join("\n"))
}

fn find_readme(path: &Path) -> Option<PathBuf> {
    ["README.md", "README.txt", "README"]
        .into_iter()
        .map(|name| path.join(name))
        .find(|path| path.is_file())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GitHubKind {
    Root,
    Tree,
    Blob,
    Commit,
}

#[derive(Debug)]
struct GitHubTarget {
    owner: String,
    repo: String,
    kind: GitHubKind,
    ref_name: Option<String>,
    path: String,
}

fn parse_github_url(input: &str) -> Option<GitHubTarget> {
    let url = Url::parse(input).ok()?;
    if url.host_str()? != "github.com" {
        return None;
    }
    let segments = url.path_segments()?.collect::<Vec<_>>();
    if segments.len() < 2 {
        return None;
    }
    let owner = segments[0].to_string();
    let repo = segments[1].trim_end_matches(".git").to_string();
    match segments.get(2).copied() {
        None | Some("") => Some(GitHubTarget {
            owner,
            repo,
            kind: GitHubKind::Root,
            ref_name: None,
            path: String::new(),
        }),
        Some("tree") | Some("blob") => {
            let kind = if segments[2] == "tree" {
                GitHubKind::Tree
            } else {
                GitHubKind::Blob
            };
            let (ref_name, path) = split_github_ref_and_path(kind, &segments[3..]);
            Some(GitHubTarget {
                owner,
                repo,
                kind,
                ref_name,
                path,
            })
        }
        Some("commit") => Some(GitHubTarget {
            owner,
            repo,
            kind: GitHubKind::Commit,
            ref_name: segments.get(3).map(|value| (*value).to_string()),
            path: String::new(),
        }),
        _ => None,
    }
}

fn split_github_ref_and_path(_kind: GitHubKind, segments: &[&str]) -> (Option<String>, String) {
    if segments.is_empty() {
        return (None, String::new());
    }
    if segments.len() == 1 {
        return (Some(segments[0].to_string()), String::new());
    }

    let split_at = find_github_path_start(segments).unwrap_or(1);
    (
        Some(segments[..split_at].join("/")),
        segments[split_at..].join("/"),
    )
}

fn find_github_path_start(segments: &[&str]) -> Option<usize> {
    (1..segments.len()).find(|index| is_common_github_path_start(segments[*index]))
}

fn is_common_github_path_start(segment: &str) -> bool {
    matches!(
        segment,
        "src"
            | "docs"
            | "doc"
            | "test"
            | "tests"
            | "crates"
            | "packages"
            | "package"
            | "examples"
            | "example"
            | "scripts"
            | "script"
            | "tools"
            | "tool"
            | "app"
            | "apps"
            | "lib"
            | "libs"
            | "cmd"
            | "components"
            | "component"
            | "internal"
            | "pkg"
            | ".github"
    )
}

fn select_stored_item<'a>(
    stored: &'a StoredContent,
    args: &GetSearchContentArgs,
) -> Result<&'a StoredItem, ToolError> {
    if let Some(url) = &args.url {
        return stored
            .items
            .iter()
            .find(|item| item.url.as_deref() == Some(url.as_str()))
            .ok_or_else(|| ToolError::Message(format!("url not found for responseId: {url}")));
    }
    if let Some(index) = args.url_index {
        return stored
            .items
            .get(index)
            .ok_or_else(|| ToolError::Message(format!("urlIndex out of range: {index}")));
    }
    if let Some(query) = &args.query {
        return stored
            .items
            .iter()
            .find(|item| item.query.as_deref() == Some(query.as_str()))
            .ok_or_else(|| ToolError::Message(format!("query not found for responseId: {query}")));
    }
    if let Some(index) = args.query_index {
        return stored
            .items
            .iter()
            .filter(|item| item.query.is_some())
            .nth(index)
            .ok_or_else(|| ToolError::Message(format!("queryIndex out of range: {index}")));
    }
    stored
        .items
        .first()
        .ok_or_else(|| ToolError::Message("responseId has no stored content".into()))
}

fn collect_queries(
    query: Option<String>,
    queries: Option<Vec<String>>,
) -> Result<Vec<String>, ToolError> {
    let mut values = Vec::new();
    if let Some(query) = query {
        values.push(query);
    }
    if let Some(queries) = queries {
        values.extend(queries);
    }
    let values = values
        .into_iter()
        .map(|query| query.trim().to_string())
        .filter(|query| !query.is_empty())
        .collect::<Vec<_>>();
    if values.is_empty() {
        return Err(ToolError::Message(
            "query or queries must include at least one value".into(),
        ));
    }
    Ok(values)
}

fn collect_urls(url: Option<String>, urls: Option<Vec<String>>) -> Result<Vec<String>, ToolError> {
    let mut values = Vec::new();
    if let Some(url) = url {
        values.push(url);
    }
    if let Some(urls) = urls {
        values.extend(urls);
    }
    let values = values
        .into_iter()
        .map(|url| url.trim().to_string())
        .filter(|url| !url.is_empty())
        .collect::<Vec<_>>();
    if values.is_empty() {
        return Err(ToolError::Message(
            "url or urls must include at least one value".into(),
        ));
    }
    Ok(values)
}

fn store_content(response_id: String, content: StoredContent) {
    let _ = write_stored_content(&response_id, &content);
    content_store()
        .lock()
        .expect("content store lock poisoned")
        .insert(response_id, content);
}

fn write_stored_content(response_id: &str, content: &StoredContent) -> Result<(), ToolError> {
    let path = stored_content_path(response_id)?;
    if let Some(parent) = path.parent() {
        create_private_dir_all(parent)?;
    }
    let serialized = serde_json::to_string(content)
        .map_err(|err| ToolError::Message(format!("failed to serialize stored content: {err}")))?;
    write_private_file(&path, serialized.as_bytes())?;
    Ok(())
}

fn read_stored_content(response_id: &str) -> Result<StoredContent, ToolError> {
    let path = stored_content_path(response_id)?;
    let content = fs::read_to_string(&path).map_err(|_| {
        ToolError::Message(format!(
            "unknown responseId: {response_id}. Stored web content is available only while its cache file exists."
        ))
    })?;
    serde_json::from_str(&content)
        .map_err(|err| ToolError::Message(format!("stored content was not valid JSON: {err}")))
}

fn stored_content_path(response_id: &str) -> Result<PathBuf, ToolError> {
    validate_response_id(response_id)?;
    Ok(web_access_cache_root()
        .join("content")
        .join(format!("{response_id}.json")))
}

fn web_access_cache_root() -> PathBuf {
    std::env::temp_dir().join("rho-web-access")
}

fn create_private_dir_all(path: &Path) -> Result<(), ToolError> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let root = web_access_cache_root();
        if root.exists() {
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        }
        if path.exists() {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

fn write_private_file(path: &Path, contents: &[u8]) -> Result<(), ToolError> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(contents)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        fs::write(path, contents)?;
        Ok(())
    }
}

fn validate_response_id(response_id: &str) -> Result<(), ToolError> {
    let valid = response_id.len() == 32
        && response_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if valid {
        Ok(())
    } else {
        Err(ToolError::Message(
            "invalid responseId: expected 32 lowercase hexadecimal characters".into(),
        ))
    }
}

fn content_store() -> &'static Mutex<HashMap<String, StoredContent>> {
    CONTENT_STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn new_response_id() -> String {
    Uuid::new_v4().simple().to_string()
}

fn to_pretty_json(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
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

fn remote_pdf_fallback(url: &str) -> FetchedTarget {
    let content = format!(
        "Remote PDF detected at {url}. PDF text extraction is not available in this local MVP."
    );
    FetchedTarget {
        title: Some("remote pdf".into()),
        content: content.clone(),
        preview: json!({"type": "pdf", "url": url, "warning": content}),
        metadata: json!({"mode": "pdf_placeholder"}),
    }
}

fn safe_path_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn html_to_text(content: &str) -> String {
    let without_scripts = Regex::new(r"(?is)<script[^>]*>.*?</script>")
        .unwrap()
        .replace_all(content, "");
    let without_scripts = Regex::new(r"(?is)<style[^>]*>.*?</style>")
        .unwrap()
        .replace_all(&without_scripts, "");
    let with_breaks = Regex::new(r"(?i)</?(p|br|div|section|article|h[1-6]|li)[^>]*>")
        .unwrap()
        .replace_all(&without_scripts, "\n");
    let without_tags = Regex::new(r"(?s)<[^>]+>")
        .unwrap()
        .replace_all(&with_breaks, "");
    without_tags
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn extract_title(content: &str) -> Option<String> {
    Regex::new(r"(?is)<title[^>]*>(.*?)</title>")
        .ok()?
        .captures(content)?
        .get(1)
        .map(|capture| html_to_text(capture.as_str()))
}

fn is_youtube_url(target: &str) -> bool {
    Url::parse(target)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .is_some_and(|host| {
            host == "youtu.be" || host.ends_with(".youtube.com") || host == "youtube.com"
        })
}

fn content_type_from_path(path: &str) -> &'static str {
    if path.ends_with(".pdf") {
        "pdf"
    } else {
        "webpage"
    }
}

fn is_video_extension(extension: &str) -> bool {
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "mp4" | "mov" | "webm" | "mkv" | "avi" | "m4v"
    )
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    use super::*;

    fn test_context() -> ToolContext {
        ToolContext {
            cwd: tempfile::tempdir().unwrap().keep(),
            max_output_bytes: 12000,
        }
    }

    #[test]
    fn parses_github_root_tree_blob_and_commit_urls() {
        let root = parse_github_url("https://github.com/owner/repo").unwrap();
        assert_eq!(root.owner, "owner");
        assert_eq!(root.repo, "repo");
        assert_eq!(root.kind, GitHubKind::Root);

        let tree = parse_github_url("https://github.com/owner/repo/tree/main/src/tools").unwrap();
        assert_eq!(tree.kind, GitHubKind::Tree);
        assert_eq!(tree.ref_name.as_deref(), Some("main"));
        assert_eq!(tree.path, "src/tools");

        let tree_without_common_root =
            parse_github_url("https://github.com/owner/repo/tree/main/benches").unwrap();
        assert_eq!(tree_without_common_root.kind, GitHubKind::Tree);
        assert_eq!(tree_without_common_root.ref_name.as_deref(), Some("main"));
        assert_eq!(tree_without_common_root.path, "benches");

        let slashed_ref =
            parse_github_url("https://github.com/owner/repo/tree/feature/foo/src/tools").unwrap();
        assert_eq!(slashed_ref.kind, GitHubKind::Tree);
        assert_eq!(slashed_ref.ref_name.as_deref(), Some("feature/foo"));
        assert_eq!(slashed_ref.path, "src/tools");

        let component_path =
            parse_github_url("https://github.com/owner/repo/tree/feature/foo/components/Button")
                .unwrap();
        assert_eq!(component_path.kind, GitHubKind::Tree);
        assert_eq!(component_path.ref_name.as_deref(), Some("feature/foo"));
        assert_eq!(component_path.path, "components/Button");

        let blob = parse_github_url("https://github.com/owner/repo/blob/main/README.md").unwrap();
        assert_eq!(blob.kind, GitHubKind::Blob);
        assert_eq!(blob.path, "README.md");

        let nested_blob =
            parse_github_url("https://github.com/owner/repo/blob/main/foo/bar.rs").unwrap();
        assert_eq!(nested_blob.kind, GitHubKind::Blob);
        assert_eq!(nested_blob.ref_name.as_deref(), Some("main"));
        assert_eq!(nested_blob.path, "foo/bar.rs");

        let commit = parse_github_url("https://github.com/owner/repo/commit/abc123").unwrap();
        assert_eq!(commit.kind, GitHubKind::Commit);
        assert_eq!(commit.ref_name.as_deref(), Some("abc123"));
    }

    #[tokio::test]
    async fn web_search_stores_stub_content_when_provider_is_unavailable() {
        let args = json!({"query": "rho web access", "provider": "tavily", "includeContent": true});
        let ctx = test_context();
        let result = WebSearch
            .call(args.clone(), ctx.clone(), "call_1".into())
            .await
            .unwrap();
        let value: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(value["fullContentAvailable"], false);
        assert_eq!(value["sourceContentAvailable"], false);
        assert_eq!(value["storedContentAvailable"], true);
        let response_id = value.get("responseId").unwrap().as_str().unwrap();

        let display_lines = WebSearch.display_lines(&args, &ctx, &result);
        assert_eq!(display_lines.len(), 1);
        assert_eq!(
            display_lines,
            vec!["web search: no live results for \"rho web access\""]
        );

        let retrieved = GetSearchContent
            .call(
                json!({"responseId": response_id, "queryIndex": 0}),
                test_context(),
                "call_2".into(),
            )
            .await
            .unwrap();

        assert!(retrieved.content.contains("No configured search provider"));
    }

    #[tokio::test]
    async fn search_item_content_preserves_snippet_when_fetch_fails() {
        let item = SearchItem {
            title: Some("example".into()),
            url: Some("ftp://example.com/article".into()),
            snippet: "original snippet".into(),
        };

        let (content, content_kind) = search_item_content(&item, true).await;

        assert_eq!(content_kind, "snippet_with_fetch_warning");
        assert!(content.contains("original snippet"));
        assert!(content.contains("content fetch failed"));
    }

    #[test]
    fn content_availability_matches_stored_content_kind() {
        let items = vec![
            StoredItem {
                url: Some("https://example.com".into()),
                query: Some("example".into()),
                title: Some("failed".into()),
                content: "content fetch failed".into(),
                metadata: json!({"contentKind": "fetch_failed"}),
            },
            StoredItem {
                url: Some("https://example.net".into()),
                query: Some("example".into()),
                title: Some("snippet preserved".into()),
                content: "original snippet\n\ncontent fetch failed".into(),
                metadata: json!({"contentKind": "snippet_with_fetch_warning"}),
            },
            StoredItem {
                url: Some("https://example.org".into()),
                query: Some("example".into()),
                title: Some("source".into()),
                content: "source page".into(),
                metadata: json!({"contentKind": "source_page"}),
            },
        ];

        assert!(source_content_available(&items));
        assert!(!source_content_available(&items[..2]));
        assert!(snippet_content_available(&items));
        assert!(!snippet_content_available(&items[..1]));
    }

    #[tokio::test]
    async fn fetch_content_stores_local_file_content() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("note.txt"), "hello from local file").unwrap();
        let ctx = ToolContext {
            cwd: dir.path().to_path_buf(),
            max_output_bytes: 12000,
        };

        let args = json!({"url": "note.txt"});
        let result = FetchContent
            .call(args.clone(), ctx.clone(), "call_1".into())
            .await
            .unwrap();
        let value: Value = serde_json::from_str(&result.content).unwrap();
        let response_id = value.get("responseId").unwrap().as_str().unwrap();

        let display_lines = FetchContent.display_lines(&args, &ctx, &result);
        assert_eq!(display_lines.len(), 1);
        assert_eq!(display_lines, vec!["fetch content: fetched 1 item"]);

        let get_args = json!({"responseId": response_id, "urlIndex": 0});
        let retrieved = GetSearchContent
            .call(get_args.clone(), ctx.clone(), "call_2".into())
            .await
            .unwrap();

        let retrieved_display_lines = GetSearchContent.display_lines(&get_args, &ctx, &retrieved);
        assert_eq!(retrieved_display_lines, vec!["retrieved content: note.txt"]);
        assert!(retrieved.content.contains("hello from local file"));
    }

    #[tokio::test]
    async fn fetch_content_reads_local_http_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 512];
            let _ = stream.read(&mut request).unwrap();
            let body = "<html><title>Local Test</title><p>Hello from HTTP</p></html>";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        });

        let result = FetchContent
            .call(
                json!({"url": format!("http://{addr}/article")}),
                test_context(),
                "call_1".into(),
            )
            .await
            .unwrap();
        server.join().unwrap();

        assert!(result.content.contains("Hello from HTTP"));
        assert!(result.content.contains("Local Test"));
    }

    #[tokio::test]
    async fn get_search_content_rejects_invalid_response_id() {
        let err = GetSearchContent
            .call(
                json!({"responseId": "../00000000000000000000000000000000"}),
                test_context(),
                "call_1".into(),
            )
            .await
            .unwrap_err();

        assert_eq!(
            err.to_string(),
            "invalid responseId: expected 32 lowercase hexadecimal characters"
        );
    }

    #[test]
    fn applies_search_filters_to_brave_query_parameters() {
        let filtered = apply_domain_filter(
            "rust async",
            Some(&["github.com".to_string(), "-example.com".to_string()]),
        );

        assert_eq!(filtered, "rust async site:github.com -site:example.com");
        assert_eq!(brave_freshness(Some("week")), Some("pw"));
    }

    #[test]
    fn builds_openai_web_search_filters() {
        let tool = openai_web_search_tool(Some(&[
            "https://github.com/rust-lang".to_string(),
            "-example.com/path".to_string(),
        ]));

        assert_eq!(tool["type"], "web_search");
        assert_eq!(tool["filters"]["allowed_domains"], json!(["github.com"]));
        assert_eq!(tool["filters"]["blocked_domains"], json!(["example.com"]));
    }

    #[test]
    fn extracts_openai_web_search_sources() {
        let output = vec![json!({
            "type": "message",
            "content": [{
                "text": "Rust 1.90 is available from the Rust blog.",
                "annotations": [{
                    "type": "url_citation",
                    "url": "https://blog.rust-lang.org/?utm_source=openai&ref=keep",
                    "title": "Rust Blog",
                    "start_index": 0,
                    "end_index": 9
                }]
            }]
        })];

        let results = extract_openai_search_results(&output, 5);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title.as_deref(), Some("Rust Blog"));
        assert_eq!(
            results[0].url.as_deref(),
            Some("https://blog.rust-lang.org/?ref=keep")
        );
        assert!(results[0].snippet.contains("Rust 1.90"));
    }

    #[test]
    fn safe_path_component_avoids_repo_cache_collisions() {
        let first = PathBuf::from(safe_path_component("foo")).join(safe_path_component("bar-baz"));
        let second = PathBuf::from(safe_path_component("foo-bar")).join(safe_path_component("baz"));

        assert_ne!(first, second);
    }

    #[test]
    fn remote_pdf_returns_warning_without_text_fetch() {
        let fetched = remote_pdf_fallback("https://example.com/file.pdf");

        assert_eq!(fetched.metadata["mode"], "pdf_placeholder");
        assert!(fetched.content.contains("Remote PDF detected"));
    }

    #[tokio::test]
    async fn stored_content_can_be_reloaded_from_disk() {
        let response_id = new_response_id();
        let stored = StoredContent {
            kind: "fetch_content".into(),
            items: vec![StoredItem {
                url: Some("file.txt".into()),
                query: None,
                title: Some("file".into()),
                content: "persisted content".into(),
                metadata: json!({"mode": "test"}),
            }],
        };
        write_stored_content(&response_id, &stored).unwrap();
        content_store()
            .lock()
            .expect("content store lock poisoned")
            .remove(&response_id);

        let retrieved = GetSearchContent
            .call(
                json!({"responseId": response_id, "urlIndex": 0}),
                test_context(),
                "call_1".into(),
            )
            .await
            .unwrap();

        assert!(retrieved.content.contains("persisted content"));
    }

    #[cfg(unix)]
    #[test]
    fn persisted_content_uses_private_unix_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let response_id = new_response_id();
        let stored = StoredContent {
            kind: "fetch_content".into(),
            items: vec![StoredItem {
                url: Some("file.txt".into()),
                query: None,
                title: Some("file".into()),
                content: "private content".into(),
                metadata: json!({"mode": "test"}),
            }],
        };

        write_stored_content(&response_id, &stored).unwrap();
        let path = stored_content_path(&response_id).unwrap();
        let parent = path.parent().unwrap();
        let file_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let dir_mode = fs::metadata(parent).unwrap().permissions().mode() & 0o777;

        assert_eq!(file_mode, 0o600);
        assert_eq!(dir_mode, 0o700);
    }

    #[tokio::test]
    async fn rejects_empty_web_search_query() {
        let err = WebSearch
            .call(json!({"query": "   "}), test_context(), "call_1".into())
            .await
            .unwrap_err();

        assert_eq!(
            err.to_string(),
            "query or queries must include at least one value"
        );
    }

    #[test]
    fn html_to_text_removes_scripts_and_tags() {
        let text = html_to_text("<title>Hello</title><script>bad()</script><p>Visible</p>");

        assert!(text.contains("Hello"));
        assert!(text.contains("Visible"));
        assert!(!text.contains("bad()"));
    }

    #[test]
    fn tool_specs_use_requested_names() {
        assert_eq!(WebSearch.spec().name, "web_search");
        assert_eq!(FetchContent.spec().name, "fetch_content");
        assert_eq!(GetSearchContent.spec().name, "get_search_content");
    }
}
