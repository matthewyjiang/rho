//! Web-search `/config` policy: pickers, routing copy, URL vs secret editors.
//!
//! Generic picker rendering stays in the shared picker. This module owns the
//! Mode / backend / endpoint rows and must not treat configuring a backend as
//! selecting it. Mode, backend, and connection rows open choice pickers.
//! OpenAI and Exa pages always show both connection settings.

use rho_providers::credentials::{
    load_web_search_api_key, CredentialResult, CredentialStore, WebSearchCredential,
};

use crate::config::{
    parse_search_endpoint_url, web_search_route, Config, ExaSearchConnection,
    OpenAiSearchConnection, SearchBackend, WebSearchMode, WebSearchRoute, WebSearchSettings,
    OPENAI_CODEX_RESPONSES_URL,
};

use super::{
    config_row::ConfigCommitCtx,
    picker::{PickerBadge, PickerBadgeTone, PickerItem, UiPicker},
    App, ComposerMode, Entry, InlineChoice, InlineChoiceModal, InlineChoiceOption,
    InlineChoicePending,
};

pub(super) const WEB_SEARCH_MODE_VALUE: &str = "web_search_mode";
pub(super) const WEB_SEARCH_MODE_PREFIX: &str = "web_search_mode:";
pub(super) const WEB_SEARCH_BACKEND_VALUE: &str = "web_search_backend";
pub(super) const WEB_SEARCH_BACKEND_PREFIX: &str = "web_search_backend:";
pub(super) const WEB_SEARCH_ROUTE_VALUE: &str = "web_search_route";
pub(super) const WEB_SEARCH_TEST_VALUE: &str = "web_search_test";
pub(super) const WEB_SEARCH_OPENAI_PAGE_VALUE: &str = "web_search_openai";
pub(super) const WEB_SEARCH_EXA_PAGE_VALUE: &str = "web_search_exa";
pub(super) const WEB_SEARCH_BRAVE_PAGE_VALUE: &str = "web_search_brave";
pub(super) const WEB_SEARCH_FIRECRAWL_PAGE_VALUE: &str = "web_search_firecrawl";
pub(super) const WEB_SEARCH_CODEX_ENDPOINT_VALUE: &str = "web_search_openai_codex_endpoint";
pub(super) const WEB_SEARCH_OPENAI_CONNECTION_VALUE: &str = "web_search_openai_connection";
pub(super) const WEB_SEARCH_OPENAI_CONNECTION_PREFIX: &str = "web_search_openai_connection:";
pub(super) const WEB_SEARCH_EXA_CONNECTION_VALUE: &str = "web_search_exa_connection";
pub(super) const WEB_SEARCH_EXA_CONNECTION_PREFIX: &str = "web_search_exa_connection:";
pub(super) const WEB_SEARCH_OPENAI_KEY_VALUE: &str = "web_search_openai_api_key";
pub(super) const WEB_SEARCH_EXA_KEY_VALUE: &str = "web_search_exa_api_key";
pub(super) const WEB_SEARCH_BRAVE_KEY_VALUE: &str = "web_search_brave_api_key";
pub(super) const WEB_SEARCH_FIRECRAWL_KEY_VALUE: &str = "web_search_firecrawl_api_key";

const TEST_CONFIRM_VALUE: &str = "confirm";

pub(super) use fields::{WebSearchAction, WebSearchChoice, WebSearchChoiceKind, WebSearchUrlField};
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
            let destination = settings.destination(backend);
            match destination.resolve_path("") {
                Ok(url) => format!(
                    "{} · {url} · {}",
                    destination.label(),
                    if destination.configured().is_some() {
                        "custom"
                    } else {
                        "default"
                    }
                ),
                Err(error) => format!("{}: {error}", destination.label()),
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
    sectioned_item(None, label, detail, badge_text, value)
}

fn sectioned_item(
    section: Option<&str>,
    label: &str,
    detail: impl Into<String>,
    badge_text: Option<String>,
    value: &str,
) -> PickerItem {
    PickerItem {
        section: section.map(str::to_string),
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

pub(super) fn main_picker(config: &Config, provider: &str, model: &str) -> UiPicker {
    let settings = &config.web_search;
    let route = effective_route(config, provider, model);
    UiPicker::config(
        "Web search",
        vec![
            item(
                "Mode",
                "Auto uses native search when the current model supports it, otherwise the selected backend. Backend always uses that backend. Off disables search. Enter opens a picker.",
                Some(settings.mode.label().into()),
                WEB_SEARCH_MODE_VALUE,
            ),
            item(
                "Search backend",
                "Client backend used when Mode is Backend, or when Mode is Auto and native search is not selected. Configuring a backend does not select it. Enter opens a picker.",
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
                Some(settings.openai.connection.label().into()),
                WEB_SEARCH_OPENAI_PAGE_VALUE,
            ),
            item(
                "Exa",
                "API or MCP connection, endpoints, and API key. Opening this page does not select Exa.",
                Some(settings.exa.connection.label().into()),
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
            config,
            credential_store,
        ),
        SearchBackend::Firecrawl => endpoint_items(
            "Firecrawl API key used by the Firecrawl backend.",
            WebSearchUrlField::FirecrawlApiBase,
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

fn choice_item(label: &str, detail: &str, value: String, selected: bool) -> PickerItem {
    item(label, detail, selected.then(|| "selected".into()), &value)
}

fn with_current_selection(mut picker: UiPicker, value: &str) -> UiPicker {
    if let Some(index) = picker.items.iter().position(|item| item.value == value) {
        picker.selected = index;
    }
    picker
}

fn choice_picker<T: Copy + PartialEq>(
    kind: WebSearchChoiceKind,
    current: T,
    options: impl IntoIterator<Item = T>,
    label: impl Fn(T) -> &'static str,
    detail: impl Fn(T) -> &'static str,
    as_str: impl Fn(T) -> &'static str,
) -> UiPicker {
    let current_value = format!("{}{}", kind.prefix(), as_str(current));
    with_current_selection(
        UiPicker::config(
            kind.title(),
            options
                .into_iter()
                .map(|option| {
                    choice_item(
                        label(option),
                        detail(option),
                        format!("{}{}", kind.prefix(), as_str(option)),
                        option == current,
                    )
                })
                .collect(),
        ),
        &current_value,
    )
}

fn picker_for_choice(kind: WebSearchChoiceKind, settings: &WebSearchSettings) -> UiPicker {
    match kind {
        WebSearchChoiceKind::Mode => choice_picker(
            kind,
            settings.mode,
            WebSearchMode::ALL,
            WebSearchMode::label,
            |mode| {
                match mode {
                WebSearchMode::Auto => {
                    "Native search when the current model supports it, otherwise the selected backend."
                }
                WebSearchMode::Backend => {
                    "Always the selected backend, even when native search is supported."
                }
                WebSearchMode::Off => "Do not search.",
            }
            },
            WebSearchMode::as_str,
        ),
        WebSearchChoiceKind::Backend => choice_picker(
            kind,
            settings.backend,
            SearchBackend::ALL,
            SearchBackend::label,
            |backend| {
                match backend {
                SearchBackend::OpenAi => {
                    "OpenAI API or Codex. Configuring OpenAI does not select it."
                }
                SearchBackend::Exa => "Exa API or Exa MCP. Configuring Exa does not select it.",
                SearchBackend::Brave => {
                    "Brave Search API. Configuring Brave does not select it."
                }
                SearchBackend::Firecrawl => {
                    "Firecrawl Search API, including self-hosted deployments. Configuring Firecrawl does not select it."
                }
            }
            },
            SearchBackend::as_str,
        ),
        WebSearchChoiceKind::OpenAiConnection => choice_picker(
            kind,
            settings.openai.connection,
            OpenAiSearchConnection::ALL,
            OpenAiSearchConnection::label,
            |connection| match connection {
                OpenAiSearchConnection::Api => {
                    "Uses the OpenAI API base URL and API key on this page."
                }
                OpenAiSearchConnection::Codex => {
                    "Uses the Codex endpoint on this page and ChatGPT login."
                }
            },
            OpenAiSearchConnection::as_str,
        ),
        WebSearchChoiceKind::ExaConnection => choice_picker(
            kind,
            settings.exa.connection,
            ExaSearchConnection::ALL,
            ExaSearchConnection::label,
            |connection| match connection {
                ExaSearchConnection::Api => "Uses the Exa API base URL and API key on this page.",
                ExaSearchConnection::Mcp => {
                    "Uses the Exa MCP URL on this page. A stored API key does not select MCP."
                }
            },
            ExaSearchConnection::as_str,
        ),
    }
}

fn openai_items(config: &Config, credential_store: &dyn CredentialStore) -> Vec<PickerItem> {
    let settings = &config.web_search;
    let mut items = vec![item(
        "Connection",
        "Chooses Codex or OpenAI API. Both stay on this page. Enter opens a picker.",
        Some(settings.openai.connection.label().into()),
        WEB_SEARCH_OPENAI_CONNECTION_VALUE,
    )];
    items.push(sectioned_item(
        Some("Codex"),
        "Endpoint",
        "Used when Connection is Codex. Fixed ChatGPT endpoint; custom URLs never receive Codex tokens.",
        Some(OPENAI_CODEX_RESPONSES_URL.into()),
        WEB_SEARCH_CODEX_ENDPOINT_VALUE,
    ));
    items.extend(url_rows(
        WebSearchUrlField::OpenAiApiBase,
        "Used when Connection is OpenAI API. Origin and reverse-proxy prefix. Empty uses the default.",
        settings,
        Some("OpenAI API"),
    ));
    items.push(api_key_item(
        "OpenAI API key",
        "Used when Connection is OpenAI API.",
        WebSearchCredential::OpenAi,
        config,
        credential_store,
        Some("OpenAI API"),
    ));
    items
}

fn exa_items(config: &Config, credential_store: &dyn CredentialStore) -> Vec<PickerItem> {
    let settings = &config.web_search;
    let mut items = vec![item(
        "Connection",
        "Chooses Exa API or Exa MCP. Both stay on this page. Enter opens a picker.",
        Some(settings.exa.connection.label().into()),
        WEB_SEARCH_EXA_CONNECTION_VALUE,
    )];
    items.extend(url_rows(
        WebSearchUrlField::ExaApiBase,
        "Used when Connection is Exa API. Origin and reverse-proxy prefix. Empty uses the default.",
        settings,
        Some("Exa API"),
    ));
    items.push(api_key_item(
        "Exa API key",
        "Used when Connection is Exa API. A stored API key does not select MCP.",
        WebSearchCredential::Exa,
        config,
        credential_store,
        Some("Exa API"),
    ));
    items.extend(url_rows(
        WebSearchUrlField::ExaMcp,
        "Used when Connection is Exa MCP. Distinct from the Exa HTTP API. Empty uses the default. Editing this does not select MCP.",
        settings,
        Some("Exa MCP"),
    ));
    items
}

fn endpoint_items(
    key_detail: &str,
    field: WebSearchUrlField,
    config: &Config,
    credential_store: &dyn CredentialStore,
) -> Vec<PickerItem> {
    let key = field.page().credential();
    let mut items = url_rows(
        field,
        "Origin and reverse-proxy prefix. Empty uses the default.",
        &config.web_search,
        None,
    );
    items.push(api_key_item(
        key.label(),
        key_detail,
        key,
        config,
        credential_store,
        None,
    ));
    items
}

fn url_rows(
    field: WebSearchUrlField,
    detail: &str,
    settings: &WebSearchSettings,
    section: Option<&str>,
) -> Vec<PickerItem> {
    vec![
        sectioned_item(
            section,
            field.label(),
            detail,
            Some(url_badge(field, settings)),
            field.value(),
        ),
        sectioned_item(
            section,
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
    credential: WebSearchCredential,
    config: &Config,
    credential_store: &dyn CredentialStore,
    section: Option<&str>,
) -> PickerItem {
    let mut item = sectioned_item(
        section,
        label,
        detail,
        None,
        web_search_key_value(credential),
    );
    item.badge = Some(credential_badge(config, credential_store, credential));
    item
}

pub(super) fn web_search_key_value(credential: WebSearchCredential) -> &'static str {
    match credential {
        WebSearchCredential::OpenAi => WEB_SEARCH_OPENAI_KEY_VALUE,
        WebSearchCredential::Exa => WEB_SEARCH_EXA_KEY_VALUE,
        WebSearchCredential::Brave => WEB_SEARCH_BRAVE_KEY_VALUE,
        WebSearchCredential::Firecrawl => WEB_SEARCH_FIRECRAWL_KEY_VALUE,
    }
}

#[path = "web_search_config_actions.rs"]
mod actions;

#[path = "web_search_config_test.rs"]
mod connection_test;

#[cfg(test)]
#[path = "web_search_config_picker_tests.rs"]
mod picker_tests;
