pub(super) mod brave;
mod exa;
mod firecrawl;
pub(super) mod openai;

use {
    crate::{
        config::{
            firecrawl_uses_cloud_default, Config, ExaSearchConnection, OpenAiSearchConnection,
            SearchBackend, WebSearchSettings, BRAVE_API_DEFAULT_BASE, EXA_API_DEFAULT_BASE,
            EXA_MCP_DEFAULT_URL, FIRECRAWL_API_DEFAULT_BASE, OPENAI_API_DEFAULT_BASE,
            OPENAI_CODEX_RESPONSES_URL,
        },
        credential_store::AppCredentialStore,
    },
    rho_providers::{
        credentials::{
            load_codex_tokens, load_provider_api_key, load_web_search_api_key, CodexTokens,
            WebSearchCredential,
        },
        providers::openai::auth::CodexAuthSource,
    },
    rho_tools::tool::ToolError,
};

use super::fetch::fetch_url_text;

/// Search JSON is tens of KB. 1 MiB is a tripwire for runaway bodies, not a
/// working-set size.
pub(super) const SEARCH_RESPONSE_MAX_BYTES: usize = 1_048_576;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SearchItem {
    pub(super) title: Option<String>,
    pub(super) url: Option<String>,
    pub(super) snippet: String,
}

#[derive(Clone)]
pub(super) struct SearchBackendConfig {
    pub(super) settings: WebSearchSettings,
    pub(super) openai_api_key: Option<String>,
    pub(super) openai_codex_tokens: Option<CodexTokens>,
    pub(super) openai_codex_source: CodexAuthSource,
    pub(super) exa_api_key: Option<String>,
    pub(super) brave_api_key: Option<String>,
    pub(super) firecrawl_api_key: Option<String>,
}

impl SearchBackendConfig {
    pub(super) fn from_config(config: &Config) -> Self {
        let settings = config.web_search.clone();
        let openai_api_key = match settings.openai.connection {
            OpenAiSearchConnection::Api => std::env::var("OPENAI_API_KEY")
                .ok()
                .and_then(nonempty_credential)
                .or_else(|| stored_or_legacy(config, WebSearchCredential::OpenAi))
                .or_else(|| {
                    load_provider_api_key(&AppCredentialStore, "openai")
                        .ok()
                        .flatten()
                        .and_then(nonempty_credential)
                }),
            OpenAiSearchConnection::Codex => None,
        };
        let (openai_codex_tokens, openai_codex_source) = match settings.openai.connection {
            OpenAiSearchConnection::Codex => resolve_codex_tokens(),
            OpenAiSearchConnection::Api => (None, CodexAuthSource::Env),
        };
        Self {
            openai_api_key,
            openai_codex_tokens,
            openai_codex_source,
            exa_api_key: std::env::var("EXA_API_KEY")
                .ok()
                .and_then(nonempty_credential)
                .or_else(|| stored_or_legacy(config, WebSearchCredential::Exa)),
            brave_api_key: std::env::var("BRAVE_SEARCH_API_KEY")
                .ok()
                .and_then(nonempty_credential)
                .or_else(|| std::env::var("BRAVE_API_KEY").ok())
                .and_then(nonempty_credential)
                .or_else(|| stored_or_legacy(config, WebSearchCredential::Brave)),
            firecrawl_api_key: std::env::var("FIRECRAWL_API_KEY")
                .ok()
                .and_then(nonempty_credential)
                .or_else(|| {
                    load_web_search_api_key(&AppCredentialStore, WebSearchCredential::Firecrawl)
                        .ok()
                        .flatten()
                        .and_then(nonempty_credential)
                }),
            settings,
        }
    }

    pub(super) fn backend(&self) -> SearchBackend {
        self.settings.backend
    }
}

fn nonempty_credential(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn stored_or_legacy(config: &Config, credential: WebSearchCredential) -> Option<String> {
    load_web_search_api_key(&AppCredentialStore, credential)
        .ok()
        .flatten()
        .and_then(nonempty_credential)
        .or_else(|| {
            config
                .legacy_web_search_api_key(credential)
                .map(str::to_string)
                .and_then(nonempty_credential)
        })
}

fn resolve_codex_tokens() -> (Option<CodexTokens>, CodexAuthSource) {
    if let Some(access_token) = std::env::var("CODEX_ACCESS_TOKEN")
        .ok()
        .and_then(nonempty_credential)
    {
        return (
            Some(CodexTokens {
                access_token,
                refresh_token: None,
                id_token: None,
                account_id: std::env::var("CODEX_ACCOUNT_ID").ok(),
            }),
            CodexAuthSource::Env,
        );
    }
    match load_codex_tokens(&AppCredentialStore) {
        Ok(Some(tokens)) if !tokens.access_token.trim().is_empty() => {
            (Some(tokens), CodexAuthSource::Store)
        }
        _ => (None, CodexAuthSource::Store),
    }
}

pub(super) fn backend_available(config: &SearchBackendConfig) -> bool {
    match config.backend() {
        SearchBackend::OpenAi => openai::is_available(config),
        SearchBackend::Exa => match config.settings.exa.connection {
            ExaSearchConnection::Api => config.exa_api_key.is_some(),
            ExaSearchConnection::Mcp => true,
        },
        SearchBackend::Brave => config.brave_api_key.is_some(),
        SearchBackend::Firecrawl => {
            config.firecrawl_api_key.is_some()
                || !firecrawl_uses_cloud_default(config.settings.firecrawl.api_base_url.as_deref())
        }
    }
}

pub(super) async fn run_search_query(
    client: &reqwest::Client,
    query: &str,
    num_results: usize,
    recency_filter: Option<&str>,
    domain_filter: Option<&[String]>,
    config: &SearchBackendConfig,
) -> Result<Vec<SearchItem>, ToolError> {
    match config.backend() {
        SearchBackend::OpenAi => {
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
        SearchBackend::Exa => {
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
        SearchBackend::Brave => {
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
        SearchBackend::Firecrawl => {
            firecrawl::search(
                client,
                query,
                num_results,
                recency_filter,
                domain_filter,
                config,
            )
            .await
        }
    }
}

pub(super) async fn item_content(
    item: &SearchItem,
    include_content: bool,
) -> (String, &'static str) {
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

pub(super) fn endpoint_url(
    configured: Option<&str>,
    default_base: &str,
    relative: &str,
) -> Result<String, ToolError> {
    crate::config::resolved_endpoint_url(configured, default_base, relative)
        .map(|url| url.to_string())
        .map_err(|error| ToolError::Message(error.to_string()))
}

pub(super) fn openai_responses_url(config: &SearchBackendConfig) -> Result<String, ToolError> {
    match config.settings.openai.connection {
        OpenAiSearchConnection::Codex => Ok(OPENAI_CODEX_RESPONSES_URL.to_string()),
        OpenAiSearchConnection::Api => endpoint_url(
            config.settings.openai.api_base_url.as_deref(),
            OPENAI_API_DEFAULT_BASE,
            "responses",
        ),
    }
}

pub(super) fn exa_api_url(
    config: &SearchBackendConfig,
    relative: &str,
) -> Result<String, ToolError> {
    endpoint_url(
        config.settings.exa.api_base_url.as_deref(),
        EXA_API_DEFAULT_BASE,
        relative,
    )
}

pub(super) fn exa_mcp_url(config: &SearchBackendConfig) -> Result<String, ToolError> {
    endpoint_url(
        config.settings.exa.mcp_url.as_deref(),
        EXA_MCP_DEFAULT_URL,
        "",
    )
}

pub(super) fn brave_search_url(config: &SearchBackendConfig) -> Result<String, ToolError> {
    endpoint_url(
        config.settings.brave.api_base_url.as_deref(),
        BRAVE_API_DEFAULT_BASE,
        "res/v1/web/search",
    )
}

pub(super) fn firecrawl_search_url(config: &SearchBackendConfig) -> Result<String, ToolError> {
    endpoint_url(
        config.settings.firecrawl.api_base_url.as_deref(),
        FIRECRAWL_API_DEFAULT_BASE,
        "v2/search",
    )
}

pub(super) fn apply_site_filters(query: &str, domain_filter: Option<&[String]>) -> String {
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

pub(super) fn redact_secrets(message: &str, secrets: &[&str]) -> String {
    let mut message = message.to_string();
    for secret in secrets {
        if !secret.is_empty() {
            message = message.replace(*secret, "***");
        }
    }
    message
}

pub(super) fn search_error(message: String, secrets: &[&str]) -> ToolError {
    ToolError::Message(redact_secrets(&message, secrets))
}

pub(super) async fn read_bounded_text(
    mut response: reqwest::Response,
    secrets: &[&str],
) -> Result<(reqwest::StatusCode, String), ToolError> {
    let status = response.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| search_error(format!("search response failed: {error}"), secrets))?
    {
        if chunk.len() > SEARCH_RESPONSE_MAX_BYTES.saturating_sub(bytes.len()) {
            return Err(ToolError::Message(format!(
                "search response exceeded {SEARCH_RESPONSE_MAX_BYTES} bytes"
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| ToolError::Message("search response was not UTF-8".into()))?;
    Ok((status, text))
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
