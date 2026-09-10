use ratatui::text::Line;

use {
    crate::app::config_repository::ConfigRepository,
    rho_providers::credentials::{CredentialError, CredentialResult},
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
    QuestionnaireTimeout,
    MaxOutputBytes,
    MaxToolOutputLines,
    CompactThresholdPercent,
    CompactTargetPercent,
    PromptHistoryLimit,
    AgentConcurrency,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ConfigToggle {
    CheckForUpdates,
    EnableSubagents,
    AutoCompact,
    CacheMissNotices,
    ShowReasoningOutput,
    ZenMode,
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
        ConfigToggle::XaiImageGeneration => {
            config.xai_image_generation = !config.xai_image_generation;
            config.xai_image_generation
        }
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ConfigNumberSave {
    QuestionnaireTimeout(Option<std::num::NonZeroU64>),
    MaxOutputBytes(usize),
    MaxToolOutputLines(usize),
    CompactThresholdPercent(u8),
    CompactTargetPercent(u8),
    AgentConcurrency(usize),
}

impl ConfigNumberInput {
    pub(super) fn save(
        &self,
        config_repository: &ConfigRepository,
    ) -> anyhow::Result<ConfigNumberSave> {
        let mut saved = match self.key {
            ConfigNumberKey::QuestionnaireTimeout => {
                let value = self.editor.value.trim();
                let timeout = if value.is_empty() {
                    None
                } else {
                    Some(value.parse().map_err(|_| {
                        anyhow::anyhow!("questionnaire timeout must be positive whole seconds; clear the field for Disabled")
                    })?)
                };
                ConfigNumberSave::QuestionnaireTimeout(timeout)
            }
            ConfigNumberKey::PromptHistoryLimit => {
                anyhow::bail!("prompt history limit is applied through the confirm flow");
            }
            ConfigNumberKey::MaxOutputBytes => {
                ConfigNumberSave::MaxOutputBytes(self.parsed_value()?)
            }
            ConfigNumberKey::MaxToolOutputLines => {
                ConfigNumberSave::MaxToolOutputLines(self.parsed_value()?)
            }
            ConfigNumberKey::CompactThresholdPercent => {
                ConfigNumberSave::CompactThresholdPercent(self.parsed_value()?.clamp(1, 100) as u8)
            }
            ConfigNumberKey::CompactTargetPercent => {
                ConfigNumberSave::CompactTargetPercent(self.parsed_value()?.clamp(1, 100) as u8)
            }
            ConfigNumberKey::AgentConcurrency => {
                ConfigNumberSave::AgentConcurrency(self.parsed_value()?)
            }
        };
        config_repository.update(|config| {
            match &mut saved {
                ConfigNumberSave::QuestionnaireTimeout(value) => {
                    config.questionnaire.timeout_seconds = *value
                }
                ConfigNumberSave::MaxOutputBytes(value) => config.max_output_bytes = *value,
                ConfigNumberSave::MaxToolOutputLines(value) => {
                    config.max_tool_output_lines = *value
                }
                ConfigNumberSave::CompactThresholdPercent(value) => {
                    config.set_compact_threshold_percent(*value);
                    *value = config.compact_threshold_percent;
                }
                ConfigNumberSave::CompactTargetPercent(value) => {
                    config.set_compact_target_percent(*value);
                    *value = config.compact_target_percent;
                }
                ConfigNumberSave::AgentConcurrency(value) => config.set_agent_concurrency(*value),
            }
            saved
        })
    }
}

impl ConfigNumberKey {
    pub(super) fn label(self) -> &'static str {
        match self {
            ConfigNumberKey::QuestionnaireTimeout => "questionnaire timeout seconds",
            ConfigNumberKey::MaxOutputBytes => "max output bytes",
            ConfigNumberKey::MaxToolOutputLines => "max tool output lines",
            ConfigNumberKey::CompactThresholdPercent => "compact threshold percent",
            ConfigNumberKey::CompactTargetPercent => "compact target percent",
            ConfigNumberKey::PromptHistoryLimit => "prompt history limit",
            ConfigNumberKey::AgentConcurrency => "concurrent agents",
        }
    }

    pub(super) fn picker_value(self) -> &'static str {
        match self {
            ConfigNumberKey::QuestionnaireTimeout => config_picker::QUESTIONNAIRE_TIMEOUT_VALUE,
            ConfigNumberKey::MaxOutputBytes => config_picker::MAX_OUTPUT_BYTES_VALUE,
            ConfigNumberKey::MaxToolOutputLines => config_picker::MAX_TOOL_OUTPUT_LINES_VALUE,
            ConfigNumberKey::CompactThresholdPercent => {
                config_picker::COMPACT_THRESHOLD_PERCENT_VALUE
            }
            ConfigNumberKey::CompactTargetPercent => config_picker::COMPACT_TARGET_PERCENT_VALUE,
            ConfigNumberKey::PromptHistoryLimit => config_picker::PROMPT_HISTORY_LIMIT_VALUE,
            ConfigNumberKey::AgentConcurrency => config_picker::AGENT_CONCURRENCY_VALUE,
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
            ConfigNumberKey::AgentConcurrency => Some(crate::config::MAX_AGENT_CONCURRENCY),
            _ => None,
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

    pub(super) fn questionnaire_timeout(value: Option<std::num::NonZeroU64>) -> Self {
        Self {
            key: ConfigNumberKey::QuestionnaireTimeout,
            editor: LineEditor::new(value.map(|value| value.to_string()).unwrap_or_default()),
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
    let mut lines = vec![
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
    ];
    if input.key == ConfigNumberKey::QuestionnaireTimeout {
        lines.extend(super::render::wrap_line_at_whitespace(
            "Positive seconds; empty = Disabled. Only forms with explicit fallback answers time out. Applies when the next form opens.",
            width,
        ).into_iter().map(|line| Line::styled(line.to_owned(), Theme::dim())));
    }
    lines
}

#[cfg(test)]
#[path = "config_editor_tests.rs"]
mod tests;
