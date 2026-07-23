use super::{
    model_picker, provider_picker, App, Entry, PickerAction, PickerBadge, PickerBadgeTone,
    PickerItem, UiPicker,
};
use {
    crate::config::Config,
    crate::permission::PermissionMode,
    rho_providers::credentials::{
        load_web_search_api_key, CredentialResult, CredentialStore, WebSearchCredential,
    },
};
pub(super) const MODELS_CATEGORY_VALUE: &str = "config_category:models";
pub(super) const AGENT_CATEGORY_VALUE: &str = "config_category:agent";
pub(super) const CONTEXT_CATEGORY_VALUE: &str = "config_category:context";
pub(super) const TOOLS_CATEGORY_VALUE: &str = "config_category:tools";
pub(super) const PROVIDERS_CATEGORY_VALUE: &str = "config_category:providers";
pub(super) const UPDATES_CATEGORY_VALUE: &str = "config_category:updates";
pub(super) const CONVERSATION_MODEL_VALUE: &str = "conversation_model";
pub(super) const REFRESH_MODEL_LIST_VALUE: &str = "refresh_model_list";
pub(super) const PROVIDER_LOGIN_VALUE: &str = "provider_login";
pub(super) const PROVIDER_LOGOUT_VALUE: &str = "provider_logout";
pub(super) const PERMISSION_MODE_VALUE: &str = "permission_mode";
pub(super) const PERMISSION_MODE_PREFIX: &str = "permission_mode:";
pub(super) const REASONING_VALUE: &str = "reasoning";
pub(super) const SHOW_REASONING_OUTPUT_VALUE: &str = "show_reasoning_output";
pub(super) const CHECK_FOR_UPDATES_VALUE: &str = "check_for_updates";
pub(super) const ENABLE_SUBAGENTS_VALUE: &str = "enable_subagents";
pub(super) const AUTO_COMPACT_VALUE: &str = "auto_compact";
pub(super) const COMPACT_THRESHOLD_PERCENT_VALUE: &str = "compact_threshold_percent";
pub(super) const COMPACT_TARGET_PERCENT_VALUE: &str = "compact_target_percent";
pub(super) const MAX_OUTPUT_BYTES_VALUE: &str = "max_output_bytes";
pub(super) const MAX_TOOL_OUTPUT_LINES_VALUE: &str = "max_tool_output_lines";
pub(super) const WEB_SEARCH_VALUE: &str = "web_search";
pub(super) const INLINE_SHELL_VALUE: &str = "inline_shell";
pub(super) const INLINE_SHELL_PREFIX: &str = "inline_shell:";
pub(super) const WEB_SEARCH_PROVIDER_VALUE: &str = "web_search_provider";
pub(super) const WEB_SEARCH_OPENAI_KEY_VALUE: &str = "web_search_openai_api_key";
pub(super) const WEB_SEARCH_EXA_KEY_VALUE: &str = "web_search_exa_api_key";
pub(super) const WEB_SEARCH_BRAVE_KEY_VALUE: &str = "web_search_brave_api_key";

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
    }
}

fn on_off(value: bool) -> String {
    if value { "on" } else { "off" }.into()
}

/// Badge for the conversation model, shown as `alias → provider/model` when
/// the selection came from a user-defined alias so the mapping is never hidden.
fn conversation_model_badge(info: &super::RuntimeModelView, config: &Config) -> String {
    let current = rho_providers::provider::model_reference(&info.provider, &info.model);
    match config.current_model_alias() {
        Some(alias) if config.provider == info.provider && config.model == info.model => {
            format!("{alias} → {current}")
        }
        _ => current,
    }
}

pub(super) fn config_picker(info: &super::RuntimeModelView, config: &Config) -> UiPicker {
    UiPicker::new(
        "Config · saves automatically",
        "type to search settings, enter open, esc close",
        vec![
            item(
                "Models & reasoning",
                "Conversation model, reasoning level, and reasoning output.",
                Some(info.model.clone()),
                MODELS_CATEGORY_VALUE,
            ),
            item(
                "Agent behavior",
                "Permission mode and delegation.",
                Some(format!(
                    "permissions: {}",
                    info.permission_mode.as_str()
                )),
                AGENT_CATEGORY_VALUE,
            ),
            item(
                "Context & limits",
                "Auto compact, compact threshold, compact target, maximum output bytes, and tool output lines.",
                Some(if config.auto_compact {
                    format!("compacts at {}%", config.compact_threshold_percent)
                } else {
                    "auto compaction off".into()
                }),
                CONTEXT_CATEGORY_VALUE,
            ),
            item(
                "Tools",
                "Inline shell, Web search provider, and Web search API keys.",
                Some(format!(
                    "{} shell · search {}",
                    config.inline_shell, config.web_search_provider
                )),
                TOOLS_CATEGORY_VALUE,
            ),
            item(
                "Providers",
                "Manage provider access and refresh cached model lists.",
                None,
                PROVIDERS_CATEGORY_VALUE,
            ),
            item(
                "Updates",
                "Check for Rho updates at startup.",
                Some(format!(
                    "startup checks {}",
                    on_off(config.check_for_updates)
                )),
                UPDATES_CATEGORY_VALUE,
            ),
        ],
        PickerAction::Config,
    )
    .with_confirm_verb("open")
}

pub(super) fn category_picker(
    category: &str,
    info: &super::RuntimeModelView,
    config: &Config,
) -> Option<UiPicker> {
    let (title, items) = match category {
        MODELS_CATEGORY_VALUE => {
            let capabilities =
                rho_providers::model::models_dev::current_reasoning_capabilities(
                    &info.provider,
                    &info.model,
                );
            let mut items = vec![
                item(
                    "Conversation model",
                    "Model used for conversation turns. Changes apply to the next turn.",
                    Some(conversation_model_badge(info, config)),
                    CONVERSATION_MODEL_VALUE,
                ),
                item(
                    "Reasoning",
                    format!(
                        "Controls model reasoning. Enter cycles to {}.",
                        capabilities.next_level(info.reasoning)
                    ),
                    Some(info.reasoning.to_string()),
                    REASONING_VALUE,
                ),
                item(
                    "Show reasoning output",
                    "Show model reasoning text in the TUI. Applies to the next turn. Space toggles.",
                    Some(if info.show_reasoning_output {
                        "shown".into()
                    } else {
                        "hidden".into()
                    }),
                    SHOW_REASONING_OUTPUT_VALUE,
                ),
            ];
            if capabilities == rho_providers::model::ReasoningCapabilities::NotConfigurable {
                items.retain(|item| item.value != REASONING_VALUE);
            }
            ("Config / Models & reasoning", items)
        }
        AGENT_CATEGORY_VALUE => (
            "Config / Agent behavior",
            vec![
                item(
                    "Permission mode",
                    permission_mode_description(info.permission_mode),
                    Some(info.permission_mode.label().into()),
                    PERMISSION_MODE_VALUE,
                ),
                item(
                    "Delegation",
                    "Make agent tools available. Changes apply to the next session. Space toggles.",
                    Some(on_off(config.enable_subagents)),
                    ENABLE_SUBAGENTS_VALUE,
                ),
            ],
        ),
        CONTEXT_CATEGORY_VALUE => (
            "Config / Context & limits",
            vec![
                item(
                    "Auto compact",
                    "Summarize older context before the effective context limit. Space toggles.",
                    Some(on_off(config.auto_compact)),
                    AUTO_COMPACT_VALUE,
                ),
                item(
                    "Compact threshold",
                    "Percent of the effective context window that triggers auto compaction.",
                    Some(format!("{}%", config.compact_threshold_percent)),
                    COMPACT_THRESHOLD_PERCENT_VALUE,
                ),
                item(
                    "Compact target",
                    "Post-compaction target percent for text-summary compaction. Providers with native compaction use this budget only if that path falls back.",
                    Some(format!("{}%", config.compact_target_percent)),
                    COMPACT_TARGET_PERCENT_VALUE,
                ),
                item(
                    "Max output bytes",
                    "Maximum tool output retained in context. Changes apply to the next session.",
                    Some(config.max_output_bytes.to_string()),
                    MAX_OUTPUT_BYTES_VALUE,
                ),
                item(
                    "Max tool output lines",
                    "Maximum collapsed tool output lines shown in the TUI.",
                    Some(config.max_tool_output_lines.to_string()),
                    MAX_TOOL_OUTPUT_LINES_VALUE,
                ),
            ],
        ),
        TOOLS_CATEGORY_VALUE => (
            "Config / Tools",
            vec![
                item(
                    "Inline shell",
                    "Shell used by ! and !! commands.",
                    Some(config.inline_shell.clone()),
                    INLINE_SHELL_VALUE,
                ),
                item(
                    "Web search",
                    "Configure the web_search backend and API keys.",
                    Some(config.web_search_provider.to_string()),
                    WEB_SEARCH_VALUE,
                ),
            ],
        ),
        PROVIDERS_CATEGORY_VALUE => (
            "Config / Providers",
            vec![
                item(
                    "Log in to provider",
                    "Add or replace provider credentials.",
                    None,
                    PROVIDER_LOGIN_VALUE,
                ),
                item(
                    "Log out of provider",
                    "Delete stored provider credentials.",
                    None,
                    PROVIDER_LOGOUT_VALUE,
                ),
                item(
                    "Refresh model lists",
                    "Refresh cached models from configured API providers.",
                    Some("run now".into()),
                    REFRESH_MODEL_LIST_VALUE,
                ),
            ],
        ),
        UPDATES_CATEGORY_VALUE => (
            "Config / Updates",
            vec![item(
                "Check for updates",
                "Check GitHub releases at startup and show an update notice when available. Space toggles.",
                Some(on_off(config.check_for_updates)),
                CHECK_FOR_UPDATES_VALUE,
            )],
        ),
        _ => return None,
    };
    Some(UiPicker::new(
        title,
        "type to search, enter change, esc back",
        items,
        PickerAction::Config,
    ))
}

pub(super) fn is_category(value: &str) -> bool {
    matches!(
        value,
        MODELS_CATEGORY_VALUE
            | AGENT_CATEGORY_VALUE
            | CONTEXT_CATEGORY_VALUE
            | TOOLS_CATEGORY_VALUE
            | PROVIDERS_CATEGORY_VALUE
            | UPDATES_CATEGORY_VALUE
    )
}

pub(super) fn category_for_setting(value: &str) -> Option<&'static str> {
    match value {
        CONVERSATION_MODEL_VALUE | REASONING_VALUE | SHOW_REASONING_OUTPUT_VALUE => {
            Some(MODELS_CATEGORY_VALUE)
        }
        PERMISSION_MODE_VALUE | ENABLE_SUBAGENTS_VALUE => Some(AGENT_CATEGORY_VALUE),
        AUTO_COMPACT_VALUE
        | COMPACT_THRESHOLD_PERCENT_VALUE
        | COMPACT_TARGET_PERCENT_VALUE
        | MAX_OUTPUT_BYTES_VALUE
        | MAX_TOOL_OUTPUT_LINES_VALUE => Some(CONTEXT_CATEGORY_VALUE),
        INLINE_SHELL_VALUE | WEB_SEARCH_VALUE => Some(TOOLS_CATEGORY_VALUE),
        PROVIDER_LOGIN_VALUE | PROVIDER_LOGOUT_VALUE | REFRESH_MODEL_LIST_VALUE => {
            Some(PROVIDERS_CATEGORY_VALUE)
        }
        CHECK_FOR_UPDATES_VALUE => Some(UPDATES_CATEGORY_VALUE),
        _ => None,
    }
}

pub(super) fn permission_mode_picker(mode: PermissionMode) -> UiPicker {
    UiPicker::new(
        "Permission mode",
        "enter select, esc back",
        [
            PermissionMode::Auto,
            PermissionMode::Plan,
            PermissionMode::Supervised,
        ]
        .into_iter()
        .map(|candidate| PickerItem {
            section: None,
            label: candidate.label().into(),
            detail: Some(permission_mode_description(candidate).into()),
            preview: None,
            badge: (candidate == mode).then_some(PickerBadge {
                text: "selected".into(),
                tone: PickerBadgeTone::Selected,
            }),
            value: format!("{PERMISSION_MODE_PREFIX}{}", candidate.as_str()),
        })
        .collect(),
        PickerAction::Config,
    )
}

fn permission_mode_description(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Auto => "No permission checks.",
        PermissionMode::Plan => "Investigate only; writes and processes are denied.",
        PermissionMode::Supervised => "Ask before writes and processes.",
    }
}

pub(super) fn inline_shell_picker(config: &Config) -> UiPicker {
    UiPicker::new(
        "Inline shell",
        "enter select, esc back",
        super::inline_shell::available_shells(&config.inline_shell)
            .into_iter()
            .map(|shell| PickerItem {
                section: None,
                label: shell.clone(),
                detail: Some("Use this shell for inline ! and !! commands.".into()),
                preview: None,
                badge: (shell == config.inline_shell).then_some(PickerBadge {
                    text: "selected".into(),
                    tone: PickerBadgeTone::Selected,
                }),
                value: format!("{INLINE_SHELL_PREFIX}{shell}"),
            })
            .collect(),
        PickerAction::Config,
    )
}

pub(super) fn web_search_config_picker(
    config: &Config,
    credential_store: &dyn CredentialStore,
) -> UiPicker {
    UiPicker::new(
        "Web search config",
        "type regex filter, enter change, esc back",
        vec![
            PickerItem {
                section: None,
                label: "Provider".into(),
                detail: Some(format!(
                    "Backend for web_search. Current: {}; Enter cycles to {}.",
                    config.web_search_provider,
                    config.web_search_provider.next_configurable()
                )),
                preview: None,
                badge: Some(PickerBadge {
                    text: config.web_search_provider.to_string(),
                    tone: PickerBadgeTone::Selected,
                }),
                value: WEB_SEARCH_PROVIDER_VALUE.into(),
            },
            PickerItem {
                section: None,
                label: "OpenAI API key".into(),
                detail: Some("Optional key for OpenAI web search. Codex login is used automatically when available.".into()),
                preview: None,
                badge: Some(credential_badge(
                    config,
                    credential_store,
                    WebSearchCredential::OpenAi,
                )),
                value: WEB_SEARCH_OPENAI_KEY_VALUE.into(),
            },
            PickerItem {
                section: None,
                label: "Exa API key".into(),
                detail: Some("Optional Exa API key. Without one, Exa hosted MCP is used.".into()),
                preview: None,
                badge: Some(credential_badge(
                    config,
                    credential_store,
                    WebSearchCredential::Exa,
                )),
                value: WEB_SEARCH_EXA_KEY_VALUE.into(),
            },
            PickerItem {
                section: None,
                label: "Brave API key".into(),
                detail: Some("Optional Brave Search API key used by the brave backend.".into()),
                preview: None,
                badge: Some(credential_badge(
                    config,
                    credential_store,
                    WebSearchCredential::Brave,
                )),
                value: WEB_SEARCH_BRAVE_KEY_VALUE.into(),
            },
        ],
        PickerAction::Config,
    )
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
    PickerBadge {
        text: if configured {
            "set".into()
        } else {
            "unset".into()
        },
        tone: PickerBadgeTone::Selected,
    }
}

fn web_search_api_key_is_set(
    stored: CredentialResult<Option<String>>,
    legacy: Option<&str>,
) -> bool {
    let stored = stored.ok().flatten();
    stored
        .as_deref()
        .or(legacy)
        .is_some_and(|value| !value.trim().is_empty())
}

impl App {
    pub(super) fn open_config_conversation_model_picker(&mut self) {
        self.refresh_available_auths();
        let picker = model_picker::model_picker(&self.info.runtime, &self.available_auths);
        if picker.items.is_empty() {
            self.insert_entry(&Entry::Notice(
                "no cached provider models. use Config > Refresh model lists.".into(),
            ));
            self.status = "config".into();
        } else {
            self.open_child_picker(picker);
            self.status = "select model".into();
        }
    }

    pub(super) fn open_config_conversation_model_picker_during_turn(&mut self) {
        self.refresh_available_auths();
        let picker = model_picker::model_picker_during_run(
            &self.info.runtime,
            self.pending_model_selection
                .as_ref()
                .map(|pending| &pending.selection),
            &self.available_auths,
        );
        if picker.items.is_empty() {
            self.insert_entry(&Entry::Notice(
                "no cached provider models. refresh model lists after the current turn ends."
                    .into(),
            ));
            self.status = "running".into();
        } else {
            self.open_child_picker(picker);
            self.status = "select model for next turn".into();
        }
    }

    pub(super) fn open_config_refresh_model_picker(&mut self) {
        self.refresh_available_auths();
        let picker = provider_picker::refresh_model_list_picker(&self.available_auths);
        self.open_child_picker(picker);
        self.status = "select provider to refresh".into();
    }

    pub(super) fn open_config_login_picker(&mut self) {
        self.open_child_picker(provider_picker::login_group_picker());
        self.status = "select provider to login".into();
    }

    pub(super) fn open_config_logout_picker(&mut self) {
        match provider_picker::logout_provider_picker(self.credential_store.as_ref()) {
            Ok(picker) if picker.items.is_empty() => {
                self.insert_entry(&Entry::Notice(
                    "no stored provider credentials to delete".into(),
                ));
                self.status = "config".into();
            }
            Ok(picker) => {
                self.open_child_picker(picker);
                self.status = "select provider to logout".into();
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(err.to_string()));
                self.status = "provider credentials unavailable".into();
            }
        }
    }
}

#[cfg(test)]
#[path = "config_picker_tests.rs"]
mod tests;
