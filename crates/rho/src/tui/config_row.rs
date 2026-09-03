//! Typed `/config` picker rows.
//!
//! Parse from the existing picker string values so the generic picker can keep
//! `PickerItem.value` as a [`String`].

use ratatui::DefaultTerminal;

use super::{
    config_editor::{ConfigNumberKey, ConfigTextKey},
    config_picker, InteractiveRuntime,
};

/// Idle vs during-turn commit context for one config row.
pub(super) enum ConfigCommitCtx<'a> {
    Idle {
        agent: &'a mut InteractiveRuntime,
        terminal: &'a mut DefaultTerminal,
    },
    DuringTurn,
}

/// Every `/config` picker value currently dispatched by the TUI.
#[derive(Debug)]
pub(super) enum ConfigRow {
    Category(String),
    ConversationModel,
    RefreshModelList,
    RefreshModelsDev,
    ProviderLogin,
    ProviderLogout,
    SwitchAuthMode,
    PermissionMode,
    PermissionModeChoice(String),
    Reasoning,
    ShowReasoningOutput,
    ZenMode,
    Theme,
    CheckForUpdates,
    EnableSubagents,
    AdvisorMode,
    AdvisorModel,
    AdvisorReasoning,
    PermissionClassifierModel,
    PermissionClassifierReasoning,
    AutoCompact,
    CacheMissNotices,
    Number(ConfigNumberKey),
    ClearPromptHistory,
    InlineShell,
    EditTool,
    EditToolChoice(String),
    InlineShellChoice(String),
    WebSearch,
    WebSearchHosted,
    WebSearchProvider,
    WebSearchApiKey(ConfigTextKey),
    XaiImageGeneration,
}

impl ConfigRow {
    pub(super) fn parse(value: &str) -> Option<Self> {
        if config_picker::is_category(value) {
            return Some(Self::Category(value.to_string()));
        }
        if let Some(mode) = value.strip_prefix(config_picker::PERMISSION_MODE_PREFIX) {
            return Some(Self::PermissionModeChoice(mode.to_string()));
        }
        if let Some(tool) = value.strip_prefix(config_picker::EDIT_TOOL_PREFIX) {
            return Some(Self::EditToolChoice(tool.to_string()));
        }
        if let Some(shell) = value.strip_prefix(config_picker::INLINE_SHELL_PREFIX) {
            return Some(Self::InlineShellChoice(shell.to_string()));
        }
        Some(match value {
            config_picker::CONVERSATION_MODEL_VALUE => Self::ConversationModel,
            config_picker::REFRESH_MODEL_LIST_VALUE => Self::RefreshModelList,
            config_picker::REFRESH_MODELS_DEV_VALUE => Self::RefreshModelsDev,
            config_picker::PROVIDER_LOGIN_VALUE => Self::ProviderLogin,
            config_picker::PROVIDER_LOGOUT_VALUE => Self::ProviderLogout,
            config_picker::SWITCH_AUTH_MODE_VALUE => Self::SwitchAuthMode,
            config_picker::PERMISSION_MODE_VALUE => Self::PermissionMode,
            config_picker::REASONING_VALUE => Self::Reasoning,
            config_picker::SHOW_REASONING_OUTPUT_VALUE => Self::ShowReasoningOutput,
            config_picker::ZEN_MODE_VALUE => Self::ZenMode,
            config_picker::THEME_VALUE => Self::Theme,
            config_picker::CHECK_FOR_UPDATES_VALUE => Self::CheckForUpdates,
            config_picker::ENABLE_SUBAGENTS_VALUE => Self::EnableSubagents,
            config_picker::AGENT_CONCURRENCY_VALUE => {
                Self::Number(ConfigNumberKey::AgentConcurrency)
            }
            config_picker::ADVISOR_MODE_VALUE => Self::AdvisorMode,
            config_picker::ADVISOR_MODEL_VALUE => Self::AdvisorModel,
            config_picker::ADVISOR_REASONING_VALUE => Self::AdvisorReasoning,
            config_picker::PERMISSION_CLASSIFIER_MODEL_VALUE => Self::PermissionClassifierModel,
            config_picker::PERMISSION_CLASSIFIER_REASONING_VALUE => {
                Self::PermissionClassifierReasoning
            }
            config_picker::AUTO_COMPACT_VALUE => Self::AutoCompact,
            config_picker::CACHE_MISS_NOTICES_VALUE => Self::CacheMissNotices,
            config_picker::COMPACT_THRESHOLD_PERCENT_VALUE => {
                Self::Number(ConfigNumberKey::CompactThresholdPercent)
            }
            config_picker::COMPACT_TARGET_PERCENT_VALUE => {
                Self::Number(ConfigNumberKey::CompactTargetPercent)
            }
            config_picker::MAX_OUTPUT_BYTES_VALUE => Self::Number(ConfigNumberKey::MaxOutputBytes),
            config_picker::MAX_TOOL_OUTPUT_LINES_VALUE => {
                Self::Number(ConfigNumberKey::MaxToolOutputLines)
            }
            config_picker::PROMPT_HISTORY_LIMIT_VALUE => {
                Self::Number(ConfigNumberKey::PromptHistoryLimit)
            }
            config_picker::CLEAR_PROMPT_HISTORY_VALUE => Self::ClearPromptHistory,
            config_picker::INLINE_SHELL_VALUE => Self::InlineShell,
            config_picker::EDIT_TOOL_VALUE => Self::EditTool,
            config_picker::WEB_SEARCH_VALUE => Self::WebSearch,
            config_picker::WEB_SEARCH_HOSTED_VALUE => Self::WebSearchHosted,
            config_picker::WEB_SEARCH_PROVIDER_VALUE => Self::WebSearchProvider,
            config_picker::WEB_SEARCH_OPENAI_KEY_VALUE => {
                Self::WebSearchApiKey(ConfigTextKey::OpenAiSearch)
            }
            config_picker::WEB_SEARCH_EXA_KEY_VALUE => Self::WebSearchApiKey(ConfigTextKey::Exa),
            config_picker::WEB_SEARCH_BRAVE_KEY_VALUE => {
                Self::WebSearchApiKey(ConfigTextKey::Brave)
            }
            config_picker::XAI_IMAGE_GENERATION_VALUE => Self::XaiImageGeneration,
            _ => return None,
        })
    }
}
