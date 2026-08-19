//! Submit a CLI `--prompt` once the interactive composer is free.

use ratatui::DefaultTerminal;

use super::{exclusive_screen::ExclusiveOccupant, App, InteractiveRuntime};

impl App {
    /// Start the take-once CLI prompt after first paint.
    ///
    /// Waits while setup, in-place attach, or another composer mode owns the
    /// keyboard, then submits through the same path as Enter. MCP connect still
    /// holds the turn until servers settle.
    pub(super) async fn start_startup_prompt(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        if !matches!(self.exclusive, ExclusiveOccupant::Session)
            || self.input_ui.composer().blocks_held_turn_start()
            || self.is_ui_busy()
        {
            return Ok(false);
        }
        let Some(prompt) = self.info.session.startup_prompt.take() else {
            return Ok(false);
        };
        self.input_ui.set_text(prompt);
        self.input_ui.set_cursor(self.input_ui.char_len());
        self.submit(terminal, agent).await?;
        Ok(true)
    }
}
