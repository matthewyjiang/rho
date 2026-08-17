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
                    InlineChoicePending::ClaudeCodeLogin => {
                        self.submit_claude_code_login_choice(modal.choice, terminal)
                            .await?;
                    }
                    InlineChoicePending::ClaudeCodeRelogin => {
                        self.submit_claude_code_relogin_choice(modal.choice, terminal)
                            .await?;
                    }
                    InlineChoicePending::ClaudeCodeLogout => {
                        self.submit_claude_code_logout_choice(modal.choice).await?;
                    }
                    InlineChoicePending::DeleteSession { target } => {
                        self.submit_delete_session_choice(&value, &target, modal.parent_picker)?;
                    }
                    InlineChoicePending::DeleteDirectorySessions { cwd, targets } => {
                        self.submit_delete_directory_sessions_choice(
                            &value,
                            &cwd,
                            &targets,
                            modal.parent_picker,
                        )?;
                    }
                    InlineChoicePending::CleanupMissingSessionDirectories { targets } => {
                        self.submit_cleanup_missing_session_directories_choice(
                            &value,
                            &targets,
                            modal.parent_picker,
                        )?;
                    }
                    InlineChoicePending::DeleteWorkflowPlan { plan_id } => {
                        self.submit_delete_workflow_plan_choice(&value, &plan_id)?;
                    }
                    InlineChoicePending::DeleteWorkflowRun { run_id } => {
                        self.submit_delete_workflow_run_choice(&value, &run_id)?;
                    }
                    InlineChoicePending::PromptHistoryLimit { new_limit } => {
                        self.submit_prompt_history_limit_choice(&value, new_limit)?;
                    }
                    InlineChoicePending::ClearPromptHistory => {
                        self.submit_clear_prompt_history_choice(&value)?;
                    }
                }
            }
            InlineChoiceKeyOutcome::Cancelled => {
                let ComposerMode::InlineChoice(modal) = self.input_ui.take_composer() else {
                    unreachable!("inline choice checked above");
                };
                match modal.pending {
                    InlineChoicePending::CredentialStore { .. }
                    | InlineChoicePending::ClaudeCodeLogin
                    | InlineChoicePending::ClaudeCodeRelogin
                    | InlineChoicePending::ClaudeCodeLogout => {
                        self.set_status(self.busy_status_label());
                    }
                    InlineChoicePending::ContextHandoff(pending) => {
                        self.resolve_context_handoff(None, *pending, terminal, agent)
                            .await?;
                    }
                    InlineChoicePending::DeleteSession { .. }
                    | InlineChoicePending::DeleteDirectorySessions { .. }
                    | InlineChoicePending::CleanupMissingSessionDirectories { .. } => {
                        self.restore_session_choice_parent(modal.parent_picker);
                    }
                    InlineChoicePending::DeleteWorkflowPlan { .. }
                    | InlineChoicePending::DeleteWorkflowRun { .. } => {
                        self.open_workflow_hub_or_report();
                    }
                    InlineChoicePending::PromptHistoryLimit { .. } => {
                        self.restore_prompt_history_config(
                            super::config_picker::PROMPT_HISTORY_LIMIT_VALUE,
                        )?;
                    }
                    InlineChoicePending::ClearPromptHistory => {
                        self.restore_prompt_history_config(
                            super::config_picker::CLEAR_PROMPT_HISTORY_VALUE,
                        )?;
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
