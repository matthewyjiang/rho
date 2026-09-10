//! Web-search `/config` policy: pickers, routing copy, URL vs secret editors.
//!
//! Generic picker rendering stays in the shared picker. This module owns the
//! Mode / backend / endpoint rows and must not treat configuring a backend as
//! selecting it.

use rho_providers::credentials::{
    load_web_search_api_key, CredentialResult, CredentialStore, WebSearchCredential,
};

use crate::config::{
    parse_search_endpoint_url, web_search_route, Config, OpenAiSearchConnection, SearchBackend,
    WebSearchRoute, WebSearchSettings, EXA_MCP_DEFAULT_URL, OPENAI_CODEX_RESPONSES_URL,
};

use super::{
    config_editor::ConfigTextKey,
    config_row::ConfigCommitCtx,
    picker::{PickerBadge, PickerBadgeTone, PickerItem, UiPicker},
    App, ComposerMode, Entry, InlineChoice, InlineChoiceModal, InlineChoiceOption,
    InlineChoicePending,
};

pub(super) const WEB_SEARCH_MODE_VALUE: &str = "web_search_mode";
pub(super) const WEB_SEARCH_BACKEND_VALUE: &str = "web_search_backend";
pub(super) const WEB_SEARCH_ROUTE_VALUE: &str = "web_search_route";
pub(super) const WEB_SEARCH_TEST_VALUE: &str = "web_search_test";
pub(super) const WEB_SEARCH_OPENAI_PAGE_VALUE: &str = "web_search_openai";
pub(super) const WEB_SEARCH_EXA_PAGE_VALUE: &str = "web_search_exa";
pub(super) const WEB_SEARCH_BRAVE_PAGE_VALUE: &str = "web_search_brave";
pub(super) const WEB_SEARCH_FIRECRAWL_PAGE_VALUE: &str = "web_search_firecrawl";
pub(super) const WEB_SEARCH_OPENAI_CONNECTION_VALUE: &str = "web_search_openai_connection";
pub(super) const WEB_SEARCH_EXA_CONNECTION_VALUE: &str = "web_search_exa_connection";
pub(super) const WEB_SEARCH_OPENAI_KEY_VALUE: &str = "web_search_openai_api_key";
pub(super) const WEB_SEARCH_EXA_KEY_VALUE: &str = "web_search_exa_api_key";
pub(super) const WEB_SEARCH_BRAVE_KEY_VALUE: &str = "web_search_brave_api_key";
pub(super) const WEB_SEARCH_FIRECRAWL_KEY_VALUE: &str = "web_search_firecrawl_api_key";

const TEST_CONFIRM_VALUE: &str = "confirm";

pub(super) use fields::{WebSearchAction, WebSearchUrlField};
#[path = "web_search_config_fields.rs"]
mod fields;

pub(super) fn summary(config: &Config, provider: &str, model: &str) -> String {
    route_summary(effective_route(config, provider, model), &config.web_search)
}

fn effective_route(config: &Config, provider: &str, model: &str) -> WebSearchRoute {
    web_search_route(
        &config.web_search,
        crate::tools::web::supports_hosted_web_search(provider, model),
    )
}

fn route_summary(route: WebSearchRoute, settings: &WebSearchSettings) -> String {
    match route {
        WebSearchRoute::Off => "off".into(),
        WebSearchRoute::Native => "native selected for current model".into(),
        WebSearchRoute::Backend(backend) => {
            let (connection, configured, default, path) = match backend {
                SearchBackend::OpenAi => match settings.openai.connection {
                    OpenAiSearchConnection::Codex => {
                        return format!("Codex · {OPENAI_CODEX_RESPONSES_URL} · fixed")
                    }
                    OpenAiSearchConnection::Api => (
                        "OpenAI API",
                        settings.openai.api_base_url.as_deref(),
                        backend.default_api_base(),
                        "responses",
                    ),
                },
                SearchBackend::Exa => match settings.exa.connection {
                    crate::config::ExaSearchConnection::Api => (
                        "Exa API",
                        settings.exa.api_base_url.as_deref(),
                        backend.default_api_base(),
                        "search",
                    ),
                    crate::config::ExaSearchConnection::Mcp => (
                        "Exa MCP",
                        settings.exa.mcp_url.as_deref(),
                        EXA_MCP_DEFAULT_URL,
                        "",
                    ),
                },
                SearchBackend::Brave => (
                    "Brave API",
                    settings.brave.api_base_url.as_deref(),
                    backend.default_api_base(),
                    "res/v1/web/search",
                ),
                SearchBackend::Firecrawl => (
                    "Firecrawl API",
                    settings.firecrawl.api_base_url.as_deref(),
                    backend.default_api_base(),
                    "v2/search",
                ),
            };
            match crate::config::resolved_endpoint_url(configured, default, path) {
                Ok(url) => {
                    let destination = if backend == SearchBackend::Exa
                        && settings.exa.connection == crate::config::ExaSearchConnection::Api
                    {
                        let answer =
                            crate::config::resolved_endpoint_url(configured, default, "answer")
                                .expect("validated Exa base URL");
                        format!("{url} or {answer}")
                    } else {
                        url.to_string()
                    };
                    format!(
                        "{connection} · {destination} · {}",
                        if configured.is_some() {
                            "custom"
                        } else {
                            "default"
                        }
                    )
                }
                Err(error) => format!("{connection}: {error}"),
            }
        }
    }
}

fn badge(text: impl Into<String>) -> PickerBadge {
    PickerBadge {
        text: text.into(),
        tone: PickerBadgeTone::Selected,
    }
}

fn item(
    label: &str,
    detail: impl Into<String>,
    badge_text: Option<String>,
    value: &str,
) -> PickerItem {
    PickerItem {
        section: None,
        label: label.into(),
        detail: Some(detail.into()),
        preview: None,
        badge: badge_text.map(badge),
        value: value.into(),
        selection_verb: None,
        allow_filter_completion: true,
    }
}

fn url_badge(field: WebSearchUrlField, settings: &WebSearchSettings) -> String {
    field
        .configured(settings)
        .unwrap_or(field.default_url())
        .to_string()
}

fn credential_badge(
    config: &Config,
    credential_store: &dyn CredentialStore,
    credential: WebSearchCredential,
) -> PickerBadge {
    let configured = web_search_api_key_is_set(
        load_web_search_api_key(credential_store, credential),
        config.legacy_web_search_api_key(credential),
    );
    badge(if configured { "set" } else { "unset" })
}

fn web_search_api_key_is_set(
    stored: CredentialResult<Option<String>>,
    legacy: Option<&str>,
) -> bool {
    stored
        .ok()
        .flatten()
        .as_deref()
        .or(legacy)
        .is_some_and(|value| !value.trim().is_empty())
}

pub(super) fn main_picker(
    config: &Config,
    _credential_store: &dyn CredentialStore,
    provider: &str,
    model: &str,
) -> UiPicker {
    let settings = &config.web_search;
    let route = effective_route(config, provider, model);
    UiPicker::config(
        "Web search",
        vec![
            item(
                "Mode",
                format!(
                    "Auto uses native search when the current model supports it, otherwise the selected backend. Backend always uses that backend. Off disables search. Enter cycles to {}.",
                    settings.mode.next().label()
                ),
                Some(settings.mode.label().into()),
                WEB_SEARCH_MODE_VALUE,
            ),
            item(
                "Search backend",
                format!(
                    "Client backend used when Mode is Backend, or when Mode is Auto and native search is not selected. Configuring a backend does not select it. Enter cycles to {}.",
                    settings.backend.next().label()
                ),
                Some(settings.backend.label().into()),
                WEB_SEARCH_BACKEND_VALUE,
            ),
            item(
                "Next turn route",
                format!("{}\nApplied before the next turn. An active turn keeps its existing route and credentials.", route_summary(route, settings)),
                Some(route_summary(route, settings)),
                WEB_SEARCH_ROUTE_VALUE,
            ),
            item(
                "OpenAI",
                "API or Codex connection, endpoint, and API key. Opening this page does not select OpenAI.",
                Some(openai_page_badge(settings)),
                WEB_SEARCH_OPENAI_PAGE_VALUE,
            ),
            item(
                "Exa",
                "API or MCP connection, endpoints, and API key. Opening this page does not select Exa.",
                Some(exa_page_badge(settings)),
                WEB_SEARCH_EXA_PAGE_VALUE,
            ),
            item(
                "Brave",
                "API base URL and API key. Opening this page does not select Brave.",
                Some(url_badge(WebSearchUrlField::BraveApiBase, settings)),
                WEB_SEARCH_BRAVE_PAGE_VALUE,
            ),
            item(
                "Firecrawl",
                "API base URL and API key. Opening this page does not select Firecrawl.",
                Some(url_badge(WebSearchUrlField::FirecrawlApiBase, settings)),
                WEB_SEARCH_FIRECRAWL_PAGE_VALUE,
            ),
            item(
                "Test connection",
                "Send a test query through the selected backend. Asks before the query leaves this machine.",
                Some("run now".into()),
                WEB_SEARCH_TEST_VALUE,
            ),
        ],
    )
}

fn openai_page_badge(settings: &WebSearchSettings) -> String {
    settings.openai.connection.label().into()
}

fn exa_page_badge(settings: &WebSearchSettings) -> String {
    settings.exa.connection.label().into()
}

pub(super) fn backend_picker(
    backend: SearchBackend,
    config: &Config,
    credential_store: &dyn CredentialStore,
) -> UiPicker {
    let items = match backend {
        SearchBackend::OpenAi => openai_items(config, credential_store),
        SearchBackend::Exa => exa_items(config, credential_store),
        SearchBackend::Brave => endpoint_items(
            "Brave Search API key used by the Brave backend.",
            WebSearchUrlField::BraveApiBase,
            ConfigTextKey::Brave,
            config,
            credential_store,
        ),
        SearchBackend::Firecrawl => endpoint_items(
            "Firecrawl API key used by the Firecrawl backend.",
            WebSearchUrlField::FirecrawlApiBase,
            ConfigTextKey::Firecrawl,
            config,
            credential_store,
        ),
    };
    UiPicker::config(backend_page_title(backend), items)
}

fn backend_page_title(backend: SearchBackend) -> &'static str {
    match backend {
        SearchBackend::OpenAi => "OpenAI search",
        SearchBackend::Exa => "Exa search",
        SearchBackend::Brave => "Brave search",
        SearchBackend::Firecrawl => "Firecrawl search",
    }
}

fn openai_items(config: &Config, credential_store: &dyn CredentialStore) -> Vec<PickerItem> {
    let settings = &config.web_search;
    let mut items = vec![item(
        "Connection",
        format!(
            "OpenAI API or Codex. Codex uses a fixed endpoint and does not take a custom URL. Enter cycles to {}.",
            settings.openai.connection.next().label()
        ),
        Some(settings.openai.connection.label().into()),
        WEB_SEARCH_OPENAI_CONNECTION_VALUE,
    )];
    match settings.openai.connection {
        OpenAiSearchConnection::Api => {
            items.extend(url_rows(
                WebSearchUrlField::OpenAiApiBase,
                "Origin and reverse-proxy prefix for OpenAI search. Empty uses the default.",
                settings,
            ));
            items.push(api_key_item(
                "OpenAI API key",
                "API key for OpenAI search.",
                ConfigTextKey::OpenAiSearch,
                config,
                credential_store,
            ));
        }
        OpenAiSearchConnection::Codex => {
            items.push(item(
                "Endpoint",
                "Codex search uses this fixed endpoint. Custom URLs and OAuth host overrides are not used.",
                Some(OPENAI_CODEX_RESPONSES_URL.into()),
                WEB_SEARCH_ROUTE_VALUE,
            ));
        }
    }
    items
}

fn exa_items(config: &Config, credential_store: &dyn CredentialStore) -> Vec<PickerItem> {
    let settings = &config.web_search;
    let mut items = vec![item(
        "Connection",
        format!(
            "Exa API or Exa MCP. A stored API key does not select MCP. Enter cycles to {}.",
            settings.exa.connection.next().label()
        ),
        Some(settings.exa.connection.label().into()),
        WEB_SEARCH_EXA_CONNECTION_VALUE,
    )];
    items.extend(url_rows(
        WebSearchUrlField::ExaApiBase,
        "Origin and reverse-proxy prefix for Exa API search. Empty uses the default.",
        settings,
    ));
    items.extend(url_rows(
        WebSearchUrlField::ExaMcp,
        "Distinct MCP URL for Exa MCP search. Empty uses the default. Editing this does not select MCP.",
        settings,
    ));
    items.push(api_key_item(
        "Exa API key",
        "API key for Exa API search. Unused when Connection is Exa MCP.",
        ConfigTextKey::Exa,
        config,
        credential_store,
    ));
    items
}

fn endpoint_items(
    key_detail: &str,
    field: WebSearchUrlField,
    key: ConfigTextKey,
    config: &Config,
    credential_store: &dyn CredentialStore,
) -> Vec<PickerItem> {
    let mut items = url_rows(
        field,
        "Origin and reverse-proxy prefix. Empty uses the default.",
        &config.web_search,
    );
    items.push(api_key_item(
        key.label(),
        key_detail,
        key,
        config,
        credential_store,
    ));
    items
}

fn url_rows(
    field: WebSearchUrlField,
    detail: &str,
    settings: &WebSearchSettings,
) -> Vec<PickerItem> {
    vec![
        item(
            field.label(),
            detail,
            Some(url_badge(field, settings)),
            field.value(),
        ),
        item(
            &format!("Reset {}", field.label()),
            "Restore the default URL. Does not select this backend.",
            Some(if field.configured(settings).is_some() {
                "custom".into()
            } else {
                "default".into()
            }),
            field.reset_value(),
        ),
    ]
}

fn api_key_item(
    label: &str,
    detail: &str,
    key: ConfigTextKey,
    config: &Config,
    credential_store: &dyn CredentialStore,
) -> PickerItem {
    PickerItem {
        section: None,
        label: label.into(),
        detail: Some(detail.into()),
        preview: None,
        badge: Some(credential_badge(
            config,
            credential_store,
            key.web_search_credential(),
        )),
        value: key.picker_value().into(),
        selection_verb: None,
        allow_filter_completion: true,
    }
}

#[path = "web_search_config_actions.rs"]
mod actions;

#[path = "web_search_config_test.rs"]
mod connection_test;
