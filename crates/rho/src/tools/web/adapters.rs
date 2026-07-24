use serde::Deserialize;
use serde_json::{json, Value};

use {
    crate::config::{Config, SearchProvider},
    rho_tools::tool::{truncate, Tool, ToolContext, ToolError, ToolResult, ToolSpec},
};

use super::{
    search::{self, SearchBackendConfig},
    storage::{self, StoredContent, StoredItem, WebAccessStore},
    util::to_pretty_json,
};

pub struct WebSearch {
    config: SearchBackendConfig,
    client: reqwest::Client,
    store: WebAccessStore,
}

pub struct GetSearchContent {
    store: WebAccessStore,
}

impl GetSearchContent {
    pub(super) fn new(store: WebAccessStore) -> Self {
        Self { store }
    }
}

impl WebSearch {
    pub(super) fn with_client(
        config: &Config,
        client: reqwest::Client,
        store: WebAccessStore,
    ) -> Self {
        Self {
            config: SearchBackendConfig::from_config(config),
            client,
            store,
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
pub(super) struct GetSearchContentArgs {
    pub(super) response_id: String,
    pub(super) query: Option<String>,
    pub(super) query_index: Option<usize>,
    pub(super) url: Option<String>,
    pub(super) url_index: Option<usize>,
}

#[async_trait::async_trait]
impl Tool for WebSearch {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "web_search".into(),
            description: "Search the web through a zero-config interface with optional provider credentials. Returns a concise summary, stores snippets by default under a responseId, and stores full source pages only when includeContent succeeds. Use get_search_content with that responseId when you need stored snippets or source pages.".into(),
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
                            search::item_content(&self.client, &item, include_content).await;
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
        self.store.store(
            response_id.clone(),
            StoredContent {
                kind: "web_search".into(),
                items,
            },
        )?;

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
            "note": "Summary is inline. Call get_search_content with only responseId (or an exact original query / queryIndex / url / urlIndex) for stored snippets; full source pages exist only when includeContent succeeded."
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
            description: "Retrieve stored web_search snippets/source pages or fetch_content bodies by responseId. Prefer responseId alone, or exact url/urlIndex/query/queryIndex selectors from the prior tool result. query is the original search query or fetch prompt, not a free-text content search.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "responseId": {"type": "string", "pattern": "^[0-9a-f]{32}$", "description": "responseId returned by web_search or fetch_content."},
                    "query": {"type": "string", "description": "Exact original web_search query or fetch_content prompt. Not a keyword search over page text."},
                    "queryIndex": {"type": "integer", "minimum": 0, "description": "Index among stored items that have a query."},
                    "url": {"type": "string", "description": "Exact stored URL to select."},
                    "urlIndex": {"type": "integer", "minimum": 0, "description": "Index into the stored item list."}
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
        self.execute(args, ctx.max_output_bytes, id)
    }
}

impl GetSearchContent {
    pub(super) fn execute(
        &self,
        args: GetSearchContentArgs,
        max_output_bytes: usize,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        let stored = self.store.load(&args.response_id)?;
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
            content: truncate(to_pretty_json(&content), max_output_bytes),
        })
    }
}

fn select_stored_item<'a>(
    stored: &'a StoredContent,
    args: &GetSearchContentArgs,
) -> Result<&'a StoredItem, ToolError> {
    let available = || storage::available_selectors(stored);
    if let Some(url) = &args.url {
        return stored
            .items
            .iter()
            .find(|item| item.url.as_deref() == Some(url.as_str()))
            .ok_or_else(|| {
                ToolError::Message(format!(
                    "url not found for responseId: {url}. Available selectors:\n{}",
                    available()
                ))
            });
    }
    if let Some(index) = args.url_index {
        return stored.items.get(index).ok_or_else(|| {
            ToolError::Message(format!(
                "urlIndex out of range: {index}. Available selectors:\n{}",
                available()
            ))
        });
    }
    if let Some(query) = &args.query {
        return stored
            .items
            .iter()
            .find(|item| item.query.as_deref() == Some(query.as_str()))
            .ok_or_else(|| {
                ToolError::Message(format!(
                    "query not found for responseId: {query}. query must equal an original web_search query or fetch_content prompt, not page keywords. Available selectors:\n{}",
                    available()
                ))
            });
    }
    if let Some(index) = args.query_index {
        return stored
            .items
            .iter()
            .filter(|item| item.query.is_some())
            .nth(index)
            .ok_or_else(|| {
                ToolError::Message(format!(
                    "queryIndex out of range: {index}. Available selectors:\n{}",
                    available()
                ))
            });
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
