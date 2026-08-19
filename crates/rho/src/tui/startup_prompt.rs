//! Submit a CLI `--prompt` once the interactive composer is free.

use ratatui::DefaultTerminal;

use super::{App, InteractiveRuntime};

impl App {
    /// Start the take-once CLI prompt after first paint.
    ///
    /// Waits while setup or another composer mode owns the keyboard, then
    /// submits through the same path as Enter. MCP connect still holds the
    /// turn until servers settle.
    pub(super) async fn start_startup_prompt(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        if self.info.session.startup_prompt.is_none() {
            return Ok(false);
        }
        if self.setup_screen.is_some()
            || self.input_ui.composer().blocks_held_turn_start()
            || self.is_ui_busy()
        {
            return Ok(false);
        }
        let prompt = self
            .info
            .session
            .startup_prompt
            .take()
            .expect("startup prompt checked above");
        self.input_ui.set_text(prompt);
        self.input_ui.set_cursor(self.input_ui.char_len());
        self.submit(terminal, agent).await?;
        Ok(true)
    }
}
