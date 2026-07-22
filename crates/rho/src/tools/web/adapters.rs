use serde::Deserialize;
use serde_json::{json, Value};

use {
    crate::config::{Config, SearchProvider},
    rho_tools::tool::{truncate, Tool, ToolContext, ToolError, ToolResult, ToolSpec},
};

use super::{
    search::{self, SearchBackendConfig},
    storage::{self, StoredContent, StoredItem},
    util::to_pretty_json,
};

pub struct WebSearch {
    config: SearchBackendConfig,
    client: reqwest::Client,
    access: super::guard::NetworkAccess,
}

pub struct GetSearchContent;

impl WebSearch {
    pub(super) fn with_client(
        config: &Config,
        client: reqwest::Client,
        access: super::guard::NetworkAccess,
    ) -> Self {
        Self {
            config: SearchBackendConfig::from_config(config),
            client,
            access,
        }
    }

    pub fn is_available(&self) -> bool {
        match self.config.provider {
            SearchProvider::Disabled => false,
            SearchProvider::OpenAi => search::openai_available(&self.config),
            SearchProvider::Brave => search::brave_available(&self.config),
            SearchProvider::Auto
            | SearchProvider::Exa
            | SearchProvider::Parallel
            | SearchProvider::Tavily
            | SearchProvider::Perplexity
            | SearchProvider::Gemini => true,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebSearchArgs {
    query: Option<String>,
    queries: Option<Vec<String>>,
    num_results: Option<usize>,
    recency_filter: Option<String>,
    domain_filter: Option<Vec<String>>,
    provider: Option<SearchProvider>,
    include_content: Option<bool>,
    workflow: Option<String>,
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
                    "queries": {"type": "array", "items": {"type": "string"}, "description": "Search queries. Use one item for a single search, or multiple items for broader research."},
                    "numResults": {"type": "integer", "minimum": 1, "maximum": 20, "description": "Results per query."},
                    "recencyFilter": {"type": "string", "enum": ["day", "week", "month", "year"]},
                    "domainFilter": {"type": "array", "items": {"type": "string"}},
                    "provider": {"type": "string", "enum": ["auto", "openai", "brave", "parallel", "tavily", "exa", "perplexity", "gemini"]},
                    "includeContent": {"type": "boolean", "description": "Try to fetch and store result pages when the selected provider returns URLs."},
                    "workflow": {"type": "string", "enum": ["none", "summary-review", "auto-summary"]}
                },
                "required": ["queries"]
            }),
        }
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        let args: WebSearchArgs = serde_json::from_value(args)?;
        let queries = collect_values(args.query, args.queries, "query", "queries")?;
        let num_results = args.num_results.unwrap_or(5).clamp(1, 20);
        let provider = args.provider.unwrap_or(self.config.provider);
        let workflow = args.workflow.unwrap_or_else(|| "summary-review".into());
        let include_content = args.include_content.unwrap_or(false);
        let response_id = storage::new_response_id();
        let mut items = Vec::new();
        let mut summaries = Vec::new();

        for query in queries {
            let result = search::run_search_query(
                &self.client,
                &query,
                num_results,
                provider,
                args.recency_filter.as_deref(),
                args.domain_filter.as_deref(),
                &self.config,
            )
            .await;
            match result {
                Ok(search_items) if !search_items.is_empty() => {
                    for (index, item) in search_items.into_iter().enumerate() {
                        let (content, content_kind) =
                            search::item_content(&self.client, &item, include_content, self.access)
                                .await;
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

        let availability = storage::content_availability(&items);
        let stored_content_available = !items.is_empty();
        storage::store(
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
            "snippetContentAvailable": availability.snippets,
            "sourceContentAvailable": availability.sources,
            "fullContentAvailable": availability.sources,
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

    async fn call(
        &self,
        args: Value,
        ctx: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        let args: GetSearchContentArgs = serde_json::from_value(args)?;
        let stored = storage::load(&args.response_id)?;
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

fn collect_values(
    value: Option<String>,
    values: Option<Vec<String>>,
    singular: &str,
    plural: &str,
) -> Result<Vec<String>, ToolError> {
    let values = value
        .into_iter()
        .chain(values.into_iter().flatten())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if values.is_empty() {
        Err(ToolError::Message(format!(
            "{singular} or {plural} must include at least one value"
        )))
    } else {
        Ok(values)
    }
}
