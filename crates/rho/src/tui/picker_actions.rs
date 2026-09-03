//! Feature execution for picker confirm, escape, and row-delete.
//!
//! Picker widget state lives in [`super::picker`]. This module is the one App
//! layer that matches [`PickerAction`] and calls owning features.

use ratatui::DefaultTerminal;
use rho_providers::model::catalog;

use super::{
    claude_login, config_picker, model_picker,
    picker::{ConfigParentRow, DuringTurnSelect, PickerAction, PickerTurn},
    provider_picker, App, ComposerMode, Entry, InteractiveRuntime, UiPicker,
};

enum PickerCommit<'a> {
    Idle {
        terminal: &'a mut DefaultTerminal,
        agent: &'a mut InteractiveRuntime,
    },
    DuringTurn,
}

impl PickerCommit<'_> {
    fn turn(&self) -> PickerTurn {
        match self {
            Self::Idle { .. } => PickerTurn::Idle,
            Self::DuringTurn => PickerTurn::DuringTurn,
        }
    }
}

impl App {
    pub(in crate::tui) async fn submit_picker_selection(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        self.commit_picker(PickerCommit::Idle { terminal, agent })
            .await
    }

    pub(in crate::tui) async fn submit_picker_selection_during_turn(
        &mut self,
    ) -> anyhow::Result<()> {
        self.commit_picker(PickerCommit::DuringTurn).await
    }

    async fn commit_picker(&mut self, mut commit: PickerCommit<'_>) -> anyhow::Result<()> {
        let turn = commit.turn();
        let Some((action, value)) = self.active_picker_selection() else {
            self.input_ui.set_composer(ComposerMode::Input);
            self.set_status(match turn {
                PickerTurn::Idle => "ready",
                PickerTurn::DuringTurn => "running",
            });
            return Ok(());
        };

        let return_picker = self.take_picker_parent_after_selection(action);
        // Idle SelectModel restores its config parent inside the model arm
        // (`request_model_selection_from_config_picker`). Every other path
        // restores after the commit, including during-turn SelectModel.
        let (idle_model_parent, restore_parent) = match (turn, action) {
            (PickerTurn::Idle, PickerAction::SelectModel) => (return_picker, None),
            _ => (None, return_picker),
        };

        if let PickerTurn::DuringTurn = turn {
            match action.during_turn_select() {
                DuringTurnSelect::Unavailable(message) => {
                    if !action.keeps_composer_open(turn) {
                        self.input_ui.set_composer(ComposerMode::Input);
                    }
                    self.set_status(message);
                    return self
                        .restore_config_parent(restore_parent, /*preserve_status*/ false);
                }
                DuringTurnSelect::CloseOnly => {
                    if !action.keeps_composer_open(turn) {
                        self.input_ui.set_composer(ComposerMode::Input);
                    }
                    self.set_status("running");
                    return self
                        .restore_config_parent(restore_parent, /*preserve_status*/ false);
                }
                DuringTurnSelect::Apply => {}
            }
        }

        if !action.keeps_composer_open(turn) {
            self.input_ui.set_composer(ComposerMode::Input);
        }

        let result = self
            .execute_picker_action(action, &value, &mut commit, idle_model_parent)
            .await;
        if result.is_ok() {
            let preserve_status = matches!(turn, PickerTurn::Idle);
            self.restore_config_parent(restore_parent, preserve_status)?;
        }
        result
    }

    async fn execute_picker_action(
        &mut self,
        action: PickerAction,
        value: &str,
        commit: &mut PickerCommit<'_>,
        idle_model_parent: Option<(UiPicker, &'static str)>,
    ) -> anyhow::Result<()> {
        match action {
            PickerAction::SelectModel => {
                self.commit_select_model(value, commit, idle_model_parent)
                    .await
            }
            PickerAction::SelectInternalAgentModel => {
                let PickerCommit::Idle { agent, .. } = commit else {
                    unreachable!("internal agent model commit is idle-only");
                };
                self.commit_internal_agent_model(value, agent).await
            }
            PickerAction::LoginGroup => {
                let PickerCommit::Idle { terminal, agent } = commit else {
                    unreachable!("login group commit is idle-only");
                };
                self.commit_login_group(value, terminal, agent).await
            }
            PickerAction::LoginProvider => match claude_login::SignInTarget::parse(value) {
                claude_login::SignInTarget::ClaudeCode => self.execute_claude_code_login().await,
                claude_login::SignInTarget::NewCustomHost { api } => {
                    self.start_custom_provider_onboarding(api);
                    Ok(())
                }
                claude_login::SignInTarget::Provider(provider) => {
                    let PickerCommit::Idle { terminal, agent } = commit else {
                        unreachable!("login provider commit is idle-only");
                    };
                    self.start_login_for_provider(&provider, terminal, agent)
                        .await
                }
            },
            PickerAction::LogoutProvider => match claude_login::SignInTarget::parse(value) {
                claude_login::SignInTarget::ClaudeCode => self.execute_claude_code_logout().await,
                // Nothing is stored for a host that was never created.
                claude_login::SignInTarget::NewCustomHost { .. } => Ok(()),
                claude_login::SignInTarget::Provider(provider) => {
                    let PickerCommit::Idle { agent, .. } = commit else {
                        unreachable!("logout commit is idle-only");
                    };
                    self.logout_provider(&provider, agent).await
                }
            },
            PickerAction::SwitchAuthMode => {
                let PickerCommit::Idle { agent, .. } = commit else {
                    unreachable!("auth-mode commit is idle-only");
                };
                self.switch_active_auth_mode(value, agent).await
            }
            PickerAction::RefreshModelList => {
                let PickerCommit::Idle { terminal, .. } = commit else {
                    unreachable!("refresh-model-list commit is idle-only");
                };
                self.refresh_model_lists(value, terminal).await
            }
            PickerAction::InsertSkillCommand => {
                if matches!(commit.turn(), PickerTurn::Idle) {
                    self.input_ui.set_shell_mode(None);
                }
                self.input_ui
                    .set_text_and_cursor(format!("/skill:{value}"), self.input_char_len());
                self.input_ui.set_command_palette_dismissed(true);
                self.set_status("skill command inserted");
                Ok(())
            }
            PickerAction::ResumeSession => {
                let PickerCommit::Idle { terminal, agent } = commit else {
                    unreachable!("resume commit is idle-only");
                };
                self.submit_resume_selection(value, terminal, agent).await
            }
            PickerAction::ManageSessions => {
                let PickerCommit::Idle { terminal, agent } = commit else {
                    unreachable!("sessions commit is idle-only");
                };
                self.submit_sessions_selection(value, terminal, agent).await
            }
            PickerAction::SelectTreeNode => {
                let PickerCommit::Idle { terminal, agent } = commit else {
                    unreachable!("tree commit is idle-only");
                };
                self.submit_tree_selection(value, terminal, agent).await
            }
            PickerAction::SelectRewindCheckpoint => {
                let PickerCommit::Idle { agent, .. } = commit else {
                    unreachable!("rewind preview is idle-only");
                };
                self.submit_rewind_preview(value, agent)
            }
            PickerAction::ConfirmRewindCheckpoint => {
                let PickerCommit::Idle { terminal, agent } = commit else {
                    unreachable!("rewind confirm is idle-only");
                };
                self.submit_rewind_confirmation(value, terminal, agent)
                    .await
            }
            PickerAction::Config => match commit {
                PickerCommit::Idle { terminal, agent } => {
                    self.submit_config_selection(value, agent, terminal).await
                }
                PickerCommit::DuringTurn => self.submit_config_selection_during_turn(value).await,
            },
            PickerAction::SelectTheme => self.submit_theme_selection(value),
            PickerAction::ViewAgent => self.submit_view_agent_selection(value),
            PickerAction::EditAgent => {
                let PickerCommit::Idle { terminal, .. } = commit else {
                    unreachable!("agent editor commit is idle-only");
                };
                self.submit_edit_agent_selection(value, terminal).await
            }
            PickerAction::Workflow => {
                let PickerCommit::Idle { terminal, agent } = commit else {
                    unreachable!("workflow commit is idle-only");
                };
                self.submit_workflow_selection(value, terminal, agent).await
            }
            PickerAction::AttachSubagent => {
                self.submit_attach_selection(value);
                Ok(())
            }
            PickerAction::Dismiss | PickerAction::ViewMcpServers => Ok(()),
        }
    }

    async fn commit_select_model(
        &mut self,
        value: &str,
        commit: &mut PickerCommit<'_>,
        idle_model_parent: Option<(UiPicker, &'static str)>,
    ) -> anyhow::Result<()> {
        self.refresh_available_auths();
        match self.resolve_model_selection(
            value,
            &self.info.runtime.provider,
            &self.info.runtime.auth,
        ) {
            Ok(selection) => match commit {
                PickerCommit::Idle { agent, .. } => {
                    if let Some((picker, selected_value)) = idle_model_parent {
                        self.request_model_selection_from_config_picker(
                            selection,
                            picker,
                            selected_value,
                            agent,
                        )
                        .await
                    } else {
                        self.request_model_selection(selection, agent).await
                    }
                }
                PickerCommit::DuringTurn => self.queue_model_selection(selection),
            },
            Err(err) => {
                self.insert_entry(&Entry::Error(err.to_string()));
                self.set_status("model switch failed");
                Ok(())
            }
        }
    }

    async fn commit_internal_agent_model(
        &mut self,
        value: &str,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let Some(target) = self.internal_agent_model_target.clone() else {
            self.set_status("internal agent model selection expired");
            return Ok(());
        };
        let id = target.id.as_str();
        let selected = match model_picker::parse_internal_agent_model_row(value) {
            model_picker::InternalAgentModelRow::Conversation => {
                self.select_internal_agent_model(id, None)?;
                true
            }
            model_picker::InternalAgentModelRow::ClaudeCode { model } => {
                self.select_internal_agent_claude_model(id, model)?;
                true
            }
            model_picker::InternalAgentModelRow::RhoModel(reference) => {
                self.refresh_available_auths();
                let current = self.internal_agent_rho_model_or_conversation(id);
                match self.resolve_model_selection(&reference, &current.provider, &current.auth) {
                    Ok(selection) => {
                        self.select_internal_agent_model(id, Some(selection.selection))?;
                        true
                    }
                    Err(err) => {
                        self.insert_entry(&Entry::Error(err.to_string()));
                        self.set_status("internal agent model switch failed");
                        false
                    }
                }
            }
        };
        self.finish_internal_agent_model_flow(target, selected, agent)
            .await
    }

    async fn commit_login_group(
        &mut self,
        value: &str,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        // A single-method group short-circuits to that method's value,
        // which may name the external runtime or new-host onboarding
        // rather than a group id.
        let value = match claude_login::SignInTarget::parse(value) {
            claude_login::SignInTarget::ClaudeCode => {
                return self.execute_claude_code_login().await
            }
            claude_login::SignInTarget::NewCustomHost { api } => {
                self.start_custom_provider_onboarding(api);
                return Ok(());
            }
            claude_login::SignInTarget::Provider(provider) => provider,
        };
        let Some(group) = catalog::login_group(&value) else {
            self.insert_entry(&Entry::Error(format!(
                "unsupported login provider group '{value}'"
            )));
            self.set_status("login failed");
            return Ok(());
        };
        match provider_picker::login_group_next(group) {
            provider_picker::LoginGroupNext::Provider(provider) => {
                self.start_login_for_provider(&provider, terminal, agent)
                    .await
            }
            provider_picker::LoginGroupNext::MethodPicker(child) => {
                self.open_child_picker(*child);
                Ok(())
            }
        }
    }

    fn take_picker_parent_after_selection(
        &mut self,
        action: PickerAction,
    ) -> Option<(UiPicker, &'static str)> {
        let selected_value = config_parent_value(action.config_parent_row()?);
        match self.input_ui.composer_mut() {
            ComposerMode::Picker(picker) => {
                picker.take_parent().map(|parent| (parent, selected_value))
            }
            _ => None,
        }
    }

    fn restore_config_parent(
        &mut self,
        return_picker: Option<(UiPicker, &'static str)>,
        preserve_status: bool,
    ) -> anyhow::Result<()> {
        let Some((picker, selected_value)) = return_picker else {
            return Ok(());
        };
        let feedback = preserve_status.then(|| self.status().to_string());
        self.open_main_config_picker(selected_value, picker.filter)?;
        if let Some(feedback) = feedback.filter(|feedback| !feedback.is_empty()) {
            self.set_status(feedback);
        }
        Ok(())
    }

    pub(in crate::tui) fn apply_picker_row_delete(&mut self) -> anyhow::Result<()> {
        let Some((action, _)) = self.active_picker_selection() else {
            return Ok(());
        };
        match action {
            PickerAction::ResumeSession => self.prompt_delete_selected_session(),
            PickerAction::ManageSessions => self.prompt_delete_selected_sessions_item(),
            PickerAction::Workflow => self.prompt_delete_selected_workflow_item(),
            PickerAction::SelectModel
            | PickerAction::SelectInternalAgentModel
            | PickerAction::LoginGroup
            | PickerAction::LoginProvider
            | PickerAction::LogoutProvider
            | PickerAction::SwitchAuthMode
            | PickerAction::RefreshModelList
            | PickerAction::InsertSkillCommand
            | PickerAction::ViewAgent
            | PickerAction::ViewMcpServers
            | PickerAction::SelectTreeNode
            | PickerAction::SelectRewindCheckpoint
            | PickerAction::ConfirmRewindCheckpoint
            | PickerAction::Config
            | PickerAction::SelectTheme
            | PickerAction::EditAgent
            | PickerAction::AttachSubagent
            | PickerAction::Dismiss => Ok(()),
        }
    }

    pub(in crate::tui) fn handle_picker_escape(&mut self, running: bool) -> anyhow::Result<()> {
        let leaving_edit_agent = matches!(
            self.input_ui.composer(),
            ComposerMode::Picker(picker) if picker.is_edit_agent()
        );
        if leaving_edit_agent {
            return self.handle_edit_agent_escape();
        }

        let leaving_theme = matches!(
            self.input_ui.composer(),
            ComposerMode::Picker(picker) if picker.is_theme()
        );
        if leaving_theme {
            self.cancel_theme_preview();
        }

        let sessions_picker = matches!(
            self.input_ui.composer(),
            ComposerMode::Picker(picker) if picker.is_manage_sessions()
        );
        let internal_agent_model_picker = matches!(
            self.input_ui.composer(),
            ComposerMode::Picker(picker) if picker.is_internal_agent_model()
        );
        if self.pop_picker_level() {
            if sessions_picker {
                self.sessions_hub_state.navigate_back();
            }
            if internal_agent_model_picker {
                self.cancel_permission_classifier_model_prompt(/*restore_input*/ false);
            }
        } else {
            if sessions_picker {
                self.sessions_hub_state.clear();
            }
            self.input_ui.set_composer(ComposerMode::Input);
            if !self.cancel_advisor_model_prompt()
                && !self.cancel_permission_classifier_model_prompt(/*restore_input*/ true)
            {
                self.set_status(if running { "running" } else { "ready" });
            }
            // Escaping the setup picker leaves setup. Escaping a pending
            // login does not take this path; that restore is
            // `restore_after_cancelled_login()`.
            self.dismiss_setup_screen();
        }
        Ok(())
    }
}

fn config_parent_value(row: ConfigParentRow) -> &'static str {
    match row {
        ConfigParentRow::ConversationModel => config_picker::CONVERSATION_MODEL_VALUE,
        ConfigParentRow::Theme => config_picker::THEME_VALUE,
        ConfigParentRow::LogoutProvider => config_picker::PROVIDER_LOGOUT_VALUE,
        ConfigParentRow::SwitchAuthMode => config_picker::SWITCH_AUTH_MODE_VALUE,
        ConfigParentRow::RefreshModelList => config_picker::REFRESH_MODEL_LIST_VALUE,
    }
}
