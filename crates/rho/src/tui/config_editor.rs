use ratatui::text::Line;

use crate::{
    app::config_repository::ConfigRepository,
    credentials::{
        delete_web_search_api_key, save_web_search_api_key, CredentialError, CredentialResult,
        CredentialStore, WebSearchCredential,
    },
};

use super::{
    config_picker,
    render::{styled_line, truncate_one_line, LineFill},
    theme::Theme,
};

#[derive(Clone, Debug)]
pub(super) struct ConfigNumberInput {
    pub(super) key: ConfigNumberKey,
    pub(super) value: String,
    pub(super) cursor: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ConfigNumberKey {
    MaxOutputBytes,
    MaxToolOutputLines,
    CompactThresholdPercent,
    CompactTargetPercent,
}

#[derive(Clone, Debug)]
pub(super) struct ConfigTextInput {
    pub(super) key: ConfigTextKey,
    pub(super) value: String,
    pub(super) cursor: usize,
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
    ShowReasoningOutput,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ConfigMutation {
    CheckForUpdates(bool),
    EnableSubagents(bool),
    AutoCompact(bool),
    ShowReasoningOutput(bool),
    WebSearchProvider(String),
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
) -> anyhow::Result<ConfigMutation> {
    config_repository.update(|config| match setting {
        ConfigToggle::CheckForUpdates => {
            config.check_for_updates = !config.check_for_updates;
            ConfigMutation::CheckForUpdates(config.check_for_updates)
        }
        ConfigToggle::EnableSubagents => {
            config.enable_subagents = !config.enable_subagents;
            ConfigMutation::EnableSubagents(config.enable_subagents)
        }
        ConfigToggle::AutoCompact => {
            config.auto_compact = !config.auto_compact;
            ConfigMutation::AutoCompact(config.auto_compact)
        }
        ConfigToggle::ShowReasoningOutput => {
            config.show_reasoning_output = !config.show_reasoning_output;
            ConfigMutation::ShowReasoningOutput(config.show_reasoning_output)
        }
    })
}

pub(super) fn cycle_web_search_provider(
    config_repository: &ConfigRepository,
) -> anyhow::Result<ConfigMutation> {
    config_repository.update(|config| {
        config.web_search_provider = config.web_search_provider.next_configurable();
        ConfigMutation::WebSearchProvider(config.web_search_provider.to_string())
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
        let Ok(mut value) = self.value.parse::<usize>() else {
            anyhow::bail!("{} must be a positive whole number", self.key.label());
        };
        value = value.max(1);
        config_repository.update(|config| match self.key {
            ConfigNumberKey::MaxOutputBytes => {
                config.max_output_bytes = value;
                ConfigNumberSave::MaxOutputBytes(value)
            }
            ConfigNumberKey::MaxToolOutputLines => {
                config.max_tool_output_lines = value;
                ConfigNumberSave::MaxToolOutputLines(value)
            }
            ConfigNumberKey::CompactThresholdPercent => {
                config.set_compact_threshold_percent(value.clamp(1, 100) as u8);
                ConfigNumberSave::CompactThresholdPercent(config.compact_threshold_percent)
            }
            ConfigNumberKey::CompactTargetPercent => {
                config.set_compact_target_percent(value.clamp(1, 100) as u8);
                ConfigNumberSave::CompactTargetPercent(config.compact_target_percent)
            }
        })
    }
}

impl ConfigNumberKey {
    pub(super) fn label(self) -> &'static str {
        match self {
            ConfigNumberKey::MaxOutputBytes => "max output bytes",
            ConfigNumberKey::MaxToolOutputLines => "max tool output lines",
            ConfigNumberKey::CompactThresholdPercent => "compact threshold percent",
            ConfigNumberKey::CompactTargetPercent => "compact target percent",
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
    pub(super) fn new(key: ConfigNumberKey, value: usize) -> Self {
        let value = value.to_string();
        let cursor = value.chars().count();
        Self { key, value, cursor }
    }

    fn byte_index(&self, char_index: usize) -> usize {
        self.value
            .char_indices()
            .nth(char_index)
            .map(|(index, _)| index)
            .unwrap_or(self.value.len())
    }

    pub(super) fn insert_char(&mut self, ch: char) {
        if !ch.is_ascii_digit() {
            return;
        }
        let byte_index = self.byte_index(self.cursor);
        self.value.insert(byte_index, ch);
        self.cursor += 1;
    }

    pub(super) fn insert_text(&mut self, text: &str) {
        for ch in text.chars().filter(|ch| ch.is_ascii_digit()) {
            self.insert_char(ch);
        }
    }

    pub(super) fn move_cursor_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub(super) fn move_cursor_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.value.chars().count());
    }

    pub(super) fn move_cursor_home(&mut self) {
        self.cursor = 0;
    }

    pub(super) fn move_cursor_end(&mut self) {
        self.cursor = self.value.chars().count();
    }

    pub(super) fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self.byte_index(self.cursor - 1);
        let end = self.byte_index(self.cursor);
        self.value.replace_range(start..end, "");
        self.cursor -= 1;
    }
}

impl ConfigTextInput {
    pub(super) fn new(key: ConfigTextKey, value: Option<String>) -> Self {
        let value = value.unwrap_or_default();
        let cursor = value.chars().count();
        Self { key, value, cursor }
    }

    pub(super) fn save(&self, credential_store: &dyn CredentialStore) -> CredentialResult<()> {
        let value = self.value.trim();
        let credential = self.key.web_search_credential();
        if value.is_empty() {
            delete_web_search_api_key(credential_store, credential).map(|_| ())
        } else {
            save_web_search_api_key(credential_store, credential, value)
        }
    }

    fn char_len(&self) -> usize {
        self.value.chars().count()
    }

    fn byte_index(&self, char_index: usize) -> usize {
        self.value
            .char_indices()
            .nth(char_index)
            .map(|(index, _)| index)
            .unwrap_or(self.value.len())
    }

    pub(super) fn insert_char(&mut self, ch: char) {
        if ch == '\n' || ch == '\r' {
            return;
        }
        let byte_index = self.byte_index(self.cursor);
        self.value.insert(byte_index, ch);
        self.cursor += 1;
    }

    pub(super) fn insert_text(&mut self, text: &str) {
        let sanitized = text.replace(['\n', '\r'], "");
        let byte_index = self.byte_index(self.cursor);
        self.value.insert_str(byte_index, &sanitized);
        self.cursor += sanitized.chars().count();
    }

    pub(super) fn move_cursor_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub(super) fn move_cursor_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.char_len());
    }

    pub(super) fn move_cursor_home(&mut self) {
        self.cursor = 0;
    }

    pub(super) fn move_cursor_end(&mut self) {
        self.cursor = self.char_len();
    }

    pub(super) fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self.byte_index(self.cursor - 1);
        let end = self.byte_index(self.cursor);
        self.value.replace_range(start..end, "");
        self.cursor -= 1;
    }

    pub(super) fn delete(&mut self) {
        if self.cursor >= self.char_len() {
            return;
        }
        let start = self.byte_index(self.cursor);
        let end = self.byte_index(self.cursor + 1);
        self.value.replace_range(start..end, "");
    }
}

pub(super) fn config_number_input_lines(
    input: &ConfigNumberInput,
    width: usize,
) -> Vec<Line<'static>> {
    let label = input.key.label();
    vec![
        styled_line(
            truncate_one_line(&format!("edit {label}  enter save, esc cancel"), width),
            width,
            Theme::dim(),
            LineFill::Natural,
        ),
        styled_line(
            truncate_one_line(&input.value, width),
            width,
            Theme::text(),
            LineFill::Natural,
        ),
    ]
}

pub(super) fn config_text_input_lines(input: &ConfigTextInput, width: usize) -> Vec<Line<'static>> {
    let masked = "•".repeat(input.value.chars().count());
    vec![
        styled_line(
            truncate_one_line(
                &format!("edit {}  enter save, esc cancel", input.key.label()),
                width,
            ),
            width,
            Theme::dim(),
            LineFill::Natural,
        ),
        styled_line(
            truncate_one_line(&masked, width),
            width,
            Theme::text(),
            LineFill::Natural,
        ),
    ]
}

#[cfg(test)]
#[path = "config_editor_tests.rs"]
mod tests;
