pub(super) mod brave;
mod exa;
pub(super) mod openai;

use {
    crate::config::{Config, SearchProvider},
    rho_providers::credentials::{load_web_search_api_key, OsCredentialStore, WebSearchCredential},
    rho_tools::tool::ToolError,
};

use super::fetch::fetch_url_text;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SearchItem {
    pub(super) title: Option<String>,
    pub(super) url: Option<String>,
    pub(super) snippet: String,
}

#[derive(Clone, Debug)]
pub(super) struct SearchBackendConfig {
    pub(super) provider: SearchProvider,
    pub(super) legacy_openai_api_key: Option<String>,
    pub(super) legacy_exa_api_key: Option<String>,
    pub(super) legacy_brave_api_key: Option<String>,
}

impl SearchBackendConfig {
    pub(super) fn from_config(config: &Config) -> Self {
        Self {
            provider: config.web_search_provider,
            legacy_openai_api_key: config
                .legacy_web_search_api_key(WebSearchCredential::OpenAi)
                .map(str::to_string),
            legacy_exa_api_key: config
                .legacy_web_search_api_key(WebSearchCredential::Exa)
                .map(str::to_string),
            legacy_brave_api_key: config
                .legacy_web_search_api_key(WebSearchCredential::Brave)
                .map(str::to_string),
        }
    }

    pub(super) fn credential(&self, credential: WebSearchCredential) -> Option<String> {
        let stored = load_web_search_api_key(&OsCredentialStore, credential)
            .ok()
            .flatten();
        stored.or_else(|| match credential {
            WebSearchCredential::OpenAi => self.legacy_openai_api_key.clone(),
            WebSearchCredential::Exa => self.legacy_exa_api_key.clone(),
            WebSearchCredential::Brave => self.legacy_brave_api_key.clone(),
        })
    }
}

pub(super) fn openai_available(config: &SearchBackendConfig) -> bool {
    openai::is_available(config)
}

pub(super) fn brave_available(config: &SearchBackendConfig) -> bool {
    brave::resolve_api_key(config).is_some()
}

pub(super) async fn run_search_query(
    client: &reqwest::Client,
    query: &str,
    num_results: usize,
    provider: SearchProvider,
    recency_filter: Option<&str>,
    domain_filter: Option<&[String]>,
    config: &SearchBackendConfig,
) -> Result<Vec<SearchItem>, ToolError> {
    match provider {
        SearchProvider::Auto => {
            if let Ok(results) = openai::search(
                client,
                query,
                num_results,
                recency_filter,
                domain_filter,
                config,
            )
            .await
            {
                return Ok(results);
            }
            if let Ok(results) = exa::search(
                client,
                query,
                num_results,
                recency_filter,
                domain_filter,
                config,
            )
            .await
            {
                return Ok(results);
            }
            brave::search(
                client,
                query,
                num_results,
                recency_filter,
                domain_filter,
                config,
            )
            .await
        }
        SearchProvider::OpenAi => {
            openai::search(
                client,
                query,
                num_results,
                recency_filter,
                domain_filter,
                config,
            )
            .await
        }
        SearchProvider::Exa => {
            exa::search(
                client,
                query,
                num_results,
                recency_filter,
                domain_filter,
                config,
            )
            .await
        }
        SearchProvider::Brave => {
            brave::search(
                client,
                query,
                num_results,
                recency_filter,
                domain_filter,
                config,
            )
            .await
        }
        SearchProvider::Parallel
        | SearchProvider::Tavily
        | SearchProvider::Perplexity
        | SearchProvider::Gemini => Err(ToolError::Message(format!(
            "provider '{provider}' is not configured in this local MVP"
        ))),
        SearchProvider::Disabled => Err(ToolError::Message(
            "web search is disabled in config".into(),
        )),
    }
}

pub(super) async fn item_content(
    client: &reqwest::Client,
    item: &SearchItem,
    include_content: bool,
) -> (String, &'static str) {
    if !include_content {
        return (item.snippet.clone(), "snippet");
    }
    let Some(url) = item.url.as_deref() else {
        return (item.snippet.clone(), "snippet");
    };
    match fetch_url_text(client, url).await {
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
