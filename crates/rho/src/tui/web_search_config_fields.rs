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
    const ALL: [Self; 5] = [
        Self::OpenAiApiBase,
        Self::ExaApiBase,
        Self::ExaMcp,
        Self::BraveApiBase,
        Self::FirecrawlApiBase,
    ];

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
        Self::ALL
            .iter()
            .copied()
            .find(|field| field.value() == value)
    }

    pub(super) fn parse_reset(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|field| field.reset_value() == value)
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
pub(in crate::tui) enum WebSearchChoiceKind {
    Mode,
    Backend,
    OpenAiConnection,
    ExaConnection,
}

impl WebSearchChoiceKind {
    pub(super) fn prefix(self) -> &'static str {
        match self {
            Self::Mode => WEB_SEARCH_MODE_PREFIX,
            Self::Backend => WEB_SEARCH_BACKEND_PREFIX,
            Self::OpenAiConnection => WEB_SEARCH_OPENAI_CONNECTION_PREFIX,
            Self::ExaConnection => WEB_SEARCH_EXA_CONNECTION_PREFIX,
        }
    }

    pub(super) fn title(self) -> &'static str {
        match self {
            Self::Mode => "Web search mode",
            Self::Backend => "Web search backend",
            Self::OpenAiConnection => "OpenAI connection",
            Self::ExaConnection => "Exa connection",
        }
    }

    pub(in crate::tui) fn parent_value(self) -> &'static str {
        match self {
            Self::Mode => WEB_SEARCH_MODE_VALUE,
            Self::Backend => WEB_SEARCH_BACKEND_VALUE,
            Self::OpenAiConnection => WEB_SEARCH_OPENAI_CONNECTION_VALUE,
            Self::ExaConnection => WEB_SEARCH_EXA_CONNECTION_VALUE,
        }
    }

    pub(in crate::tui) fn page(self) -> Option<SearchBackend> {
        match self {
            Self::Mode | Self::Backend => None,
            Self::OpenAiConnection => Some(SearchBackend::OpenAi),
            Self::ExaConnection => Some(SearchBackend::Exa),
        }
    }

    pub(in crate::tui) fn open_status(self) -> &'static str {
        match self {
            Self::Mode => "select web search mode",
            Self::Backend => "select search backend",
            Self::OpenAiConnection => "select OpenAI connection",
            Self::ExaConnection => "select Exa connection",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum WebSearchChoice {
    Mode(WebSearchMode),
    Backend(SearchBackend),
    OpenAiConnection(OpenAiSearchConnection),
    ExaConnection(ExaSearchConnection),
}

impl WebSearchChoice {
    pub(in crate::tui) fn kind(self) -> WebSearchChoiceKind {
        match self {
            Self::Mode(_) => WebSearchChoiceKind::Mode,
            Self::Backend(_) => WebSearchChoiceKind::Backend,
            Self::OpenAiConnection(_) => WebSearchChoiceKind::OpenAiConnection,
            Self::ExaConnection(_) => WebSearchChoiceKind::ExaConnection,
        }
    }

    pub(in crate::tui) fn apply(self, settings: &mut WebSearchSettings) {
        match self {
            Self::Mode(mode) => settings.mode = mode,
            Self::Backend(backend) => settings.backend = backend,
            Self::OpenAiConnection(connection) => settings.openai.connection = connection,
            Self::ExaConnection(connection) => settings.exa.connection = connection,
        }
    }

    pub(in crate::tui) fn status(self) -> String {
        match self {
            Self::Mode(mode) => format!("web search mode: {}", mode.label()),
            Self::Backend(backend) => format!("web search backend: {}", backend.label()),
            Self::OpenAiConnection(connection) => {
                format!("OpenAI search connection: {}", connection.label())
            }
            Self::ExaConnection(connection) => {
                format!("Exa search connection: {}", connection.label())
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum WebSearchAction {
    OpenChoice(WebSearchChoiceKind),
    SelectChoice(WebSearchChoice),
    Route,
    Test,
    OpenBackend(SearchBackend),
    CodexEndpoint,
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
        if let Some(mode) = value.strip_prefix(WEB_SEARCH_MODE_PREFIX) {
            return mode
                .parse()
                .ok()
                .map(|mode| Self::SelectChoice(WebSearchChoice::Mode(mode)));
        }
        if let Some(backend) = value.strip_prefix(WEB_SEARCH_BACKEND_PREFIX) {
            return backend
                .parse()
                .ok()
                .map(|backend| Self::SelectChoice(WebSearchChoice::Backend(backend)));
        }
        if let Some(connection) = value.strip_prefix(WEB_SEARCH_OPENAI_CONNECTION_PREFIX) {
            return connection.parse().ok().map(|connection| {
                Self::SelectChoice(WebSearchChoice::OpenAiConnection(connection))
            });
        }
        if let Some(connection) = value.strip_prefix(WEB_SEARCH_EXA_CONNECTION_PREFIX) {
            return connection
                .parse()
                .ok()
                .map(|connection| Self::SelectChoice(WebSearchChoice::ExaConnection(connection)));
        }
        Some(match value {
            WEB_SEARCH_MODE_VALUE => Self::OpenChoice(WebSearchChoiceKind::Mode),
            WEB_SEARCH_BACKEND_VALUE => Self::OpenChoice(WebSearchChoiceKind::Backend),
            WEB_SEARCH_ROUTE_VALUE => Self::Route,
            WEB_SEARCH_TEST_VALUE => Self::Test,
            WEB_SEARCH_OPENAI_PAGE_VALUE => Self::OpenBackend(SearchBackend::OpenAi),
            WEB_SEARCH_EXA_PAGE_VALUE => Self::OpenBackend(SearchBackend::Exa),
            WEB_SEARCH_BRAVE_PAGE_VALUE => Self::OpenBackend(SearchBackend::Brave),
            WEB_SEARCH_FIRECRAWL_PAGE_VALUE => Self::OpenBackend(SearchBackend::Firecrawl),
            WEB_SEARCH_OPENAI_CONNECTION_VALUE => {
                Self::OpenChoice(WebSearchChoiceKind::OpenAiConnection)
            }
            WEB_SEARCH_EXA_CONNECTION_VALUE => Self::OpenChoice(WebSearchChoiceKind::ExaConnection),
            WEB_SEARCH_CODEX_ENDPOINT_VALUE => Self::CodexEndpoint,
            WEB_SEARCH_OPENAI_KEY_VALUE => Self::EditKey(WebSearchCredential::OpenAi),
            WEB_SEARCH_EXA_KEY_VALUE => Self::EditKey(WebSearchCredential::Exa),
            WEB_SEARCH_BRAVE_KEY_VALUE => Self::EditKey(WebSearchCredential::Brave),
            WEB_SEARCH_FIRECRAWL_KEY_VALUE => Self::EditKey(WebSearchCredential::Firecrawl),
            _ => return None,
        })
    }

    pub(super) fn refresh_page(self) -> Option<SearchBackend> {
        match self {
            Self::OpenChoice(kind) => kind.page(),
            Self::SelectChoice(choice) => choice.kind().page(),
            Self::Route | Self::Test | Self::OpenBackend(_) => None,
            Self::CodexEndpoint => Some(SearchBackend::OpenAi),
            Self::EditUrl(field) | Self::ResetUrl(field) => Some(field.page()),
            Self::EditKey(credential) => Some(SearchBackend::from_credential(credential)),
        }
    }
}
