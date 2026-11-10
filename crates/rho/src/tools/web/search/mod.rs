pub(super) mod brave;
mod exa;
mod firecrawl;
pub(super) mod openai;

use {
    crate::config::{
        Config, ExaSearchConnection, OpenAiSearchConnection, SearchBackend, WebSearchSettings,
    },
    rho_providers::{
        credentials::{
            load_codex_tokens, load_provider_api_key, load_web_search_api_key, CodexTokens,
            CredentialStore, WebSearchCredential,
        },
        providers::openai::auth::CodexAuthSource,
    },
    rho_tools::tool::ToolError,
    std::sync::Arc,
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
enum ReadyBackend {
    OpenAi(openai::OpenAiSearchAuth),
    ExaApi { key: String },
    ExaMcp,
    Brave { key: String },
    Firecrawl { key: Option<String> },
}

#[derive(Clone)]
pub(super) struct SearchBackendConfig {
    pub(super) settings: WebSearchSettings,
    ready: Result<ReadyBackend, String>,
    /// The credentials the keys were read from, which a refreshed Codex
    /// token is saved back to.
    pub(super) credentials: Arc<dyn CredentialStore>,
}

impl SearchBackendConfig {
    /// The backend `config` selects, ready when `credentials` hold its key:
    /// those of the session that searches.
    pub(super) fn from_config(config: &Config, credentials: Arc<dyn CredentialStore>) -> Self {
        Self {
            settings: config.web_search.clone(),
            ready: resolve_ready_backend(config, credentials.as_ref()),
            credentials,
        }
    }

    pub(super) fn backend(&self) -> SearchBackend {
        self.settings.backend
    }

    pub(super) fn destination_url(&self, path: &str) -> Result<String, ToolError> {
        self.settings
            .destination(self.settings.backend)
            .resolve_path(path)
            .map(|url| url.to_string())
            .map_err(|error| ToolError::Message(error.to_string()))
    }

    pub(super) fn is_ready(&self) -> bool {
        self.ready.is_ok()
    }
}

fn resolve_ready_backend(
    config: &Config,
    credentials: &dyn CredentialStore,
) -> Result<ReadyBackend, String> {
    let settings = &config.web_search;
    match settings.backend {
        SearchBackend::OpenAi => resolve_openai(config, credentials).map(ReadyBackend::OpenAi),
        SearchBackend::Exa => match settings.exa.connection {
            ExaSearchConnection::Api => Ok(ReadyBackend::ExaApi {
                key: require_key(
                    load_search_key(WebSearchCredential::Exa, credentials),
                    "EXA_API_KEY",
                )?,
            }),
            ExaSearchConnection::Mcp => Ok(ReadyBackend::ExaMcp),
        },
        SearchBackend::Brave => Ok(ReadyBackend::Brave {
            key: require_key(
                load_search_key(WebSearchCredential::Brave, credentials),
                "BRAVE_SEARCH_API_KEY",
            )?,
        }),
        SearchBackend::Firecrawl => {
            let key = load_search_key(WebSearchCredential::Firecrawl, credentials);
            if key.is_none()
                && settings
                    .destination(SearchBackend::Firecrawl)
                    .is_default_origin()
            {
                return Err("FIRECRAWL_API_KEY is not set".into());
            }
            Ok(ReadyBackend::Firecrawl { key })
        }
    }
}

fn resolve_openai(
    config: &Config,
    credentials: &dyn CredentialStore,
) -> Result<openai::OpenAiSearchAuth, String> {
    match config.web_search.openai.connection {
        OpenAiSearchConnection::Codex => {
            let (tokens, source) = resolve_codex_tokens(credentials);
            let tokens = tokens.ok_or_else(|| {
                "OpenAI Codex web search unavailable: sign in with /login openai-codex".to_string()
            })?;
            Ok(openai::OpenAiSearchAuth::Codex { tokens, source })
        }
        OpenAiSearchConnection::Api => {
            let key = load_openai_api_key(credentials).ok_or_else(|| {
                "OpenAI web search unavailable: /login openai or set OPENAI_API_KEY".to_string()
            })?;
            Ok(openai::OpenAiSearchAuth::ApiKey(key))
        }
    }
}

fn nonempty_credential(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn require_key(key: Option<String>, name: &str) -> Result<String, String> {
    key.ok_or_else(|| format!("{name} is not set"))
}

fn load_search_key(
    credential: WebSearchCredential,
    credentials: &dyn CredentialStore,
) -> Option<String> {
    env_search_key(credential, credentials).or_else(|| stored_key(credential, credentials))
}

fn env_search_key(
    credential: WebSearchCredential,
    credentials: &dyn CredentialStore,
) -> Option<String> {
    credential.env_vars().iter().find_map(|name| {
        credentials
            .environment_variable(name)
            .and_then(nonempty_credential)
    })
}

fn load_openai_api_key(credentials: &dyn CredentialStore) -> Option<String> {
    env_search_key(WebSearchCredential::OpenAi, credentials)
        .or_else(|| stored_key(WebSearchCredential::OpenAi, credentials))
        .or_else(|| {
            load_provider_api_key(credentials, "openai")
                .ok()
                .flatten()
                .and_then(nonempty_credential)
        })
}

fn stored_key(
    credential: WebSearchCredential,
    credentials: &dyn CredentialStore,
) -> Option<String> {
    load_web_search_api_key(credentials, credential)
        .ok()
        .flatten()
        .and_then(nonempty_credential)
}

fn resolve_codex_tokens(
    credentials: &dyn CredentialStore,
) -> (Option<CodexTokens>, CodexAuthSource) {
    if let Some(access_token) = credentials
        .environment_variable("CODEX_ACCESS_TOKEN")
        .and_then(nonempty_credential)
    {
        return (
            Some(CodexTokens {
                access_token,
                refresh_token: None,
                id_token: None,
                account_id: credentials.environment_variable("CODEX_ACCOUNT_ID"),
            }),
            CodexAuthSource::Env,
        );
    }
    match load_codex_tokens(credentials) {
        Ok(Some(tokens)) if !tokens.access_token.trim().is_empty() => {
            (Some(tokens), CodexAuthSource::Store)
        }
        _ => (None, CodexAuthSource::Store),
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
    let ready = config
        .ready
        .as_ref()
        .map_err(|error| ToolError::Message(error.clone()))?;
    match ready {
        ReadyBackend::OpenAi(auth) => {
            openai::search(
                client,
                query,
                num_results,
                recency_filter,
                domain_filter,
                config,
                auth,
            )
            .await
        }
        ReadyBackend::ExaApi { key } => {
            exa::search_api(
                client,
                query,
                num_results,
                recency_filter,
                domain_filter,
                key,
                config,
            )
            .await
        }
        ReadyBackend::ExaMcp => {
            exa::search_mcp(
                client,
                query,
                num_results,
                recency_filter,
                domain_filter,
                config,
            )
            .await
        }
        ReadyBackend::Brave { key } => {
            brave::search(
                client,
                query,
                num_results,
                recency_filter,
                domain_filter,
                key,
                config,
            )
            .await
        }
        ReadyBackend::Firecrawl { key } => {
            firecrawl::search(
                client,
                query,
                num_results,
                recency_filter,
                domain_filter,
                key.as_deref(),
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
    use url::Url;
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

pub(super) fn recency_label(recency_filter: &str) -> Option<&'static str> {
    match recency_filter {
        "day" => Some("past 24 hours"),
        "week" => Some("past week"),
        "month" => Some("past month"),
        "year" => Some("past year"),
        _ => None,
    }
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
