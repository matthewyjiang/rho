use ratatui::DefaultTerminal;

use super::{App, ComposerMode, InlineChoiceKeyOutcome, InlineChoicePending, InteractiveRuntime};

impl App {
    pub(super) async fn handle_inline_choice_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        let outcome = match self.input_ui.composer_mut() {
            ComposerMode::InlineChoice(modal) => modal.choice.handle_key(key),
            _ => return Ok(false),
        };

        match outcome {
            InlineChoiceKeyOutcome::Selected(value) => {
                let ComposerMode::InlineChoice(modal) = self.input_ui.take_composer() else {
                    unreachable!("inline choice checked above");
                };
                match modal.pending {
                    InlineChoicePending::CredentialStore { next } => {
                        self.submit_credential_store_choice(modal.choice, next, terminal, agent)
                            .await?;
                    }
                    InlineChoicePending::ContextHandoff(pending) => {
                        self.resolve_context_handoff(Some(&value), *pending, terminal, agent)
                            .await?;
                    }
                    InlineChoicePending::ClaudeCodeRelogin => {
                        self.submit_claude_code_relogin_choice(modal.choice, terminal)
                            .await?;
                    }
                    InlineChoicePending::ClaudeCodeLogout => {
                        self.submit_claude_code_logout_choice(modal.choice).await?;
                    }
                    InlineChoicePending::DeleteSession { session_id } => {
                        self.submit_delete_session_choice(&value, &session_id)?;
                    }
                    InlineChoicePending::DeleteWorkflowPlan { plan_id } => {
                        self.submit_delete_workflow_plan_choice(&value, &plan_id)?;
                    }
                    InlineChoicePending::DeleteWorkflowRun { run_id } => {
                        self.submit_delete_workflow_run_choice(&value, &run_id)?;
                    }
                }
            }
            InlineChoiceKeyOutcome::Cancelled => {
                let ComposerMode::InlineChoice(modal) = self.input_ui.take_composer() else {
                    unreachable!("inline choice checked above");
                };
                match modal.pending {
                    InlineChoicePending::CredentialStore { .. }
                    | InlineChoicePending::ClaudeCodeRelogin
                    | InlineChoicePending::ClaudeCodeLogout => {
                        self.status = self.busy_status_label().into();
                    }
                    InlineChoicePending::ContextHandoff(pending) => {
                        self.resolve_context_handoff(None, *pending, terminal, agent)
                            .await?;
                    }
                    InlineChoicePending::DeleteSession { .. } => {
                        self.open_resume_picker()?;
                    }
                    InlineChoicePending::DeleteWorkflowPlan { .. }
                    | InlineChoicePending::DeleteWorkflowRun { .. } => {
                        self.open_workflow_hub_or_report();
                    }
                }
            }
            InlineChoiceKeyOutcome::Handled => {}
        }
        self.input_ui.clear_paste_burst();
        self.ctrl_c_streak = 0;
        Ok(true)
    }
}
