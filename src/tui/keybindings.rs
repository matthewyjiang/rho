use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::DefaultTerminal;

use super::{Agent, App, Entry};

impl App {
    pub(super) fn handle_configurable_running_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
    ) -> std::io::Result<bool> {
        if self.info.keybindings.paste_image.matches(key)
            || matches!(
                (key.modifiers, key.code),
                (KeyModifiers::ALT, KeyCode::Char('v'))
            )
        {
            self.paste_clipboard_image();
        } else if self.info.keybindings.toggle_tool_output.matches(key) {
            self.toggle_latest_tool_output(terminal)?;
        } else if self.info.keybindings.reset_conversation.matches(key) {
            self.notify_status("reset is unavailable while a model turn is running");
        } else if self.info.keybindings.insert_newline.matches(key) {
            self.insert_input_char('\n');
        } else {
            return Ok(false);
        }
        self.paste_burst.clear();
        self.ctrl_c_streak = 0;
        Ok(true)
    }

    pub(super) fn handle_configurable_composer_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> std::io::Result<bool> {
        if self.info.keybindings.paste_image.matches(key)
            || matches!(
                (key.modifiers, key.code),
                (KeyModifiers::ALT, KeyCode::Char('v'))
            )
        {
            self.paste_clipboard_image();
        } else if self.info.keybindings.toggle_tool_output.matches(key) {
            self.toggle_latest_tool_output(terminal)?;
        } else if self.info.keybindings.reset_conversation.matches(key) {
            agent.reset();
            self.info.session_id = None;
            agent.set_session_id(None);
            agent.clear_history_sink();
            self.cumulative_usage = None;
            self.latest_usage = None;
            self.current_context = None;
            self.insert_entry(&Entry::Notice(
                "conversation reset; next message starts a new session".into(),
            ));
        } else if self.info.keybindings.insert_newline.matches(key) {
            self.insert_input_char('\n');
        } else {
            return Ok(false);
        }
        self.paste_burst.clear();
        self.ctrl_c_streak = 0;
        Ok(true)
    }
}
