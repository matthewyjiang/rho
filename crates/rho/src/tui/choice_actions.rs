use ratatui::DefaultTerminal;

use super::composer_pointer::ChoiceClick;
use super::sessions_hub_tasks::SessionsDelete;
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
                if !self.resolve_inline_choice(value, terminal, agent).await? {
                    return Ok(true);
                }
            }
            InlineChoiceKeyOutcome::Cancelled => {
                let ComposerMode::InlineChoice(modal) = self.input_ui.take_composer() else {
                    unreachable!("inline choice checked above");
                };
                match modal.pending {
                    InlineChoicePending::ComputerInstall => {
                        self.confirm_computer_installation("cancel", agent)
                    }
                    InlineChoicePending::ComputerAccess => {
                        self.confirm_computer_access("cancel", agent)
                    }
                    InlineChoicePending::CredentialStore { .. } => {
                        if let Some(parent) = modal.parent_picker {
                            self.set_status_quiet(parent.title.clone());
                            self.input_ui.set_composer(ComposerMode::Picker(*parent));
                        } else {
                            self.restore_after_cancelled_login();
                        }
                    }
                    InlineChoicePending::ClaudeCodeLogin
                    | InlineChoicePending::ClaudeCodeRelogin
                    | InlineChoicePending::ClaudeCodeLogout => {
                        self.set_status(self.busy_status_label());
                    }
                    InlineChoicePending::ContextHandoff(pending) => {
                        self.resolve_context_handoff(None, *pending, terminal, agent)
                            .await?;
                    }
                    InlineChoicePending::ConfirmSend(pending) => {
                        self.resolve_send_confirm(None, *pending, terminal, agent)
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
                        self.open_main_config_picker_selected(
                            super::config_picker::PROMPT_HISTORY_LIMIT_VALUE,
                        )?;
                    }
                    InlineChoicePending::ClearPromptHistory => {
                        self.open_main_config_picker_selected(
                            super::config_picker::CLEAR_PROMPT_HISTORY_VALUE,
                        )?;
                    }
                    InlineChoicePending::TestWebSearch => {
                        self.submit_web_search_test_choice("cancel", modal.parent_picker)?;
                    }
                }
            }
            InlineChoiceKeyOutcome::Handled => {}
        }
        self.input_ui.clear_paste_burst();
        self.ctrl_c_streak = 0;
        Ok(true)
    }

    /// A click focuses an available option. A double click also asks the idle
    /// loop to confirm it like Enter, since [`Self::resolve_inline_choice`]
    /// needs the terminal and runtime the pointer handler does not have.
    pub(super) fn click_inline_choice(&mut self, index: usize, click: ChoiceClick) {
        let ComposerMode::InlineChoice(modal) = self.input_ui.composer_mut() else {
            return;
        };
        if !modal.choice.focus_option(index) {
            return;
        }
        match click {
            ChoiceClick::Single => {}
            ChoiceClick::Double => self.input_ui.request_inline_choice_confirm(),
        }
    }

    /// Confirms the open inline choice with `value`, as Enter does, then
    /// closes it and runs its pending action. Keys and pointer double clicks
    /// share this. `false` when nothing resolved: no inline choice is open, or
    /// the focused option requires full visibility and the disclosure is
    /// clipped (the choice stays open with a status explaining why).
    pub(super) async fn resolve_inline_choice(
        &mut self,
        value: String,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        let requires_full_visibility = match self.input_ui.composer() {
            ComposerMode::InlineChoice(modal) => modal.choice.selected_requires_full_visibility(),
            _ => return Ok(false),
        };
        if requires_full_visibility && !self.inline_choice_fully_visible(terminal) {
            return Ok(false);
        }
        let ComposerMode::InlineChoice(modal) = self.input_ui.take_composer() else {
            unreachable!("inline choice checked above");
        };
        match modal.pending {
            InlineChoicePending::ComputerInstall => {
                self.confirm_computer_installation(&value, agent)
            }
            InlineChoicePending::ComputerAccess => self.confirm_computer_access(&value, agent),
            InlineChoicePending::CredentialStore { next } => {
                // Resume login with its navigation context, so a subsequent
                // flow picker can attach to the original provider picker.
                if let Some(parent) = modal.parent_picker {
                    self.input_ui.set_composer(ComposerMode::Picker(*parent));
                }
                self.submit_credential_store_choice(modal.choice, next, terminal, agent)
                    .await?;
            }
            InlineChoicePending::ContextHandoff(pending) => {
                self.resolve_context_handoff(Some(&value), *pending, terminal, agent)
                    .await?;
            }
            InlineChoicePending::ConfirmSend(pending) => {
                self.resolve_send_confirm(Some(&value), *pending, terminal, agent)
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
                self.submit_sessions_delete_choice(
                    &value,
                    SessionsDelete::One(target),
                    modal.parent_picker,
                );
            }
            InlineChoicePending::DeleteDirectorySessions { cwd, targets } => {
                self.submit_sessions_delete_choice(
                    &value,
                    SessionsDelete::Directory { cwd, targets },
                    modal.parent_picker,
                );
            }
            InlineChoicePending::CleanupMissingSessionDirectories { targets } => {
                self.submit_sessions_delete_choice(
                    &value,
                    SessionsDelete::CleanupMissing(targets),
                    modal.parent_picker,
                );
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
            InlineChoicePending::TestWebSearch => {
                self.submit_web_search_test_choice(&value, modal.parent_picker)?;
            }
        }
        Ok(true)
    }

    /// Submission must not bypass a disclosure clipped by the terminal viewport.
    fn inline_choice_fully_visible(&mut self, terminal: &DefaultTerminal) -> bool {
        let Ok(size) = terminal.size() else {
            self.set_status("could not check choice visibility; resize the terminal or Esc cancel");
            return false;
        };
        let frame = self.frame_context(ratatui::layout::Rect::new(0, 0, size.width, size.height));
        let needed = frame.composer.lines.len();
        let visible = usize::from(frame.layout.composer.height);
        if frame.layout.composer_start != 0 || visible < needed {
            self.set_status(format!("enlarge terminal to review choice: disclosure needs {needed} rows, {visible} visible; Esc cancel"));
            return false;
        }
        true
    }
}
