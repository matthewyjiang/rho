use ratatui::text::Line;

use {
    crate::app::config_repository::ConfigRepository,
    rho_providers::credentials::{CredentialError, CredentialResult, WebSearchCredential},
};

use super::{
    config_picker,
    line_editor::LineEditor,
    render::{styled_line, truncate_one_line, LineFill},
    theme::Theme,
};

#[derive(Clone, Debug)]
pub(super) struct ConfigNumberInput {
    pub(super) key: ConfigNumberKey,
    pub(super) editor: LineEditor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ConfigNumberKey {
    MaxOutputBytes,
    MaxToolOutputLines,
    CompactThresholdPercent,
    CompactTargetPercent,
    PromptHistoryLimit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ConfigTextKey {
    OpenAiSearch,
    Exa,
    Brave,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ConfigToggle {
    CheckForUpdates,
    EnableSubagents,
    AutoCompact,
    CacheMissNotices,
    ShowReasoningOutput,
    ZenMode,
    WebSearchHosted,
    XaiImageGeneration,
}

pub(super) fn resolve_web_search_editor_value(
    stored: CredentialResult<Option<String>>,
    legacy: Option<&str>,
) -> (Option<String>, Option<CredentialError>) {
    match stored {
        Ok(Some(value)) => (Some(value), None),
        Ok(None) => (legacy.map(str::to_string), None),
        Err(err) => (legacy.map(str::to_string), Some(err)),
    }
}

pub(super) fn toggle(
    config_repository: &ConfigRepository,
    setting: ConfigToggle,
) -> anyhow::Result<bool> {
    config_repository.update(|config| match setting {
        ConfigToggle::CheckForUpdates => {
            config.check_for_updates = !config.check_for_updates;
            config.check_for_updates
        }
        ConfigToggle::EnableSubagents => {
            config.enable_subagents = !config.enable_subagents;
            config.enable_subagents
        }
        ConfigToggle::AutoCompact => {
            config.auto_compact = !config.auto_compact;
            config.auto_compact
        }
        ConfigToggle::CacheMissNotices => {
            config.cache_miss_notices = !config.cache_miss_notices;
            config.cache_miss_notices
        }
        ConfigToggle::ShowReasoningOutput => {
            config.show_reasoning_output = !config.show_reasoning_output;
            config.show_reasoning_output
        }
        ConfigToggle::ZenMode => {
            config.zen_mode = !config.zen_mode;
            config.zen_mode
        }
        ConfigToggle::WebSearchHosted => {
            config.web_search_hosted = !config.web_search_hosted;
            config.web_search_hosted
        }
        ConfigToggle::XaiImageGeneration => {
            config.xai_image_generation = !config.xai_image_generation;
            config.xai_image_generation
        }
    })
}

pub(super) fn cycle_web_search_provider(
    config_repository: &ConfigRepository,
) -> anyhow::Result<String> {
    config_repository.update(|config| {
        config.web_search_provider = config.web_search_provider.next_configurable();
        config.web_search_provider.to_string()
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ConfigNumberSave {
    MaxOutputBytes(usize),
    MaxToolOutputLines(usize),
    CompactThresholdPercent(u8),
    CompactTargetPercent(u8),
}

impl ConfigNumberInput {
    pub(super) fn save(
        &self,
        config_repository: &ConfigRepository,
    ) -> anyhow::Result<ConfigNumberSave> {
        let value = self.parsed_value()?;
        match self.key {
            ConfigNumberKey::PromptHistoryLimit => {
                anyhow::bail!("prompt history limit is applied through the confirm flow");
            }
            ConfigNumberKey::MaxOutputBytes => config_repository.update(|config| {
                config.max_output_bytes = value;
                ConfigNumberSave::MaxOutputBytes(value)
            }),
            ConfigNumberKey::MaxToolOutputLines => config_repository.update(|config| {
                config.max_tool_output_lines = value;
                ConfigNumberSave::MaxToolOutputLines(value)
            }),
            ConfigNumberKey::CompactThresholdPercent => config_repository.update(|config| {
                config.set_compact_threshold_percent(value.clamp(1, 100) as u8);
                ConfigNumberSave::CompactThresholdPercent(config.compact_threshold_percent)
            }),
            ConfigNumberKey::CompactTargetPercent => config_repository.update(|config| {
                config.set_compact_target_percent(value.clamp(1, 100) as u8);
                ConfigNumberSave::CompactTargetPercent(config.compact_target_percent)
            }),
        }
    }
}

impl ConfigNumberKey {
    pub(super) fn label(self) -> &'static str {
        match self {
            ConfigNumberKey::MaxOutputBytes => "max output bytes",
            ConfigNumberKey::MaxToolOutputLines => "max tool output lines",
            ConfigNumberKey::CompactThresholdPercent => "compact threshold percent",
            ConfigNumberKey::CompactTargetPercent => "compact target percent",
            ConfigNumberKey::PromptHistoryLimit => "prompt history limit",
        }
    }

    pub(super) fn picker_value(self) -> &'static str {
        match self {
            ConfigNumberKey::MaxOutputBytes => config_picker::MAX_OUTPUT_BYTES_VALUE,
            ConfigNumberKey::MaxToolOutputLines => config_picker::MAX_TOOL_OUTPUT_LINES_VALUE,
            ConfigNumberKey::CompactThresholdPercent => {
                config_picker::COMPACT_THRESHOLD_PERCENT_VALUE
            }
            ConfigNumberKey::CompactTargetPercent => config_picker::COMPACT_TARGET_PERCENT_VALUE,
            ConfigNumberKey::PromptHistoryLimit => config_picker::PROMPT_HISTORY_LIMIT_VALUE,
        }
    }

    pub(super) fn proposes_confirm(self) -> bool {
        matches!(self, Self::PromptHistoryLimit)
    }

    pub(super) fn min_value(self) -> usize {
        match self {
            ConfigNumberKey::PromptHistoryLimit => 0,
            _ => 1,
        }
    }

    pub(super) fn max_value(self) -> Option<usize> {
        match self {
            ConfigNumberKey::PromptHistoryLimit => Some(crate::config::MAX_PROMPT_HISTORY_LIMIT),
            _ => None,
        }
    }
}

impl ConfigTextKey {
    pub(super) fn label(self) -> &'static str {
        match self {
            ConfigTextKey::OpenAiSearch => "OpenAI web search API key",
            ConfigTextKey::Exa => "Exa API key",
            ConfigTextKey::Brave => "Brave Search API key",
        }
    }

    pub(super) fn picker_value(self) -> &'static str {
        match self {
            ConfigTextKey::OpenAiSearch => config_picker::WEB_SEARCH_OPENAI_KEY_VALUE,
            ConfigTextKey::Exa => config_picker::WEB_SEARCH_EXA_KEY_VALUE,
            ConfigTextKey::Brave => config_picker::WEB_SEARCH_BRAVE_KEY_VALUE,
        }
    }

    pub(super) fn web_search_credential(self) -> WebSearchCredential {
        match self {
            ConfigTextKey::OpenAiSearch => WebSearchCredential::OpenAi,
            ConfigTextKey::Exa => WebSearchCredential::Exa,
            ConfigTextKey::Brave => WebSearchCredential::Brave,
        }
    }
}

impl ConfigNumberInput {
    pub(super) fn parsed_value(&self) -> anyhow::Result<usize> {
        let value = self
            .editor
            .value
            .parse::<usize>()
            .map_err(|_| anyhow::anyhow!("{} must be a whole number", self.key.label()))?;
        let value = value.max(self.key.min_value());
        Ok(match self.key.max_value() {
            Some(max) => value.min(max),
            None => value,
        })
    }

    pub(super) fn new(key: ConfigNumberKey, value: usize) -> Self {
        Self {
            key,
            editor: LineEditor::new(value.to_string()),
        }
    }

    pub(super) fn insert_char(&mut self, ch: char) {
        if !ch.is_ascii_digit() {
            return;
        }
        self.editor.insert_char(ch);
    }

    pub(super) fn insert_text(&mut self, text: &str) {
        for ch in text.chars().filter(|ch| ch.is_ascii_digit()) {
            self.insert_char(ch);
        }
    }
}

pub(super) fn config_number_input_lines(
    input: &ConfigNumberInput,
    width: usize,
) -> Vec<Line<'static>> {
    let label = input.key.label();
    vec![
        styled_line(
            truncate_one_line(
                &format!(
                    "edit {label}  {}",
                    super::composer_chrome::join_footer_parts(["Enter save", "Esc cancel"])
                ),
                width,
            ),
            width,
            Theme::dim(),
            LineFill::Natural,
        ),
        styled_line(
            truncate_one_line(&input.editor.value, width),
            width,
            Theme::text(),
            LineFill::Natural,
        ),
    ]
}

#[cfg(test)]
#[path = "config_editor_tests.rs"]
mod tests;
