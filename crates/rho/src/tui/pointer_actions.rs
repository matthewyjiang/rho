//! Async follow-ups that pointer events hand to the event loops.
//!
//! Pointer handling is sync and backend-generic, so a click that must submit,
//! confirm, or run a command records a [`PointerAction`] instead. The idle and
//! running loops take it right after the mouse event and run it here with the
//! terminal and runtime, through the same paths the equivalent keys use.

use ratatui::DefaultTerminal;

use super::{
    app_state::PointerAction, command_actions::CommandSubmission, commands, App, ComposerMode,
    InteractiveRuntime, TurnPrompt,
};

impl App {
    /// Runs a pointer action from the idle loop.
    pub(super) async fn run_idle_pointer_action(
        &mut self,
        action: PointerAction,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        match action {
            PointerAction::ConfirmInlineChoice => {
                if let ComposerMode::InlineChoice(modal) = self.input_ui.composer() {
                    let value = modal.choice.selected_value().to_owned();
                    self.resolve_inline_choice(value, terminal, agent).await?;
                }
            }
            PointerAction::SubmitPicker => {
                if matches!(self.input_ui.composer(), ComposerMode::Picker(_)) {
                    self.submit_picker_selection(terminal, agent).await?;
                }
            }
            PointerAction::RunCommand(text) => {
                if let Some(invocation) = self.pointer_command(&text) {
                    // The click carries its own command text and no media or
                    // pasted segments, so the composer draft stays untouched.
                    let submission = CommandSubmission::new(
                        invocation,
                        TurnPrompt::command(text.clone(), text),
                        Vec::new(),
                        Vec::new(),
                    );
                    self.execute_command(submission, terminal, agent).await?;
                }
            }
        }
        Ok(())
    }

    /// Runs a pointer action from the running-turn loop. Inline choices only
    /// resolve while idle, so that request is dropped here; commands take the
    /// during-turn dispatch, which reports the ones a turn blocks.
    pub(super) async fn run_running_pointer_action(
        &mut self,
        action: PointerAction,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        match action {
            PointerAction::ConfirmInlineChoice => {}
            PointerAction::SubmitPicker => {
                if matches!(self.input_ui.composer(), ComposerMode::Picker(_)) {
                    self.submit_picker_selection_during_turn().await?;
                }
            }
            PointerAction::RunCommand(text) => {
                if let Some(invocation) = self.pointer_command(&text) {
                    self.execute_command_during_turn(invocation, terminal)
                        .await?;
                }
            }
        }
        Ok(())
    }

    /// Parses a pointer-issued command. Only an idle `Input` composer runs
    /// one, so a click never discards a modal the user is in the middle of.
    fn pointer_command(&mut self, text: &str) -> Option<commands::CommandInvocation> {
        if !matches!(self.input_ui.composer(), ComposerMode::Input) {
            return None;
        }
        match commands::parse_command(text) {
            Ok(invocation) => invocation,
            Err(error) => {
                self.set_status(format!("could not run {text}: {error}"));
                None
            }
        }
    }
}
