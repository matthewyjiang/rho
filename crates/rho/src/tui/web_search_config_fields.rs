//! Search URL fields and picker actions.
use rho_providers::credentials::WebSearchCredential;

use crate::config::EXA_MCP_DEFAULT_URL;

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum WebSearchUrlField {
    OpenAiApiBase,
    ExaApiBase,
    ExaMcp,
    BraveApiBase,
    FirecrawlApiBase,
}

impl WebSearchUrlField {
    pub(in crate::tui) fn label(self) -> &'static str {
        match self {
            Self::OpenAiApiBase => "OpenAI API base URL",
            Self::ExaApiBase => "Exa API base URL",
            Self::ExaMcp => "Exa MCP URL",
            Self::BraveApiBase => "Brave API base URL",
            Self::FirecrawlApiBase => "Firecrawl API base URL",
        }
    }

    pub(in crate::tui) fn value(self) -> &'static str {
        match self {
            Self::OpenAiApiBase => "web_search_openai_api_base",
            Self::ExaApiBase => "web_search_exa_api_base",
            Self::ExaMcp => "web_search_exa_mcp_url",
            Self::BraveApiBase => "web_search_brave_api_base",
            Self::FirecrawlApiBase => "web_search_firecrawl_api_base",
        }
    }

    pub(in crate::tui) fn reset_value(self) -> &'static str {
        match self {
            Self::OpenAiApiBase => "web_search_openai_api_base_reset",
            Self::ExaApiBase => "web_search_exa_api_base_reset",
            Self::ExaMcp => "web_search_exa_mcp_url_reset",
            Self::BraveApiBase => "web_search_brave_api_base_reset",
            Self::FirecrawlApiBase => "web_search_firecrawl_api_base_reset",
        }
    }

    pub(super) fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "web_search_openai_api_base" => Self::OpenAiApiBase,
            "web_search_exa_api_base" => Self::ExaApiBase,
            "web_search_exa_mcp_url" => Self::ExaMcp,
            "web_search_brave_api_base" => Self::BraveApiBase,
            "web_search_firecrawl_api_base" => Self::FirecrawlApiBase,
            _ => return None,
        })
    }

    pub(super) fn parse_reset(value: &str) -> Option<Self> {
        Some(match value {
            "web_search_openai_api_base_reset" => Self::OpenAiApiBase,
            "web_search_exa_api_base_reset" => Self::ExaApiBase,
            "web_search_exa_mcp_url_reset" => Self::ExaMcp,
            "web_search_brave_api_base_reset" => Self::BraveApiBase,
            "web_search_firecrawl_api_base_reset" => Self::FirecrawlApiBase,
            _ => return None,
        })
    }

    pub(super) fn default_url(self) -> &'static str {
        match self {
            Self::ExaMcp => EXA_MCP_DEFAULT_URL,
            Self::OpenAiApiBase
            | Self::ExaApiBase
            | Self::BraveApiBase
            | Self::FirecrawlApiBase => self.page().default_api_base(),
        }
    }

    pub(super) fn configured(self, settings: &WebSearchSettings) -> Option<&str> {
        match self {
            Self::ExaMcp => settings.exa.mcp_url.as_deref(),
            Self::OpenAiApiBase
            | Self::ExaApiBase
            | Self::BraveApiBase
            | Self::FirecrawlApiBase => settings.endpoint(self.page()),
        }
    }

    pub(super) fn set(self, settings: &mut WebSearchSettings, url: Option<String>) {
        match self {
            Self::ExaMcp => settings.exa.mcp_url = url,
            Self::OpenAiApiBase
            | Self::ExaApiBase
            | Self::BraveApiBase
            | Self::FirecrawlApiBase => settings.set_endpoint(self.page(), url),
        }
    }

    pub(super) fn page(self) -> SearchBackend {
        match self {
            Self::OpenAiApiBase => SearchBackend::OpenAi,
            Self::ExaApiBase | Self::ExaMcp => SearchBackend::Exa,
            Self::BraveApiBase => SearchBackend::Brave,
            Self::FirecrawlApiBase => SearchBackend::Firecrawl,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum WebSearchAction {
    Mode,
    Backend,
    Route,
    Info,
    Test,
    OpenBackend(SearchBackend),
    OpenAiConnection,
    ExaConnection,
    EditUrl(WebSearchUrlField),
    ResetUrl(WebSearchUrlField),
    EditKey(WebSearchCredential),
}

impl WebSearchAction {
    pub(in crate::tui) fn parse(value: &str) -> Option<Self> {
        if let Some(field) = WebSearchUrlField::parse(value) {
            return Some(Self::EditUrl(field));
        }
        if let Some(field) = WebSearchUrlField::parse_reset(value) {
            return Some(Self::ResetUrl(field));
        }
        Some(match value {
            WEB_SEARCH_MODE_VALUE => Self::Mode,
            WEB_SEARCH_BACKEND_VALUE => Self::Backend,
            WEB_SEARCH_ROUTE_VALUE => Self::Route,
            WEB_SEARCH_CODEX_ENDPOINT_VALUE => Self::Info,
            WEB_SEARCH_TEST_VALUE => Self::Test,
            WEB_SEARCH_OPENAI_PAGE_VALUE => Self::OpenBackend(SearchBackend::OpenAi),
            WEB_SEARCH_EXA_PAGE_VALUE => Self::OpenBackend(SearchBackend::Exa),
            WEB_SEARCH_BRAVE_PAGE_VALUE => Self::OpenBackend(SearchBackend::Brave),
            WEB_SEARCH_FIRECRAWL_PAGE_VALUE => Self::OpenBackend(SearchBackend::Firecrawl),
            WEB_SEARCH_OPENAI_CONNECTION_VALUE => Self::OpenAiConnection,
            WEB_SEARCH_EXA_CONNECTION_VALUE => Self::ExaConnection,
            WEB_SEARCH_OPENAI_KEY_VALUE => Self::EditKey(WebSearchCredential::OpenAi),
            WEB_SEARCH_EXA_KEY_VALUE => Self::EditKey(WebSearchCredential::Exa),
            WEB_SEARCH_BRAVE_KEY_VALUE => Self::EditKey(WebSearchCredential::Brave),
            WEB_SEARCH_FIRECRAWL_KEY_VALUE => Self::EditKey(WebSearchCredential::Firecrawl),
            _ => return None,
        })
    }

    pub(super) fn refresh_page(self) -> Option<SearchBackend> {
        match self {
            Self::Mode
            | Self::Backend
            | Self::Route
            | Self::Info
            | Self::Test
            | Self::OpenBackend(_) => None,
            Self::OpenAiConnection => Some(SearchBackend::OpenAi),
            Self::ExaConnection => Some(SearchBackend::Exa),
            Self::EditUrl(field) | Self::ResetUrl(field) => Some(field.page()),
            Self::EditKey(credential) => Some(backend_for_credential(credential)),
        }
    }
}

fn backend_for_credential(credential: WebSearchCredential) -> SearchBackend {
    match credential {
        WebSearchCredential::OpenAi => SearchBackend::OpenAi,
        WebSearchCredential::Exa => SearchBackend::Exa,
        WebSearchCredential::Brave => SearchBackend::Brave,
        WebSearchCredential::Firecrawl => SearchBackend::Firecrawl,
    }
}
