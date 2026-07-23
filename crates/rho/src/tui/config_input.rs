//! Config, OAuth, secret, and reasoning-cycle key handlers for the interactive TUI.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::DefaultTerminal;

use super::{
    config_editor::ConfigNumberSave, config_picker, App, ComposerMode, Entry, InteractiveRuntime,
};

impl App {
    pub(super) fn handle_oauth_pending_key(&mut self, key: KeyEvent) -> anyhow::Result<bool> {
        if !matches!(self.composer, ComposerMode::OAuthPending(_)) {
            return Ok(false);
        }

        match key.code {
            KeyCode::Esc => {
                let provider = if let Some(pending) = self.pending_oauth_login.take() {
                    let provider = pending.target.provider;
                    pending.handle.abort();
                    provider
                } else {
                    "OAuth".into()
                };
                self.composer = ComposerMode::Input;
                self.status = "login cancelled".into();
                self.insert_entry(&Entry::Notice(format!("{provider} login cancelled")));
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            _ => Ok(true),
        }
    }

    pub(super) async fn handle_secret_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        let ComposerMode::SecretInput(secret) = &mut self.composer else {
            return Ok(false);
        };

        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Enter) => {
                let target = secret.target.clone();
                let value = secret.value.trim().to_string();
                self.composer = ComposerMode::Input;
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                self.submit_api_key_login(target, value, terminal, agent)
                    .await?;
                Ok(true)
            }
            (_, KeyCode::Esc) => {
                self.composer = ComposerMode::Input;
                self.status = "login cancelled".into();
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (_, KeyCode::Backspace) => {
                secret.backspace();
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (_, KeyCode::Delete) => {
                secret.delete();
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (_, KeyCode::Left) => {
                secret.cursor = secret.cursor.saturating_sub(1);
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (_, KeyCode::Right) => {
                secret.cursor = (secret.cursor + 1).min(secret.char_len());
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (_, KeyCode::Home) => {
                secret.cursor = 0;
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (_, KeyCode::End) => {
                secret.cursor = secret.char_len();
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (modifiers, KeyCode::Char(ch))
                if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                secret.insert_char(ch);
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            _ => Ok(true),
        }
    }

    pub(super) fn handle_config_number_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<bool> {
        if !matches!(self.composer, ComposerMode::ConfigNumberInput(_)) {
            return Ok(false);
        }

        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Enter) => {
                let ComposerMode::ConfigNumberInput(input) = &self.composer else {
                    return Ok(true);
                };
                let saved = match input.save(&self.info.services.config_repository) {
                    Ok(saved) => saved,
                    Err(err) => {
                        self.insert_entry(&Entry::Error(err.to_string()));
                        self.status = "config save failed".into();
                        return Ok(true);
                    }
                };
                match saved {
                    ConfigNumberSave::MaxOutputBytes(value) => {
                        self.open_main_config_picker_selected(
                            config_picker::MAX_OUTPUT_BYTES_VALUE,
                        )?;
                        self.insert_entry(&Entry::Notice(format!(
                            "max output bytes set to {value}; applies next session"
                        )));
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
                        self.insert_entry(&Entry::Notice(format!(
                            "max tool output lines set to {value}"
                        )));
                    }
                    ConfigNumberSave::CompactThresholdPercent(value) => {
                        self.open_main_config_picker_selected(
                            config_picker::COMPACT_THRESHOLD_PERCENT_VALUE,
                        )?;
                        self.insert_entry(&Entry::Notice(format!(
                            "compact threshold set to {value}%"
                        )));
                    }
                    ConfigNumberSave::CompactTargetPercent(value) => {
                        self.open_main_config_picker_selected(
                            config_picker::COMPACT_TARGET_PERCENT_VALUE,
                        )?;
                        self.insert_entry(&Entry::Notice(format!(
                            "compact target set to {value}%"
                        )));
                    }
                }
                self.status = "config saved".into();
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Backspace) => {
                if let ComposerMode::ConfigNumberInput(input) = &mut self.composer {
                    input.backspace();
                }
                Ok(true)
            }
            (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(ch)) => {
                if let ComposerMode::ConfigNumberInput(input) = &mut self.composer {
                    input.insert_char(ch);
                }
                Ok(true)
            }
            (_, KeyCode::Left) => {
                if let ComposerMode::ConfigNumberInput(input) = &mut self.composer {
                    input.move_cursor_left();
                }
                Ok(true)
            }
            (_, KeyCode::Right) => {
                if let ComposerMode::ConfigNumberInput(input) = &mut self.composer {
                    input.move_cursor_right();
                }
                Ok(true)
            }
            (_, KeyCode::Home) => {
                if let ComposerMode::ConfigNumberInput(input) = &mut self.composer {
                    input.move_cursor_home();
                }
                Ok(true)
            }
            (_, KeyCode::End) => {
                if let ComposerMode::ConfigNumberInput(input) = &mut self.composer {
                    input.move_cursor_end();
                }
                Ok(true)
            }
            (_, KeyCode::Esc) => {
                let ComposerMode::ConfigNumberInput(input) = &self.composer else {
                    return Ok(true);
                };
                let selected_value = input.key.picker_value();
                let config = self.info.services.config_repository.load()?;
                self.info.runtime.show_reasoning_output = config.show_reasoning_output;
                self.open_main_config_picker_selected(selected_value)?;
                Ok(true)
            }
            _ => Ok(true),
        }
    }

    pub(super) fn handle_config_text_key(&mut self, key: KeyEvent) -> anyhow::Result<bool> {
        if !matches!(self.composer, ComposerMode::ConfigTextInput(_)) {
            return Ok(false);
        }

        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Enter) => {
                let ComposerMode::ConfigTextInput(input) = &self.composer else {
                    return Ok(true);
                };
                let key = input.key;
                let save_result = input.save(self.credential_store.as_ref());
                match save_result {
                    Ok(()) => {
                        self.refresh_web_search_config_picker(key.picker_value())?;
                        self.status = format!("{} saved", key.label());
                    }
                    Err(err) => {
                        self.insert_entry(&Entry::Error(format!(
                            "could not save {}: {err}",
                            key.label()
                        )));
                        self.status = "config save failed".into();
                    }
                }
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Backspace) => {
                if let ComposerMode::ConfigTextInput(input) = &mut self.composer {
                    input.backspace();
                }
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Delete) => {
                if let ComposerMode::ConfigTextInput(input) = &mut self.composer {
                    input.delete();
                }
                Ok(true)
            }
            (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(ch)) => {
                if let ComposerMode::ConfigTextInput(input) = &mut self.composer {
                    input.insert_char(ch);
                }
                Ok(true)
            }
            (_, KeyCode::Left) => {
                if let ComposerMode::ConfigTextInput(input) = &mut self.composer {
                    input.move_cursor_left();
                }
                Ok(true)
            }
            (_, KeyCode::Right) => {
                if let ComposerMode::ConfigTextInput(input) = &mut self.composer {
                    input.move_cursor_right();
                }
                Ok(true)
            }
            (_, KeyCode::Home) => {
                if let ComposerMode::ConfigTextInput(input) = &mut self.composer {
                    input.move_cursor_home();
                }
                Ok(true)
            }
            (_, KeyCode::End) => {
                if let ComposerMode::ConfigTextInput(input) = &mut self.composer {
                    input.move_cursor_end();
                }
                Ok(true)
            }
            (_, KeyCode::Esc) => {
                let ComposerMode::ConfigTextInput(input) = &self.composer else {
                    return Ok(true);
                };
                self.refresh_web_search_config_picker(input.key.picker_value())?;
                self.status = "web search config".into();
                Ok(true)
            }
            _ => Ok(true),
        }
    }

    pub(super) fn handle_reasoning_cycle_key(
        &mut self,
        key: KeyEvent,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        let is_shift_tab = matches!(key.code, KeyCode::BackTab)
            || (matches!(key.code, KeyCode::Tab) && key.modifiers.contains(KeyModifiers::SHIFT));
        if !is_shift_tab {
            return Ok(false);
        }

        self.cycle_reasoning(agent)?;
        self.paste_burst.clear();
        self.ctrl_c_streak = 0;
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
        self.composer =
            ComposerMode::Picker(config_picker::config_picker(&self.info.runtime, &config));
        self.status = "config".into();
        terminal.draw(|frame| self.draw(frame))?;
        Ok(())
    }
}
