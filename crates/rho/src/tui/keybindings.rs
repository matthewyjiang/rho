use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::Backend, DefaultTerminal, Terminal};

use super::{App, Entry, InteractiveRuntime};

impl App {
    pub(super) fn handle_configurable_running_key<B: Backend>(
        &mut self,
        key: KeyEvent,
        terminal: &mut Terminal<B>,
    ) -> anyhow::Result<bool> {
        if self.info.runtime.keybindings.queue_prompt_matches(key) {
            self.queue_prompt_after_turn()?;
        } else if self.info.runtime.keybindings.paste_image.matches(key)
            || matches!(
                (key.modifiers, key.code),
                (KeyModifiers::ALT, KeyCode::Char('v'))
            )
        {
            self.paste_clipboard_image();
        } else if self
            .info
            .runtime
            .keybindings
            .toggle_tool_output
            .matches(key)
        {
            self.toggle_latest_tool_output(terminal)?;
        } else if self
            .info
            .runtime
            .keybindings
            .reset_conversation
            .matches(key)
        {
            self.notify_status("reset is unavailable while a model turn is running");
        } else if self.info.runtime.keybindings.insert_newline.matches(key) {
            self.insert_input_char('\n');
        } else {
            return Ok(false);
        }
        self.input_ui.clear_paste_burst();
        self.ctrl_c_streak = 0;
        Ok(true)
    }

    pub(super) async fn handle_configurable_composer_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        if self.info.runtime.keybindings.queue_prompt_matches(key) {
            if agent.is_compacting() {
                self.queue_prompt_after_turn()?;
            } else {
                self.insert_input_char('\n');
            }
        } else if self.info.runtime.keybindings.paste_image.matches(key)
            || matches!(
                (key.modifiers, key.code),
                (KeyModifiers::ALT, KeyCode::Char('v'))
            )
        {
            self.paste_clipboard_image();
        } else if self
            .info
            .runtime
            .keybindings
            .toggle_tool_output
            .matches(key)
        {
            self.toggle_latest_tool_output(terminal)?;
        } else if self
            .info
            .runtime
            .keybindings
            .reset_conversation
            .matches(key)
        {
            if let Err(error) = agent.reset().await {
                // The conversation is still live, so report the failure instead
                // of clearing the UI as though a new session had started.
                self.insert_entry(&Entry::Error(format!(
                    "could not reset conversation: {error}"
                )));
            } else {
                self.info.session.session_id = None;
                self.pending_session_title = None;
                self.session_title_locked = false;
                self.reset_usage();
                self.usage.current_context = None;
                self.insert_entry(&Entry::Notice(
                    "conversation reset; next message starts a new session".into(),
                ));
            }
        } else if self.info.runtime.keybindings.insert_newline.matches(key) {
            self.insert_input_char('\n');
        } else {
            return Ok(false);
        }
        self.input_ui.clear_paste_burst();
        self.ctrl_c_streak = 0;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{backend::TestBackend, Terminal};

    use super::super::tests::test_app;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    // Covers: a non-default `queue_prompt` chord queues during a turn, including
    // when it collides with `insert_newline`, and Ctrl+Enter stays a fallback.
    // Owner: tui queue binding
    #[test]
    fn remapped_queue_prompt_queues_during_turn() {
        let mut app = test_app();
        app.info.runtime.keybindings.queue_prompt = "ctrl+j".parse().unwrap();
        app.input_ui.set_text("follow-up".into());
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();

        assert!(app
            .handle_configurable_running_key(
                key(KeyCode::Char('j'), KeyModifiers::CONTROL),
                &mut terminal,
            )
            .unwrap());
        assert_eq!(app.pending.queued_prompts().len(), 1);
        assert_eq!(app.pending.queued_prompts()[0].prompt, "follow-up");
        assert!(app.input_ui.text().is_empty());

        app.info.runtime.keybindings.queue_prompt = "ctrl+k".parse().unwrap();
        app.input_ui.set_text("later".into());
        assert!(app
            .handle_configurable_running_key(
                key(KeyCode::Char('k'), KeyModifiers::CONTROL),
                &mut terminal,
            )
            .unwrap());
        assert_eq!(app.pending.queued_prompts().len(), 2);
        assert_eq!(app.pending.queued_prompts()[1].prompt, "later");

        app.input_ui.set_text("ignored".into());
        assert!(!app
            .handle_configurable_running_key(key(KeyCode::Enter, KeyModifiers::ALT), &mut terminal)
            .unwrap());
        assert_eq!(app.input_ui.text(), "ignored");
        assert_eq!(app.pending.queued_prompts().len(), 2);

        assert!(app
            .handle_configurable_running_key(
                key(KeyCode::Enter, KeyModifiers::CONTROL),
                &mut terminal,
            )
            .unwrap());
        assert_eq!(app.pending.queued_prompts().len(), 3);
        assert!(app.input_ui.text().is_empty());
    }
}
