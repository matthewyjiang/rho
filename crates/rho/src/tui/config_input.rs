//! Login, secret, line-editor, and reasoning-cycle key handlers for the interactive TUI.

use crate::{keybindings::ReservedComposerKey, tui::DefaultTerminal};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{text_input::TextInputTarget, App, ComposerMode};

impl App {
    pub(super) fn handle_interactive_pending_key(&mut self, key: KeyEvent) -> anyhow::Result<bool> {
        if !matches!(
            self.input_ui.composer(),
            ComposerMode::InteractivePending(_)
        ) {
            return Ok(false);
        }

        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('c' | 'C')) => return Ok(false),
            (modifiers, KeyCode::Char('c' | 'C'))
                if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let url = match self.input_ui.composer() {
                    ComposerMode::InteractivePending(pending) => {
                        pending.prompt.copyable_url().to_string()
                    }
                    _ => return Ok(true),
                };
                self.copy_text(&url, std::time::Instant::now());
            }
            (_, KeyCode::Esc) => {
                self.abort_interactive_login();
                // Cancelling a pending login is not backing out of setup.
                // Setup picker Esc still calls `dismiss_setup_screen()`.
                self.restore_after_cancelled_login();
                self.clear_transient_key_state();
            }
            _ => {}
        }
        Ok(true)
    }

    pub(super) async fn handle_secret_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<bool> {
        match self.apply_secret_key(key) {
            SecretKeyResult::NotSecret => Ok(false),
            SecretKeyResult::Handled => {
                self.clear_transient_key_state();
                Ok(true)
            }
            SecretKeyResult::Submit(submission) => {
                self.clear_transient_key_state();
                self.submit_api_key_login(submission, terminal).await?;
                Ok(true)
            }
        }
    }

    /// Apply a keystroke to API-key secret input.
    fn apply_secret_key(&mut self, key: KeyEvent) -> SecretKeyResult {
        if !matches!(self.input_ui.composer(), ComposerMode::SecretInput(_)) {
            return SecretKeyResult::NotSecret;
        }

        // Restore before borrowing the secret buffer: Esc needs `&mut self`.
        if matches!(key.code, KeyCode::Esc) {
            self.restore_after_cancelled_login();
            return SecretKeyResult::Handled;
        }

        let ComposerMode::SecretInput(secret) = self.input_ui.composer_mut() else {
            return SecretKeyResult::NotSecret;
        };

        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Enter) => {
                let submission = secret.submission();
                self.input_ui.set_composer(ComposerMode::Input);
                SecretKeyResult::Submit(submission)
            }
            (_, KeyCode::Backspace) => {
                secret.editor.backspace();
                SecretKeyResult::Handled
            }
            (_, KeyCode::Delete) => {
                secret.editor.delete();
                SecretKeyResult::Handled
            }
            (_, KeyCode::Left) => {
                secret.editor.move_cursor_left();
                SecretKeyResult::Handled
            }
            (_, KeyCode::Right) => {
                secret.editor.move_cursor_right();
                SecretKeyResult::Handled
            }
            (_, KeyCode::Home) => {
                secret.editor.move_cursor_home();
                SecretKeyResult::Handled
            }
            (_, KeyCode::End) => {
                secret.editor.move_cursor_end();
                SecretKeyResult::Handled
            }
            (modifiers, KeyCode::Char(ch))
                if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                secret.editor.insert_char(ch);
                SecretKeyResult::Handled
            }
            _ => SecretKeyResult::Handled,
        }
    }

    /// Keys for the line editor of a setting, agent field, or login step.
    /// Runs the same whether or not a turn is under way.
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
        let target = input.target.clone();
        let value = input.editor.value.clone();
        match target {
            TextInputTarget::Setting { namespace, key, .. } => {
                self.commit_setting_input(namespace, key, &value);
            }
            TextInputTarget::AgentField(field) => self.commit_agent_text_input(field, value)?,
            TextInputTarget::CustomHost(step) => self.submit_custom_host_step(step, value)?,
        }
        Ok(true)
    }

    fn cancel_text_input(&mut self) -> anyhow::Result<bool> {
        let ComposerMode::TextInput(input) = self.input_ui.composer() else {
            return Ok(true);
        };
        match input.target.clone() {
            TextInputTarget::Setting { .. } => self.close_setting_input(),
            TextInputTarget::AgentField(field) => self.reopen_agent_field_picker(field.value()),
            TextInputTarget::CustomHost(_) => self.cancel_custom_host_step(),
        }
        Ok(true)
    }

    pub(super) fn handle_reasoning_cycle_key(&mut self, key: KeyEvent) -> anyhow::Result<bool> {
        if ReservedComposerKey::from_key(key) != Some(ReservedComposerKey::ReasoningCycle) {
            return Ok(false);
        }

        self.cycle_daemon_reasoning()?;
        self.clear_transient_key_state();
        Ok(true)
    }

    fn with_text_input_mut(&mut self, f: impl FnOnce(&mut super::text_input::TextInput)) {
        if let ComposerMode::TextInput(input) = self.input_ui.composer_mut() {
            f(input);
        }
    }
}

enum SecretKeyResult {
    NotSecret,
    Handled,
    Submit(super::login::ApiKeySubmission),
}

#[cfg(test)]
#[path = "config_input_tests.rs"]
mod tests;
