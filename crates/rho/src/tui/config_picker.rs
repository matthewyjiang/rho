use super::{
    model_picker, provider_picker, App, Entry, PickerAction, PickerBadge, PickerBadgeTone,
    PickerItem, UiPicker,
};
use {
    crate::config::{Config, EditTool},
    crate::permission::PermissionMode,
    rho_providers::credentials::{
        load_web_search_api_key, CredentialResult, CredentialStore, WebSearchCredential,
    },
};
pub(super) const MODELS_CATEGORY_VALUE: &str = "config_category:models";
pub(super) const APPEARANCE_CATEGORY_VALUE: &str = "config_category:appearance";
pub(super) const AGENT_CATEGORY_VALUE: &str = "config_category:agent";
pub(super) const CONTEXT_CATEGORY_VALUE: &str = "config_category:context";
pub(super) const TOOLS_CATEGORY_VALUE: &str = "config_category:tools";
pub(super) const PROVIDERS_CATEGORY_VALUE: &str = "config_category:providers";
pub(super) const CONVERSATION_MODEL_VALUE: &str = "conversation_model";
pub(super) const REFRESH_MODEL_LIST_VALUE: &str = "refresh_model_list";
pub(super) const PROVIDER_LOGIN_VALUE: &str = "provider_login";
pub(super) const PROVIDER_LOGOUT_VALUE: &str = "provider_logout";
pub(super) const SWITCH_AUTH_MODE_VALUE: &str = "switch_auth_mode";
pub(super) const PERMISSION_MODE_VALUE: &str = "permission_mode";
pub(super) const PERMISSION_MODE_PREFIX: &str = "permission_mode:";
pub(super) const REASONING_VALUE: &str = "reasoning";
pub(super) const SHOW_REASONING_OUTPUT_VALUE: &str = "show_reasoning_output";
pub(super) const ZEN_MODE_VALUE: &str = "zen_mode";
pub(super) const THEME_VALUE: &str = "theme";
pub(super) const CHECK_FOR_UPDATES_VALUE: &str = "check_for_updates";
pub(super) const ENABLE_SUBAGENTS_VALUE: &str = "enable_subagents";
pub(super) const ADVISOR_MODE_VALUE: &str = "advisor_mode";
pub(super) const ADVISOR_MODEL_VALUE: &str = "advisor_model";
pub(super) const ADVISOR_REASONING_VALUE: &str = "advisor_reasoning";
pub(super) const PERMISSION_CLASSIFIER_MODEL_VALUE: &str = "permission_classifier_model";
pub(super) const PERMISSION_CLASSIFIER_REASONING_VALUE: &str = "permission_classifier_reasoning";
pub(super) const AUTO_COMPACT_VALUE: &str = "auto_compact";
pub(super) const COMPACT_THRESHOLD_PERCENT_VALUE: &str = "compact_threshold_percent";
pub(super) const COMPACT_TARGET_PERCENT_VALUE: &str = "compact_target_percent";
pub(super) const MAX_OUTPUT_BYTES_VALUE: &str = "max_output_bytes";
pub(super) const MAX_TOOL_OUTPUT_LINES_VALUE: &str = "max_tool_output_lines";
pub(super) const PROMPT_HISTORY_LIMIT_VALUE: &str = "prompt_history_limit";
pub(super) const CLEAR_PROMPT_HISTORY_VALUE: &str = "clear_prompt_history";
pub(super) const WEB_SEARCH_VALUE: &str = "web_search";
pub(super) const INLINE_SHELL_VALUE: &str = "inline_shell";
pub(super) const INLINE_SHELL_PREFIX: &str = "inline_shell:";
pub(super) const EDIT_TOOL_VALUE: &str = "edit_tool";
pub(super) const EDIT_TOOL_PREFIX: &str = "edit_tool:";
pub(super) const WEB_SEARCH_HOSTED_VALUE: &str = "web_search_hosted";
pub(super) const WEB_SEARCH_PROVIDER_VALUE: &str = "web_search_provider";
pub(super) const WEB_SEARCH_OPENAI_KEY_VALUE: &str = "web_search_openai_api_key";
pub(super) const WEB_SEARCH_EXA_KEY_VALUE: &str = "web_search_exa_api_key";
pub(super) const WEB_SEARCH_BRAVE_KEY_VALUE: &str = "web_search_brave_api_key";
pub(super) const XAI_IMAGE_GENERATION_VALUE: &str = "xai_image_generation";

fn xai_image_generation_visible(provider: &str) -> bool {
    provider == "xai"
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
    }
}

fn on_off(value: bool) -> String {
    if value { "on" } else { "off" }.into()
}

fn theme_badge(config: &Config) -> String {
    super::theme::theme_display_name(&config.theme)
}

/// Advisor badge: the mode, plus the advisor model once one is selected.
fn advisor_mode_badge(config: &Config, info: &super::RuntimeModelView) -> String {
    super::advisor_status::AdvisorStatus::new(
        config.advisor_mode,
        info.internal_agents.get(crate::agent::ADVISOR_AGENT_ID),
    )
    .badge()
}

fn advisor_model_badge(info: &super::RuntimeModelView) -> String {
    match info.internal_agents.get(crate::agent::ADVISOR_AGENT_ID) {
        Some(selection) => selection.display_reference(),
        None => "not selected".into(),
    }
}

fn advisor_reasoning_row(info: &super::RuntimeModelView) -> Option<(String, String, String)> {
    let selection = info.internal_agents.get(crate::agent::ADVISOR_AGENT_ID)?;
    let capabilities = crate::agent::internal_agent_reasoning_capabilities(selection);
    if capabilities == rho_providers::model::ReasoningCapabilities::NotConfigurable {
        return None;
    }
    let current = crate::tools::advisor::advisor_effective_reasoning(selection);
    let next = capabilities.next_level(current);
    Some((
        "Advisor reasoning".into(),
        format!("Controls advisor model reasoning. Enter cycles to {next}."),
        current.to_string(),
    ))
}

fn permission_classifier_model_badge(info: &super::RuntimeModelView) -> String {
    match info
        .internal_agents
        .get(crate::agent::PERMISSION_CLASSIFIER_AGENT_ID)
    {
        Some(selection) => selection.display_reference(),
        None => "not selected".into(),
    }
}

fn permission_classifier_reasoning_row(
    info: &super::RuntimeModelView,
) -> Option<(String, String, String)> {
    let selection = info
        .internal_agents
        .get(crate::agent::PERMISSION_CLASSIFIER_AGENT_ID)?;
    let capabilities = crate::agent::internal_agent_reasoning_capabilities(selection);
    if capabilities == rho_providers::model::ReasoningCapabilities::NotConfigurable {
        return None;
    }
    let current = crate::agent::effective_internal_agent_reasoning(
        crate::agent::PERMISSION_CLASSIFIER_AGENT_ID,
        selection,
    );
    let next = capabilities.next_level(current);
    Some((
        "Permission classifier reasoning".into(),
        format!("Controls the Auto permission classifier model reasoning. Enter cycles to {next}."),
        current.to_string(),
    ))
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
        vec![
            item(
                "Models",
                "Conversation model and reasoning level.",
                Some(info.model.clone()),
                MODELS_CATEGORY_VALUE,
            ),
            item(
                "Appearance",
                "Theme, zen mode, reasoning output, and collapsed tool output lines.",
                Some(theme_badge(config)),
                APPEARANCE_CATEGORY_VALUE,
            ),
            item(
                "Agent behavior",
                "Permission mode, classifier model, advisor, and delegation.",
                Some(format!("permissions: {}", info.permission_mode.as_str())),
                AGENT_CATEGORY_VALUE,
            ),
            item(
                "Context & limits",
                "Auto compact, compact threshold, compact target, max output bytes, and prompt history.",
                Some(if config.auto_compact {
                    format!("compacts at {}%", config.compact_threshold_percent)
                } else {
                    "auto compaction off".into()
                }),
                CONTEXT_CATEGORY_VALUE,
            ),
            item(
                "Tools",
                "Inline shell, edit tool, and web search (hosted + backup).",
                Some(tools_summary(info, config)),
                TOOLS_CATEGORY_VALUE,
            ),
            item(
                "Providers",
                "Provider login, logout, auth mode, refresh model lists, and startup update checks.",
                None,
                PROVIDERS_CATEGORY_VALUE,
            ),
        ],
        PickerAction::Config,
    )
    .with_confirm_verb("open")
}

fn tools_summary(info: &super::RuntimeModelView, config: &Config) -> String {
    let summary = format!(
        "{} shell · {} · {}",
        config.inline_shell,
        config.edit_tool.display_label(&info.provider),
        web_search_summary(config)
    );
    if xai_image_generation_visible(&info.provider) {
        format!(
            "{summary} · image gen {}",
            on_off(config.xai_image_generation)
        )
    } else {
        summary
    }
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
            ];
            if capabilities == rho_providers::model::ReasoningCapabilities::NotConfigurable {
                items.retain(|item| item.value != REASONING_VALUE);
            }
            ("Config / Models", items)
        }
        APPEARANCE_CATEGORY_VALUE => (
            "Config / Appearance",
            vec![
                item(
                    "Theme",
                    "Color theme for the interactive TUI. Enter opens a preview picker. Default matches the host terminal.",
                    Some(theme_badge(config)),
                    THEME_VALUE,
                ),
                item(
                    "Zen mode",
                    "Show only message text. Hides tool cards, reasoning, and the Thinking... placeholder. Keeps the activity rail. Space toggles.",
                    Some(on_off(info.zen_mode)),
                    ZEN_MODE_VALUE,
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
                item(
                    "Max tool output lines",
                    "Maximum collapsed tool output lines shown in the TUI.",
                    Some(config.max_tool_output_lines.to_string()),
                    MAX_TOOL_OUTPUT_LINES_VALUE,
                ),
            ],
        ),
        AGENT_CATEGORY_VALUE => {
            let mut items = vec![
                sectioned_item(
                    Some("Permissions"),
                    "Permission mode",
                    permission_mode_description(info.permission_mode),
                    Some(info.permission_mode.label().into()),
                    PERMISSION_MODE_VALUE,
                ),
                sectioned_item(
                    Some("Permissions"),
                    "Permission classifier model",
                    "Model used by Auto permission mode to review writes and processes. Enter opens a picker.",
                    Some(permission_classifier_model_badge(info)),
                    PERMISSION_CLASSIFIER_MODEL_VALUE,
                ),
            ];
            if let Some((label, detail, badge_text)) = permission_classifier_reasoning_row(info) {
                items.push(sectioned_item(
                    Some("Permissions"),
                    &label,
                    detail,
                    Some(badge_text),
                    PERMISSION_CLASSIFIER_REASONING_VALUE,
                ));
            }
            items.push(sectioned_item(
                Some("Advisor"),
                "Advisor mode",
                "Let the agent ask an advisor model to review the session. Needs an advisor model; turning it on picks one. Space toggles.",
                Some(advisor_mode_badge(config, info)),
                ADVISOR_MODE_VALUE,
            ));
            items.push(sectioned_item(
                Some("Advisor"),
                "Advisor model",
                "Model used by the advisor tool. Enter opens a picker. Reasoning is set below when the model supports it.",
                Some(advisor_model_badge(info)),
                ADVISOR_MODEL_VALUE,
            ));
            if let Some((label, detail, badge_text)) = advisor_reasoning_row(info) {
                items.push(sectioned_item(
                    Some("Advisor"),
                    &label,
                    detail,
                    Some(badge_text),
                    ADVISOR_REASONING_VALUE,
                ));
            }
            items.push(item(
                "Delegation",
                "Make agent tools available. Changes apply to the next session. Space toggles.",
                Some(on_off(config.enable_subagents)),
                ENABLE_SUBAGENTS_VALUE,
            ));
            ("Config / Agent behavior", items)
        }
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
                    "Prompt history limit",
                    "Saved composer prompts kept for up-arrow recall across sessions. 0 disables saving. Lowering the cap deletes older saved prompts after confirm.",
                    Some(config.prompt_history_limit.to_string()),
                    PROMPT_HISTORY_LIMIT_VALUE,
                ),
                item(
                    "Clear prompt history",
                    "Permanently delete every saved composer prompt used by up-arrow recall.",
                    Some("run now".into()),
                    CLEAR_PROMPT_HISTORY_VALUE,
                ),
            ],
        ),
        TOOLS_CATEGORY_VALUE => {
            let mut items = vec![
                item(
                    "Inline shell",
                    "Shell used by ! and !! commands.",
                    Some(config.inline_shell.clone()),
                    INLINE_SHELL_VALUE,
                ),
                item(
                    "Edit tool",
                    "File edit format exposed to models. Auto follows the active provider.",
                    Some(config.edit_tool.display_label(&info.provider)),
                    EDIT_TOOL_VALUE,
                ),
                item(
                    "Web search",
                    "Hosted search when supported; backup client backend and API keys.",
                    Some(web_search_summary(config)),
                    WEB_SEARCH_VALUE,
                ),
            ];
            if xai_image_generation_visible(&info.provider) {
                items.push(item(
                    "xAI image generation",
                    "Attach xAI hosted image_generation on create turns. Space or Enter toggles. Applies to the next session.",
                    Some(on_off(config.xai_image_generation)),
                    XAI_IMAGE_GENERATION_VALUE,
                ));
            }
            ("Config / Tools", items)
        }
        PROVIDERS_CATEGORY_VALUE => {
            let mut items = vec![
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
            ];
            if rho_providers::provider::provider_descriptor(&info.provider)
                .is_some_and(|descriptor| descriptor.auth_modes().count() > 1)
            {
                items.push(item(
                    "Switch active auth mode",
                    "Use another available credential for the active provider.",
                    Some(info.auth.clone()),
                    SWITCH_AUTH_MODE_VALUE,
                ));
            }
            items.push(item(
                "Refresh model lists",
                "Refresh cached models from configured API providers.",
                Some("run now".into()),
                REFRESH_MODEL_LIST_VALUE,
            ));
            items.push(item(
                "Check for updates",
                "Check GitHub releases at startup and show an update notice when available. Space toggles.",
                Some(on_off(config.check_for_updates)),
                CHECK_FOR_UPDATES_VALUE,
            ));
            ("Config / Providers", items)
        }
        _ => return None,
    };
    Some(UiPicker::new(title, items, PickerAction::Config))
}

pub(super) fn is_category(value: &str) -> bool {
    matches!(
        value,
        MODELS_CATEGORY_VALUE
            | APPEARANCE_CATEGORY_VALUE
            | AGENT_CATEGORY_VALUE
            | CONTEXT_CATEGORY_VALUE
            | TOOLS_CATEGORY_VALUE
            | PROVIDERS_CATEGORY_VALUE
    )
}

pub(super) fn category_for_setting(value: &str) -> Option<&'static str> {
    match value {
        CONVERSATION_MODEL_VALUE | REASONING_VALUE => Some(MODELS_CATEGORY_VALUE),
        SHOW_REASONING_OUTPUT_VALUE
        | ZEN_MODE_VALUE
        | THEME_VALUE
        | MAX_TOOL_OUTPUT_LINES_VALUE => Some(APPEARANCE_CATEGORY_VALUE),
        PERMISSION_MODE_VALUE
        | PERMISSION_CLASSIFIER_MODEL_VALUE
        | PERMISSION_CLASSIFIER_REASONING_VALUE
        | ENABLE_SUBAGENTS_VALUE
        | ADVISOR_MODE_VALUE
        | ADVISOR_MODEL_VALUE
        | ADVISOR_REASONING_VALUE => Some(AGENT_CATEGORY_VALUE),
        AUTO_COMPACT_VALUE
        | COMPACT_THRESHOLD_PERCENT_VALUE
        | COMPACT_TARGET_PERCENT_VALUE
        | MAX_OUTPUT_BYTES_VALUE
        | PROMPT_HISTORY_LIMIT_VALUE
        | CLEAR_PROMPT_HISTORY_VALUE => Some(CONTEXT_CATEGORY_VALUE),
        INLINE_SHELL_VALUE | EDIT_TOOL_VALUE | WEB_SEARCH_VALUE | XAI_IMAGE_GENERATION_VALUE => {
            Some(TOOLS_CATEGORY_VALUE)
        }
        PROVIDER_LOGIN_VALUE
        | PROVIDER_LOGOUT_VALUE
        | SWITCH_AUTH_MODE_VALUE
        | REFRESH_MODEL_LIST_VALUE
        | CHECK_FOR_UPDATES_VALUE => Some(PROVIDERS_CATEGORY_VALUE),
        _ => None,
    }
}

pub(super) fn permission_mode_picker(mode: PermissionMode) -> UiPicker {
    UiPicker::new(
        "Permission mode",
        PermissionMode::ALL
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
                selection_verb: None,
            })
            .collect(),
        PickerAction::Config,
    )
}

fn permission_mode_description(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Bypass => "No permission checks.",
        PermissionMode::Auto => {
            "Classifier reviews new files and processes; tracked and already-approved workspace edits are free."
        }
        PermissionMode::AllowEdits => {
            "Tracked and already-approved workspace edits are free; ask before new files and processes."
        }
        PermissionMode::Plan => "Investigate only; writes and processes are denied.",
        PermissionMode::Supervised => "Ask before writes and processes.",
    }
}

pub(super) fn inline_shell_picker(config: &Config) -> UiPicker {
    UiPicker::new(
        "Inline shell",
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
                selection_verb: None,
            })
            .collect(),
        PickerAction::Config,
    )
}

pub(super) fn edit_tool_picker(selected: EditTool) -> UiPicker {
    UiPicker::new(
        "Edit tool",
        EditTool::all()
            .into_iter()
            .map(|edit_tool| PickerItem {
                section: None,
                label: edit_tool.label().into(),
                detail: Some(edit_tool.detail().into()),
                preview: None,
                badge: (edit_tool == selected).then_some(PickerBadge {
                    text: "selected".into(),
                    tone: PickerBadgeTone::Selected,
                }),
                value: format!("{EDIT_TOOL_PREFIX}{}", edit_tool.as_str()),
                selection_verb: None,
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
        vec![
            PickerItem {
                section: None,
                label: "Hosted search".into(),
                detail: Some(
                    "Use the chat provider's native web_search when supported. Space or Enter toggles."
                        .into(),
                ),
                preview: None,
                badge: Some(PickerBadge {
                    text: on_off(config.web_search_hosted),
                    tone: PickerBadgeTone::Selected,
                }),
                value: WEB_SEARCH_HOSTED_VALUE.into(),
                selection_verb: None,
            },
            PickerItem {
                section: None,
                label: "Backup provider".into(),
                detail: Some(format!(
                    "Client web_search backend when hosted search is off or unsupported. Current: {}; Enter cycles to {}.",
                    config.web_search_provider,
                    config.web_search_provider.next_configurable()
                )),
                preview: None,
                badge: Some(PickerBadge {
                    text: config.web_search_provider.to_string(),
                    tone: PickerBadgeTone::Selected,
                }),
                value: WEB_SEARCH_PROVIDER_VALUE.into(),
                selection_verb: None,
            },
            PickerItem {
                section: None,
                label: "OpenAI API key".into(),
                detail: Some("Optional key for the OpenAI backup search backend.".into()),
                preview: None,
                badge: Some(credential_badge(
                    config,
                    credential_store,
                    WebSearchCredential::OpenAi,
                )),
                value: WEB_SEARCH_OPENAI_KEY_VALUE.into(),
                selection_verb: None,
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
                selection_verb: None,
            },
            PickerItem {
                section: None,
                label: "Brave API key".into(),
                detail: Some("Optional Brave Search API key used by the brave backup backend.".into()),
                preview: None,
                badge: Some(credential_badge(
                    config,
                    credential_store,
                    WebSearchCredential::Brave,
                )),
                value: WEB_SEARCH_BRAVE_KEY_VALUE.into(),
                selection_verb: None,
            },
        ],
        PickerAction::Config,
    )
}

fn web_search_summary(config: &Config) -> String {
    if !crate::tools::web::web_search_available(config) {
        return if config.web_search_hosted
            && !crate::tools::web::supports_hosted_web_search(&config.provider, &config.model)
        {
            format!(
                "unavailable (hosted unsupported, backup {})",
                config.web_search_provider
            )
        } else {
            format!(
                "unavailable (hosted off, backup {})",
                config.web_search_provider
            )
        };
    }
    let hosted = if crate::tools::web::hosted_web_search_active(config) {
        "hosted active"
    } else if config.web_search_hosted {
        "hosted unsupported"
    } else {
        "hosted off"
    };
    format!("{hosted}, backup {}", config.web_search_provider)
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
            self.set_status("no cached provider models. use Config > Refresh model lists.");
        } else {
            self.open_child_picker(picker);
            self.set_status("select model");
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
            self.set_status(
                "no cached provider models. refresh model lists after the current turn ends.",
            );
        } else {
            self.open_child_picker(picker);
            self.set_status("select model for next turn");
        }
    }

    pub(super) fn open_config_refresh_model_picker(&mut self) {
        self.refresh_available_auths();
        let picker = provider_picker::refresh_model_list_picker(&self.available_auths);
        self.open_child_picker(picker);
        self.set_status("select provider to refresh");
    }

    pub(super) fn open_config_login_picker(&mut self) {
        self.open_child_picker(provider_picker::login_group_picker());
        self.set_status("select provider to login");
    }

    pub(super) fn open_config_auth_mode_picker(&mut self) -> anyhow::Result<()> {
        match provider_picker::auth_mode_picker(
            self.credential_store.as_ref(),
            &self.info.runtime.provider,
            &self.info.runtime.auth,
        ) {
            Ok(picker)
                if !picker
                    .items
                    .iter()
                    .any(|item| item.value != self.info.runtime.auth) =>
            {
                self.set_status(format!(
                    "{} does not have another available auth mode. Log in to another mode first.",
                    self.info.runtime.provider
                ));
            }
            Ok(picker) => {
                self.open_child_picker(picker);
                self.set_status("select active auth mode");
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not check provider credentials: {err}"
                )));
                self.set_status("provider credentials unavailable");
            }
        }
        Ok(())
    }

    pub(super) async fn open_config_logout_picker(&mut self) -> anyhow::Result<()> {
        let claude_signed_in = Self::claude_signed_in().await;
        match provider_picker::logout_provider_picker(
            self.credential_store.as_ref(),
            claude_signed_in,
        ) {
            Ok(picker) if picker.items.is_empty() => {
                self.set_status("no stored provider credentials to delete");
            }
            Ok(picker) => {
                self.open_child_picker(picker);
                self.set_status("select provider to logout");
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(err.to_string()));
                self.set_status("provider credentials unavailable");
            }
        }
        Ok(())
    }
}
