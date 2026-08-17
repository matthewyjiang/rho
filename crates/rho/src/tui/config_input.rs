//! Config, OAuth, secret, and reasoning-cycle key handlers for the interactive TUI.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::DefaultTerminal;

use super::{
    config_editor::{ConfigNumberInput, ConfigNumberSave},
    config_picker, App, ComposerMode, Entry, InteractiveRuntime,
};

impl App {
    pub(super) fn handle_interactive_pending_key(&mut self, key: KeyEvent) -> anyhow::Result<bool> {
        if !matches!(
            self.input_ui.composer(),
            ComposerMode::InteractivePending(_)
        ) {
            return Ok(false);
        }

        if key.code == KeyCode::Esc {
            if let Some(pending) = self.pending_interactive_login.take() {
                pending.handle.abort();
            }
            self.input_ui.set_composer(ComposerMode::Input);
            self.set_status("login cancelled");
            self.clear_transient_key_state();
        }
        Ok(true)
    }

    pub(super) async fn handle_secret_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        let ComposerMode::SecretInput(secret) = self.input_ui.composer_mut() else {
            return Ok(false);
        };

        let submit = match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Enter) => {
                let target = secret.target.clone();
                let value = secret.value.trim().to_string();
                self.input_ui.set_composer(ComposerMode::Input);
                Some((target, value))
            }
            (_, KeyCode::Esc) => {
                self.input_ui.set_composer(ComposerMode::Input);
                self.set_status("login cancelled");
                None
            }
            (_, KeyCode::Backspace) => {
                secret.backspace();
                None
            }
            (_, KeyCode::Delete) => {
                secret.delete();
                None
            }
            (_, KeyCode::Left) => {
                secret.cursor = secret.cursor.saturating_sub(1);
                None
            }
            (_, KeyCode::Right) => {
                secret.cursor = (secret.cursor + 1).min(secret.char_len());
                None
            }
            (_, KeyCode::Home) => {
                secret.cursor = 0;
                None
            }
            (_, KeyCode::End) => {
                secret.cursor = secret.char_len();
                None
            }
            (modifiers, KeyCode::Char(ch))
                if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                secret.insert_char(ch);
                None
            }
            _ => None,
        };
        self.clear_transient_key_state();
        if let Some((target, value)) = submit {
            self.submit_api_key_login(target, value, terminal, agent)
                .await?;
        }
        Ok(true)
    }

    pub(super) fn handle_config_number_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<bool> {
        if !matches!(self.input_ui.composer(), ComposerMode::ConfigNumberInput(_)) {
            return Ok(false);
        }

        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Enter) => {
                let ComposerMode::ConfigNumberInput(input) = self.input_ui.composer() else {
                    return Ok(true);
                };
                let saved = match input.save(&self.info.services.config_repository) {
                    Ok(saved) => saved,
                    Err(err) => {
                        self.insert_entry(&Entry::Error(err.to_string()));
                        self.set_status("config save failed");
                        return Ok(true);
                    }
                };
                match saved {
                    ConfigNumberSave::MaxOutputBytes(value) => {
                        self.open_main_config_picker_selected(
                            config_picker::MAX_OUTPUT_BYTES_VALUE,
                        )?;
                        self.set_status(format!(
                            "max output bytes set to {value}; applies next session"
                        ));
                    }
                    ConfigNumberSave::MaxToolOutputLines(value) => {
                        self.info.runtime.max_tool_output_lines = value;
                        self.info
                            .services
                            .diagnostics
                            .update_max_tool_output_lines(value);
                        self.open_main_config_picker_selected(
                            config_picker::MAX_TOOL_OUTPUT_LINES_VALUE,
                        )?;
                        self.clamp_history_scroll_for_terminal(terminal)?;
                        self.set_status(format!("max tool output lines set to {value}"));
                    }
                    ConfigNumberSave::CompactThresholdPercent(value) => {
                        self.open_main_config_picker_selected(
                            config_picker::COMPACT_THRESHOLD_PERCENT_VALUE,
                        )?;
                        self.set_status(format!("compact threshold set to {value}%"));
                    }
                    ConfigNumberSave::CompactTargetPercent(value) => {
                        self.open_main_config_picker_selected(
                            config_picker::COMPACT_TARGET_PERCENT_VALUE,
                        )?;
                        self.set_status(format!("compact target set to {value}%"));
                    }
                }
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Backspace) => {
                self.with_config_number_mut(ConfigNumberInput::backspace);
                Ok(true)
            }
            (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(ch)) => {
                self.with_config_number_mut(|input| input.insert_char(ch));
                Ok(true)
            }
            (_, KeyCode::Left) => {
                self.with_config_number_mut(ConfigNumberInput::move_cursor_left);
                Ok(true)
            }
            (_, KeyCode::Right) => {
                self.with_config_number_mut(ConfigNumberInput::move_cursor_right);
                Ok(true)
            }
            (_, KeyCode::Home) => {
                self.with_config_number_mut(ConfigNumberInput::move_cursor_home);
                Ok(true)
            }
            (_, KeyCode::End) => {
                self.with_config_number_mut(ConfigNumberInput::move_cursor_end);
                Ok(true)
            }
            (_, KeyCode::Esc) => {
                let ComposerMode::ConfigNumberInput(input) = self.input_ui.composer() else {
                    return Ok(true);
                };
                let selected_value = input.key.picker_value();
                let config = self.info.services.config_repository.load()?;
                self.info.runtime.show_reasoning_output = config.show_reasoning_output;
                self.info.runtime.zen_mode = config.zen_mode;
                self.open_main_config_picker_selected(selected_value)?;
                Ok(true)
            }
            _ => Ok(true),
        }
    }

    pub(super) fn handle_text_input_key(&mut self, key: KeyEvent) -> anyhow::Result<bool> {
        if !matches!(self.input_ui.composer(), ComposerMode::TextInput(_)) {
            return Ok(false);
        }

        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Enter) => self.commit_text_input(),
            (KeyModifiers::NONE, KeyCode::Backspace) => {
                self.with_text_input_mut(|input| input.editor.backspace());
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Delete) => {
                self.with_text_input_mut(|input| input.editor.delete());
                Ok(true)
            }
            (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(ch)) => {
                self.with_text_input_mut(|input| input.editor.insert_char(ch));
                Ok(true)
            }
            (_, KeyCode::Left) => {
                self.with_text_input_mut(|input| input.editor.move_cursor_left());
                Ok(true)
            }
            (_, KeyCode::Right) => {
                self.with_text_input_mut(|input| input.editor.move_cursor_right());
                Ok(true)
            }
            (_, KeyCode::Home) => {
                self.with_text_input_mut(|input| input.editor.move_cursor_home());
                Ok(true)
            }
            (_, KeyCode::End) => {
                self.with_text_input_mut(|input| input.editor.move_cursor_end());
                Ok(true)
            }
            (_, KeyCode::Esc) => self.cancel_text_input(),
            _ => Ok(true),
        }
    }

    fn commit_text_input(&mut self) -> anyhow::Result<bool> {
        let ComposerMode::TextInput(input) = self.input_ui.composer() else {
            return Ok(true);
        };
        match input.target {
            super::text_input::TextInputTarget::ConfigApiKey(key) => {
                let value = input.editor.value.clone();
                let save_result = save_config_api_key(self.credential_store.as_ref(), key, &value);
                match save_result {
                    Ok(()) => {
                        self.refresh_web_search_config_picker(key.picker_value())?;
                        self.set_status(format!("{} saved", key.label()));
                    }
                    Err(err) => {
                        self.insert_entry(&Entry::Error(format!(
                            "could not save {}: {err}",
                            key.label()
                        )));
                        self.set_status("config save failed");
                    }
                }
            }
            super::text_input::TextInputTarget::AgentField(field) => {
                let value = input.editor.value.clone();
                self.commit_agent_text_input(field, value)?;
            }
        }
        Ok(true)
    }

    fn cancel_text_input(&mut self) -> anyhow::Result<bool> {
        let ComposerMode::TextInput(input) = self.input_ui.composer() else {
            return Ok(true);
        };
        match input.target {
            super::text_input::TextInputTarget::ConfigApiKey(key) => {
                self.refresh_web_search_config_picker(key.picker_value())?;
                self.set_status("web search config");
            }
            super::text_input::TextInputTarget::AgentField(field) => {
                self.reopen_agent_field_picker(field.value());
            }
        }
        Ok(true)
    }

    pub(super) async fn handle_reasoning_cycle_key(
        &mut self,
        key: KeyEvent,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        let is_shift_tab = matches!(key.code, KeyCode::BackTab)
            || (matches!(key.code, KeyCode::Tab) && key.modifiers.contains(KeyModifiers::SHIFT));
        if !is_shift_tab {
            return Ok(false);
        }

        self.cycle_reasoning(agent).await?;
        self.clear_transient_key_state();
        Ok(true)
    }

    pub(super) fn execute_config_command(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let config = self.info.services.config_repository.load()?;
        self.info.runtime.max_tool_output_lines = config.max_tool_output_lines.max(1);
        self.info
            .services
            .diagnostics
            .update_max_tool_output_lines(self.info.runtime.max_tool_output_lines);
        self.info.runtime.show_reasoning_output = config.show_reasoning_output;
        self.info.runtime.zen_mode = config.zen_mode;
        let picker = self.config_root_picker(&config);
        self.input_ui.set_composer(ComposerMode::Picker(picker));
        self.set_status("config");
        terminal.draw(|frame| self.draw(frame))?;
        Ok(())
    }

    fn with_config_number_mut(&mut self, f: impl FnOnce(&mut ConfigNumberInput)) {
        if let ComposerMode::ConfigNumberInput(input) = self.input_ui.composer_mut() {
            f(input);
        }
    }

    fn with_text_input_mut(&mut self, f: impl FnOnce(&mut super::text_input::TextInput)) {
        if let ComposerMode::TextInput(input) = self.input_ui.composer_mut() {
            f(input);
        }
    }
}

fn save_config_api_key(
    credential_store: &dyn rho_providers::credentials::CredentialStore,
    key: super::config_editor::ConfigTextKey,
    value: &str,
) -> rho_providers::credentials::CredentialResult<()> {
    use rho_providers::credentials::{delete_web_search_api_key, save_web_search_api_key};
    let value = value.trim();
    let credential = key.web_search_credential();
    if value.is_empty() {
        delete_web_search_api_key(credential_store, credential).map(|_| ())
    } else {
        save_web_search_api_key(credential_store, credential, value)
    }
}
