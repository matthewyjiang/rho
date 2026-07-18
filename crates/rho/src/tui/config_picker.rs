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
pub(super) const CONVERSATION_MODEL_VALUE: &str = "conversation_model";
pub(super) const TITLE_MODEL_VALUE: &str = "title_model";
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
pub(super) const WEB_SEARCH_BACK_VALUE: &str = "web_search_back";
pub(super) const WEB_SEARCH_PROVIDER_VALUE: &str = "web_search_provider";
pub(super) const WEB_SEARCH_OPENAI_KEY_VALUE: &str = "web_search_openai_api_key";
pub(super) const WEB_SEARCH_EXA_KEY_VALUE: &str = "web_search_exa_api_key";
pub(super) const WEB_SEARCH_BRAVE_KEY_VALUE: &str = "web_search_brave_api_key";

pub(super) fn config_picker(info: &super::TuiInfo, config: &Config) -> UiPicker {
    UiPicker::new(
        "Config",
        "type regex filter, enter change, esc cancel",
        vec![
            PickerItem {
                label: "Conversation model".into(),
                detail: Some("Model used for conversation turns. Enter to choose a model.".into()),
                preview: None,
                badge: Some(PickerBadge {
                    text: format!("{}/{}", info.provider, info.model),
                    tone: PickerBadgeTone::Selected,
                }),
                value: CONVERSATION_MODEL_VALUE.into(),
            },
            PickerItem {
                label: "Session title model".into(),
                detail: Some("Model used to generate session titles. Enter to choose a model.".into()),
                preview: None,
                badge: Some(PickerBadge {
                    text: format!(
                        "{}/{}",
                        info.title_provider.as_deref().unwrap_or(&info.provider),
                        info.title_model.as_deref().unwrap_or(&info.model)
                    ),
                    tone: PickerBadgeTone::Selected,
                }),
                value: TITLE_MODEL_VALUE.into(),
            },
            PickerItem {
                label: "Refresh model lists".into(),
                detail: Some("Refresh cached models from configured API providers.".into()),
                preview: None,
                badge: None,
                value: REFRESH_MODEL_LIST_VALUE.into(),
            },
            PickerItem {
                label: "Log in to provider".into(),
                detail: Some("Add or replace provider credentials.".into()),
                preview: None,
                badge: None,
                value: PROVIDER_LOGIN_VALUE.into(),
            },
            PickerItem {
                label: "Log out of provider".into(),
                detail: Some("Delete stored provider credentials.".into()),
                preview: None,
                badge: None,
                value: PROVIDER_LOGOUT_VALUE.into(),
            },
            PickerItem {
                label: "Permission mode".into(),
                detail: Some(permission_mode_description(info.permission_mode).into()),
                preview: None,
                badge: Some(PickerBadge {
                    text: info.permission_mode.label().into(),
                    tone: PickerBadgeTone::Selected,
                }),
                value: PERMISSION_MODE_VALUE.into(),
            },
            PickerItem {
                label: "Reasoning".into(),
                detail: Some(format!(
                    "Controls model reasoning. Current: {}; Enter cycles to {}.",
                    info.reasoning,
                    info.reasoning.next_supported(
                        rho_providers::model::models_dev::cached_reasoning_levels(
                            &info.provider,
                            &info.model,
                        )
                        .as_deref(),
                    )
                )),
                preview: None,
                badge: Some(PickerBadge {
                    text: info.reasoning.to_string(),
                    tone: PickerBadgeTone::Selected,
                }),
                value: REASONING_VALUE.into(),
            },
            PickerItem {
                label: "Show reasoning output".into(),
                detail: Some(
                    "Controls whether model reasoning text is shown in the TUI. Applies next turn."
                        .into(),
                ),
                preview: None,
                badge: Some(PickerBadge {
                    text: if info.show_reasoning_output {
                        "shown".into()
                    } else {
                        "hidden".into()
                    },
                    tone: PickerBadgeTone::Selected,
                }),
                value: SHOW_REASONING_OUTPUT_VALUE.into(),
            },
            PickerItem {
                label: "Check for updates".into(),
                detail: Some("Checks GitHub releases at startup and shows an update notice in the header when available.".into()),
                preview: None,
                badge: Some(PickerBadge {
                    text: if config.check_for_updates {
                        "on".into()
                    } else {
                        "off".into()
                    },
                    tone: PickerBadgeTone::Selected,
                }),
                value: CHECK_FOR_UPDATES_VALUE.into(),
            },
            PickerItem {
                label: "Enable delegation".into(),
                detail: Some(
                    "Controls whether agent tools are available. Applies next session.".into(),
                ),
                preview: None,
                badge: Some(PickerBadge {
                    text: if config.enable_subagents {
                        "on".into()
                    } else {
                        "off".into()
                    },
                    tone: PickerBadgeTone::Selected,
                }),
                value: ENABLE_SUBAGENTS_VALUE.into(),
            },
            PickerItem {
                label: "Auto compact".into(),
                detail: Some(
                    "Summarizes older model context before the effective context limit. Transcript history is preserved."
                        .into(),
                ),
                preview: None,
                badge: Some(PickerBadge {
                    text: if config.auto_compact {
                        "on".into()
                    } else {
                        "off".into()
                    },
                    tone: PickerBadgeTone::Selected,
                }),
                value: AUTO_COMPACT_VALUE.into(),
            },
            PickerItem {
                label: "Compact threshold".into(),
                detail: Some(
                    "Percent of the effective context window that triggers auto compaction."
                        .into(),
                ),
                preview: None,
                badge: Some(PickerBadge {
                    text: format!("{}%", config.compact_threshold_percent),
                    tone: PickerBadgeTone::Selected,
                }),
                value: COMPACT_THRESHOLD_PERCENT_VALUE.into(),
            },
            PickerItem {
                label: "Compact target".into(),
                detail: Some(
                    "Post-compaction target percent. The recent verbatim tail is chosen by token budget."
                        .into(),
                ),
                preview: None,
                badge: Some(PickerBadge {
                    text: format!("{}%", config.compact_target_percent),
                    tone: PickerBadgeTone::Selected,
                }),
                value: COMPACT_TARGET_PERCENT_VALUE.into(),
            },
            PickerItem {
                label: "Max output bytes".into(),
                detail: Some(
                    "Maximum tool output retained in context. Saved for next session.".into(),
                ),
                preview: None,
                badge: Some(PickerBadge {
                    text: config.max_output_bytes.to_string(),
                    tone: PickerBadgeTone::Selected,
                }),
                value: MAX_OUTPUT_BYTES_VALUE.into(),
            },
            PickerItem {
                label: "Max tool output lines".into(),
                detail: Some("Maximum collapsed tool output lines shown in the TUI.".into()),
                preview: None,
                badge: Some(PickerBadge {
                    text: config.max_tool_output_lines.to_string(),
                    tone: PickerBadgeTone::Selected,
                }),
                value: MAX_TOOL_OUTPUT_LINES_VALUE.into(),
            },
            PickerItem {
                label: "Inline shell".into(),
                detail: Some("Shell used by ! and !! commands. Enter to choose from shells available on PATH.".into()),
                preview: None,
                badge: Some(PickerBadge {
                    text: config.inline_shell.clone(),
                    tone: PickerBadgeTone::Selected,
                }),
                value: INLINE_SHELL_VALUE.into(),
            },
            PickerItem {
                label: "Web search".into(),
                detail: Some("Configure web_search backend and API keys.".into()),
                preview: None,
                badge: Some(PickerBadge {
                    text: config.web_search_provider.to_string(),
                    tone: PickerBadgeTone::Selected,
                }),
                value: WEB_SEARCH_VALUE.into(),
            },
        ],
        PickerAction::Config,
    )
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
                label: "Back to config".into(),
                detail: Some("Return to the main config menu.".into()),
                preview: None,
                badge: None,
                value: WEB_SEARCH_BACK_VALUE.into(),
            },
            PickerItem {
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
        let picker = model_picker::model_picker(&self.info, &self.available_auths);
        if picker.items.is_empty() {
            self.insert_entry(&Entry::Notice(
                "no cached API models. use Config > Refresh model lists after signing in.".into(),
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
            &self.info,
            self.pending_model_selection.as_ref(),
            &self.available_auths,
        );
        if picker.items.is_empty() {
            self.insert_entry(&Entry::Notice(
                "no cached API models. refresh model lists after the current turn ends.".into(),
            ));
            self.status = "running".into();
        } else {
            self.open_child_picker(picker);
            self.status = "select model for next turn".into();
        }
    }

    pub(super) fn open_config_title_model_picker(&mut self) {
        self.refresh_available_auths();
        let (provider, model, _auth) = self.title_model_selection();
        let picker = model_picker::title_model_picker(
            &provider,
            &model,
            &self.info.favorite_models,
            &self.available_auths,
        );
        if picker.items.is_empty() {
            self.insert_entry(&Entry::Notice(
                "no cached API models. use Config > Refresh model lists after signing in.".into(),
            ));
            self.status = if self.running { "running" } else { "config" }.into();
        } else {
            self.open_child_picker(picker);
            self.status = "select title model".into();
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
