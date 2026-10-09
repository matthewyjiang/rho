use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::Backend, Terminal};

use crate::tui::DefaultTerminal;

use super::{App, InteractiveRuntime};

impl App {
    pub(super) fn handle_configurable_running_key<B: Backend>(
        &mut self,
        key: KeyEvent,
        terminal: &mut Terminal<B>,
    ) -> anyhow::Result<bool> {
        if self.info.runtime.keybindings.queue_prompt_matches(key) {
            self.queue_prompt_after_turn()?;
        } else if self
            .info
            .runtime
            .keybindings
            .cycle_permission_mode_matches(key)
        {
            self.queue_permission_mode_cycle();
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
            .cycle_streaming_mode
            .matches(key)
        {
            self.cycle_streaming_mode();
        } else if self
            .info
            .runtime
            .keybindings
            .reset_conversation_matches(key)
        {
            self.notify_status("/new is unavailable while a model turn is running");
        } else if self
            .info
            .runtime
            .keybindings
            .search_prompt_history
            .matches(key)
        {
            self.open_prompt_history_search();
        } else if self.info.runtime.keybindings.insert_newline.matches(key) {
            // Before search_transcript: configs that already bound the new
            // default chord to insert_newline keep inserting newlines.
            self.insert_input_char('\n');
        } else if self.info.runtime.keybindings.search_transcript.matches(key) {
            self.open_transcript_search(terminal)
                .map_err(|error| anyhow::anyhow!("could not read terminal size: {error}"))?;
        } else if self.info.runtime.keybindings.undo.matches(key) {
            // Undo and redo come last so their new defaults never shadow a
            // chord an existing config already bound.
            self.undo_input();
        } else if self.info.runtime.keybindings.redo.matches(key) {
            self.redo_input();
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
        } else if self
            .info
            .runtime
            .keybindings
            .cycle_permission_mode_matches(key)
        {
            self.cycle_permission_mode(agent).await?;
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
            .cycle_streaming_mode
            .matches(key)
        {
            self.cycle_streaming_mode();
        } else if self
            .info
            .runtime
            .keybindings
            .reset_conversation_matches(key)
        {
            self.execute_new_command(terminal, agent).await?;
        } else if self
            .info
            .runtime
            .keybindings
            .search_prompt_history
            .matches(key)
        {
            self.open_prompt_history_search();
        } else if self.info.runtime.keybindings.insert_newline.matches(key) {
            // Before search_transcript; see handle_configurable_running_key.
            self.insert_input_char('\n');
        } else if self.info.runtime.keybindings.search_transcript.matches(key) {
            self.open_transcript_search(terminal)?;
        } else if self.info.runtime.keybindings.undo.matches(key) {
            // Last; see handle_configurable_running_key.
            self.undo_input();
        } else if self.info.runtime.keybindings.redo.matches(key) {
            self.redo_input();
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

    // Covers: a config that already bound the new Ctrl+F search default to
    // `insert_newline` keeps inserting newlines instead of opening search.
    // Owner: tui keybinding dispatch
    #[test]
    fn insert_newline_wins_a_chord_shared_with_search_transcript() {
        let mut app = test_app();
        app.info.runtime.keybindings.insert_newline = "ctrl+f".parse().unwrap();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();

        assert!(app
            .handle_configurable_running_key(
                key(KeyCode::Char('f'), KeyModifiers::CONTROL),
                &mut terminal,
            )
            .unwrap());
        assert_eq!(app.input_ui.text(), "\n");
        assert!(matches!(
            app.input_ui.composer(),
            super::super::ComposerMode::Input
        ));
    }
}
