//! Web-search routing, backend settings, and config migration.
//!
//! Mode chooses Auto / Backend / Off. Backend is one of OpenAI, Exa, Brave, or
//! Firecrawl. Auto prefers native chat-provider search when the caller reports
//! that the active path supports it; otherwise it uses the selected backend
//! only. There is no runtime fallback across backends.

use std::{fmt, str::FromStr};

use rho_providers::credentials::WebSearchCredential;
use serde::{Deserialize, Serialize};
use url::Url;

#[path = "config_web_search_endpoint.rs"]
mod endpoint;
#[path = "config_web_search_migrate.rs"]
mod migrate;

#[cfg(test)]
use endpoint::join_api_path;
pub use endpoint::{
    parse_search_endpoint_url, resolved_endpoint_url, BRAVE_API_DEFAULT_BASE, EXA_API_DEFAULT_BASE,
    EXA_MCP_DEFAULT_URL, FIRECRAWL_API_DEFAULT_BASE, OPENAI_API_DEFAULT_BASE,
    OPENAI_CODEX_RESPONSES_URL,
};
#[cfg(test)]
use migrate::migrate_legacy_web_search;
pub(super) use migrate::{
    resolve_web_search_settings, EndpointPartial, ExaSearchPartial, OpenAiSearchPartial,
    WebSearchLoadWarning, WebSearchPartial,
};

/// How Rho should pick a web-search path.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WebSearchMode {
    /// Native chat-provider search when supported, otherwise the selected backend.
    #[default]
    Auto,
    /// Always the selected backend, even when native search is supported.
    Backend,
    /// Do not search.
    Off,
}

impl WebSearchMode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Backend => "backend",
            Self::Off => "off",
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Backend => "Backend",
            Self::Off => "Off",
        }
    }

    pub(crate) const fn next(self) -> Self {
        match self {
            Self::Auto => Self::Backend,
            Self::Backend => Self::Off,
            Self::Off => Self::Auto,
        }
    }
}

impl fmt::Display for WebSearchMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for WebSearchMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "backend" => Ok(Self::Backend),
            "off" => Ok(Self::Off),
            other => Err(format!("unknown web search mode: {other}")),
        }
    }
}

/// Implemented client search backends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchBackend {
    #[default]
    OpenAi,
    Exa,
    Brave,
    Firecrawl,
}

impl SearchBackend {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Exa => "exa",
            Self::Brave => "brave",
            Self::Firecrawl => "firecrawl",
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::OpenAi => "OpenAI",
            Self::Exa => "Exa",
            Self::Brave => "Brave",
            Self::Firecrawl => "Firecrawl",
        }
    }

    pub(crate) const fn next(self) -> Self {
        match self {
            Self::OpenAi => Self::Exa,
            Self::Exa => Self::Brave,
            Self::Brave => Self::Firecrawl,
            Self::Firecrawl => Self::OpenAi,
        }
    }

    pub(crate) const fn default_api_base(self) -> &'static str {
        match self {
            Self::OpenAi => OPENAI_API_DEFAULT_BASE,
            Self::Exa => EXA_API_DEFAULT_BASE,
            Self::Brave => BRAVE_API_DEFAULT_BASE,
            Self::Firecrawl => FIRECRAWL_API_DEFAULT_BASE,
        }
    }

    pub(crate) const fn credential(self) -> WebSearchCredential {
        match self {
            Self::OpenAi => WebSearchCredential::OpenAi,
            Self::Exa => WebSearchCredential::Exa,
            Self::Brave => WebSearchCredential::Brave,
            Self::Firecrawl => WebSearchCredential::Firecrawl,
        }
    }
}

impl fmt::Display for SearchBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for SearchBackend {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "openai" => Ok(Self::OpenAi),
            "exa" => Ok(Self::Exa),
            "brave" => Ok(Self::Brave),
            "firecrawl" => Ok(Self::Firecrawl),
            other => Err(format!("unknown web search backend: {other}")),
        }
    }
}

/// Explicit OpenAI search transport. Tokens never pick this at runtime.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenAiSearchConnection {
    #[default]
    Api,
    Codex,
}

impl OpenAiSearchConnection {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Codex => "codex",
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Api => "OpenAI API",
            Self::Codex => "Codex",
        }
    }

    pub(crate) const fn next(self) -> Self {
        match self {
            Self::Api => Self::Codex,
            Self::Codex => Self::Api,
        }
    }
}

impl fmt::Display for OpenAiSearchConnection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for OpenAiSearchConnection {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "api" => Ok(Self::Api),
            "codex" => Ok(Self::Codex),
            other => Err(format!("unknown OpenAI search connection: {other}")),
        }
    }
}

/// Explicit Exa search transport. A stored API key never selects MCP.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExaSearchConnection {
    #[default]
    Api,
    Mcp,
}

impl ExaSearchConnection {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Mcp => "mcp",
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Api => "Exa API",
            Self::Mcp => "Exa MCP",
        }
    }

    pub(crate) const fn next(self) -> Self {
        match self {
            Self::Api => Self::Mcp,
            Self::Mcp => Self::Api,
        }
    }
}

impl fmt::Display for ExaSearchConnection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ExaSearchConnection {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "api" => Ok(Self::Api),
            "mcp" => Ok(Self::Mcp),
            other => Err(format!("unknown Exa search connection: {other}")),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WebSearchEndpointSettings {
    /// Override for the backend API origin and reverse-proxy prefix.
    pub api_base_url: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenAiSearchSettings {
    pub connection: OpenAiSearchConnection,
    pub api_base_url: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExaSearchSettings {
    pub connection: ExaSearchConnection,
    pub api_base_url: Option<String>,
    /// MCP endpoint, distinct from the Exa HTTP API base.
    pub mcp_url: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WebSearchSettings {
    pub mode: WebSearchMode,
    pub backend: SearchBackend,
    pub openai: OpenAiSearchSettings,
    pub exa: ExaSearchSettings,
    pub brave: WebSearchEndpointSettings,
    pub firecrawl: WebSearchEndpointSettings,
}

impl WebSearchSettings {
    pub(crate) fn endpoint(&self, backend: SearchBackend) -> Option<&str> {
        match backend {
            SearchBackend::OpenAi => self.openai.api_base_url.as_deref(),
            SearchBackend::Exa => self.exa.api_base_url.as_deref(),
            SearchBackend::Brave => self.brave.api_base_url.as_deref(),
            SearchBackend::Firecrawl => self.firecrawl.api_base_url.as_deref(),
        }
    }

    pub(crate) fn set_endpoint(&mut self, backend: SearchBackend, url: Option<String>) {
        match backend {
            SearchBackend::OpenAi => self.openai.api_base_url = url,
            SearchBackend::Exa => self.exa.api_base_url = url,
            SearchBackend::Brave => self.brave.api_base_url = url,
            SearchBackend::Firecrawl => self.firecrawl.api_base_url = url,
        }
    }

    /// Canonical destination for the selected connection of `backend`.
    pub(crate) fn destination(&self, backend: SearchBackend) -> SearchDestination<'_> {
        match backend {
            SearchBackend::OpenAi => match self.openai.connection {
                OpenAiSearchConnection::Codex => SearchDestination::Fixed {
                    label: "Codex",
                    url: OPENAI_CODEX_RESPONSES_URL,
                },
                OpenAiSearchConnection::Api => SearchDestination::Resolved {
                    label: "OpenAI API",
                    configured: self.openai.api_base_url.as_deref(),
                    default_base: OPENAI_API_DEFAULT_BASE,
                    paths: &["responses"],
                },
            },
            SearchBackend::Exa => match self.exa.connection {
                ExaSearchConnection::Api => SearchDestination::Resolved {
                    label: "Exa API",
                    configured: self.exa.api_base_url.as_deref(),
                    default_base: EXA_API_DEFAULT_BASE,
                    paths: &["search", "answer"],
                },
                ExaSearchConnection::Mcp => SearchDestination::Resolved {
                    label: "Exa MCP",
                    configured: self.exa.mcp_url.as_deref(),
                    default_base: EXA_MCP_DEFAULT_URL,
                    paths: &[""],
                },
            },
            SearchBackend::Brave => SearchDestination::Resolved {
                label: "Brave API",
                configured: self.brave.api_base_url.as_deref(),
                default_base: BRAVE_API_DEFAULT_BASE,
                paths: &["res/v1/web/search"],
            },
            SearchBackend::Firecrawl => SearchDestination::Resolved {
                label: "Firecrawl API",
                configured: self.firecrawl.api_base_url.as_deref(),
                default_base: FIRECRAWL_API_DEFAULT_BASE,
                paths: &["v2/search"],
            },
        }
    }
}

/// Where a client backend actually sends queries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SearchDestination<'a> {
    Fixed {
        label: &'static str,
        url: &'static str,
    },
    Resolved {
        label: &'static str,
        configured: Option<&'a str>,
        default_base: &'static str,
        paths: &'static [&'static str],
    },
}

impl<'a> SearchDestination<'a> {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Fixed { label, .. } | Self::Resolved { label, .. } => label,
        }
    }

    pub(crate) fn configured(self) -> Option<&'a str> {
        match self {
            Self::Fixed { .. } => None,
            Self::Resolved { configured, .. } => configured,
        }
    }

    pub(crate) fn resolve_path(self, path: &str) -> anyhow::Result<Url> {
        match self {
            Self::Fixed { url, .. } => Url::parse(url).map_err(anyhow::Error::from),
            Self::Resolved {
                configured,
                default_base,
                ..
            } => resolved_endpoint_url(configured, default_base, path),
        }
    }

    pub(crate) fn resolve_all(self) -> anyhow::Result<Vec<Url>> {
        match self {
            Self::Fixed { url, .. } => Ok(vec![Url::parse(url)?]),
            Self::Resolved { paths, .. } => {
                paths.iter().map(|path| self.resolve_path(path)).collect()
            }
        }
    }
}

/// Cloud Firecrawl requires a key; any other origin may omit auth.
pub fn firecrawl_uses_cloud_default(configured: Option<&str>) -> bool {
    let Some(configured) = configured.map(str::trim).filter(|value| !value.is_empty()) else {
        return true;
    };
    let Ok(parsed) = parse_search_endpoint_url("web_search.firecrawl.api_base_url", configured)
    else {
        return false;
    };
    let Ok(default) = Url::parse(FIRECRAWL_API_DEFAULT_BASE) else {
        return false;
    };
    endpoint::same_origin_and_prefix(&parsed, &default)
}

/// Effective search path after applying mode to the active chat provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebSearchRoute {
    Off,
    Native,
    Backend(SearchBackend),
}

/// Pure routing. `hosted_supported` comes from runtime capability checks.
pub(crate) fn web_search_route(
    settings: &WebSearchSettings,
    hosted_supported: bool,
) -> WebSearchRoute {
    match settings.mode {
        WebSearchMode::Off => WebSearchRoute::Off,
        WebSearchMode::Backend => WebSearchRoute::Backend(settings.backend),
        WebSearchMode::Auto => {
            if hosted_supported {
                WebSearchRoute::Native
            } else {
                WebSearchRoute::Backend(settings.backend)
            }
        }
    }
}

#[cfg(test)]
#[path = "config_web_search_tests.rs"]
mod tests;
