//! Open, close, parent restore, and selection routing for an active picker.

use ratatui::DefaultTerminal;
use rho_providers::model::catalog;

use super::{
    overlay_layout::picker_overlay_layout, DuringTurnSelect, PickerAction, PickerRowDelete,
    UiPicker,
};
use crate::tui::{
    agent_editor, claude_login, config_picker, model_picker, provider_picker, App, ComposerMode,
    Entry, InteractiveRuntime,
};

impl App {
    pub(in crate::tui) fn clamp_overlay_detail_scroll(&mut self, terminal: &DefaultTerminal) {
        let Ok(size) = terminal.size() else {
            return;
        };
        let ComposerMode::Picker(picker) = self.input_ui.composer_mut() else {
            return;
        };
        if !picker.has_scrollable_detail() {
            return;
        }
        let layout = picker_overlay_layout(
            ratatui::layout::Rect::new(0, 0, size.width, size.height),
            picker.overlay_sizing(),
        );
        if let Some(viewport) = layout.detail_viewport() {
            picker.clamp_detail_scroll(viewport);
        }
    }

    pub(in crate::tui) fn open_child_picker(&mut self, child: UiPicker) {
        let previous = self.input_ui.take_composer();
        let ComposerMode::Picker(parent) = previous else {
            unreachable!("child picker requires an active parent picker")
        };
        self.set_status_quiet(child.title.clone());
        self.input_ui
            .set_composer(ComposerMode::Picker(child.with_parent(parent)));
    }

    pub(in crate::tui) fn pop_picker_level(&mut self) -> bool {
        let parent = match self.input_ui.composer_mut() {
            ComposerMode::Picker(picker) => picker.take_parent(),
            _ => None,
        };
        let Some(parent) = parent else {
            return false;
        };
        self.set_status_quiet(parent.title.clone());
        self.input_ui.set_composer(ComposerMode::Picker(parent));
        true
    }

    pub(in crate::tui) fn picker_space_confirms_selection(&self) -> bool {
        matches!(
            self.input_ui.composer(),
            ComposerMode::Picker(picker) if picker.space_confirms_selection()
        )
    }

    pub(in crate::tui) fn restore_picker_position(
        picker: &mut UiPicker,
        selected_value: &str,
        filter: String,
    ) {
        picker.filter = filter;
        if let Some(index) = picker
            .items
            .iter()
            .position(|item| item.value == selected_value)
        {
            picker.selected = index;
            if picker.selected_item().is_some() {
                return;
            }
        }
        picker.filter.clear();
        if let Some(index) = picker
            .items
            .iter()
            .position(|item| item.value == selected_value)
        {
            picker.selected = index;
        } else {
            picker.select_first_match();
        }
    }

    #[cfg(test)]
    pub(in crate::tui) fn active_picker_value(&self) -> Option<String> {
        self.active_picker_selection().map(|(_, value)| value)
    }

    fn active_picker_selection(&self) -> Option<(PickerAction, String)> {
        let ComposerMode::Picker(picker) = self.input_ui.composer() else {
            return None;
        };
        picker
            .selected_item()
            .map(|item| (picker.action, item.value.clone()))
    }

    fn take_picker_parent_after_selection(
        &mut self,
        action: PickerAction,
    ) -> Option<(UiPicker, &'static str)> {
        let selected_value = match action {
            PickerAction::SelectModel => config_picker::CONVERSATION_MODEL_VALUE,
            PickerAction::SelectTheme => config_picker::THEME_VALUE,
            PickerAction::LogoutProvider => config_picker::PROVIDER_LOGOUT_VALUE,
            PickerAction::SwitchAuthMode => config_picker::SWITCH_AUTH_MODE_VALUE,
            PickerAction::RefreshModelList => config_picker::REFRESH_MODEL_LIST_VALUE,
            _ => return None,
        };
        match self.input_ui.composer_mut() {
            ComposerMode::Picker(picker) => {
                picker.take_parent().map(|parent| (parent, selected_value))
            }
            _ => None,
        }
    }

    pub(in crate::tui) fn apply_picker_row_delete(&mut self) -> anyhow::Result<()> {
        let Some((action, _)) = self.active_picker_selection() else {
            return Ok(());
        };
        match action.row_delete() {
            Some(PickerRowDelete::ResumeSession) => self.prompt_delete_selected_session(),
            Some(PickerRowDelete::ManageSessions) => self.prompt_delete_selected_sessions_item(),
            Some(PickerRowDelete::Workflow) => self.prompt_delete_selected_workflow_item(),
            None => Ok(()),
        }
    }

    pub(in crate::tui) async fn submit_picker_selection(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let Some((action, value)) = self.active_picker_selection() else {
            self.input_ui.set_composer(ComposerMode::Input);
            self.set_status("ready");
            return Ok(());
        };

        let return_picker = self.take_picker_parent_after_selection(action);
        let (model_return_picker, other_return_picker) =
            if matches!(action, PickerAction::SelectModel) {
                (return_picker, None)
            } else {
                (None, return_picker)
            };
        if !action.keeps_composer_open_on_select() {
            self.input_ui.set_composer(ComposerMode::Input);
        }
        let result = match action {
            PickerAction::SelectModel => {
                self.refresh_available_auths();
                match self.resolve_model_selection(
                    &value,
                    &self.info.runtime.provider,
                    &self.info.runtime.auth,
                ) {
                    Ok(selection) => {
                        if let Some((picker, selected_value)) = model_return_picker {
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
                    Err(err) => {
                        self.insert_entry(&Entry::Error(err.to_string()));
                        self.set_status("model switch failed");
                        Ok(())
                    }
                }
            }
            PickerAction::SelectInternalAgentModel => {
                let Some(target) = self.internal_agent_model_target.clone() else {
                    self.set_status("internal agent model selection expired");
                    return Ok(());
                };
                let id = target.id.as_str();
                let selected = match model_picker::parse_internal_agent_model_row(&value) {
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
                        match self.resolve_model_selection(
                            &reference,
                            &current.provider,
                            &current.auth,
                        ) {
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
            PickerAction::LoginGroup => {
                // A single-method group short-circuits to that method's value,
                // which may name the external runtime or new-host onboarding
                // rather than a group id.
                let value = match claude_login::SignInTarget::parse(&value) {
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
            PickerAction::LoginProvider => match claude_login::SignInTarget::parse(&value) {
                claude_login::SignInTarget::ClaudeCode => self.execute_claude_code_login().await,
                claude_login::SignInTarget::NewCustomHost { api } => {
                    self.start_custom_provider_onboarding(api);
                    Ok(())
                }
                claude_login::SignInTarget::Provider(provider) => {
                    self.start_login_for_provider(&provider, terminal, agent)
                        .await
                }
            },
            PickerAction::LogoutProvider => match claude_login::SignInTarget::parse(&value) {
                claude_login::SignInTarget::ClaudeCode => self.execute_claude_code_logout().await,
                // Nothing is stored for a host that was never created.
                claude_login::SignInTarget::NewCustomHost { .. } => Ok(()),
                claude_login::SignInTarget::Provider(provider) => {
                    self.logout_provider(&provider, agent).await
                }
            },
            PickerAction::SwitchAuthMode => self.switch_active_auth_mode(&value, agent).await,
            PickerAction::RefreshModelList => self.refresh_model_lists(&value, terminal).await,
            PickerAction::InsertSkillCommand => {
                self.input_ui.set_shell_mode(None);
                self.input_ui
                    .set_text_and_cursor(format!("/skill:{value}"), self.input_char_len());
                self.input_ui.set_command_palette_dismissed(true);
                self.set_status("skill command inserted");
                Ok(())
            }
            PickerAction::ResumeSession => {
                self.submit_resume_selection(&value, terminal, agent).await
            }
            PickerAction::ManageSessions => {
                self.submit_sessions_selection(&value, terminal, agent)
                    .await
            }
            PickerAction::SelectTreeNode => {
                self.submit_tree_selection(&value, terminal, agent).await
            }
            PickerAction::SelectRewindCheckpoint => self.submit_rewind_preview(&value, agent),
            PickerAction::ConfirmRewindCheckpoint => {
                self.submit_rewind_confirmation(&value, terminal, agent)
                    .await
            }
            PickerAction::Config => self.submit_config_selection(&value, agent, terminal).await,
            PickerAction::SelectTheme => self.submit_theme_selection(&value),
            PickerAction::ViewAgent => self.submit_view_agent_selection(&value),
            PickerAction::EditAgent => self.submit_edit_agent_selection(&value, terminal).await,
            PickerAction::Workflow => {
                self.submit_workflow_selection(&value, terminal, agent)
                    .await
            }
            PickerAction::AttachSubagent => {
                self.submit_attach_selection(&value);
                Ok(())
            }
            PickerAction::Dismiss | PickerAction::ViewMcpServers => Ok(()),
        };
        if let (true, Some((picker, selected_value))) = (result.is_ok(), other_return_picker) {
            // Restore the parent picker first, then re-apply action feedback so
            // open_main_config_picker does not clobber refresh/login status.
            let feedback = self.status().to_string();
            self.open_main_config_picker(selected_value, picker.filter)?;
            if !feedback.is_empty() {
                self.set_status(feedback);
            }
        }
        result
    }

    pub(in crate::tui) fn submit_picker_selection_during_turn(&mut self) -> anyhow::Result<()> {
        let Some((action, value)) = self.active_picker_selection() else {
            self.input_ui.set_composer(ComposerMode::Input);
            self.set_status("running");
            return Ok(());
        };

        let return_picker = self.take_picker_parent_after_selection(action);
        if !matches!(action, PickerAction::Config) {
            self.input_ui.set_composer(ComposerMode::Input);
        }
        match action.during_turn_select() {
            DuringTurnSelect::Unavailable(message) => self.set_status(message),
            DuringTurnSelect::CloseOnly => self.set_status("running"),
            DuringTurnSelect::Apply => match action {
                PickerAction::InsertSkillCommand => {
                    self.input_ui
                        .set_text_and_cursor(format!("/skill:{value}"), self.input_char_len());
                    self.input_ui.set_command_palette_dismissed(true);
                    self.set_status("skill command inserted");
                }
                PickerAction::AttachSubagent => self.submit_attach_selection(&value),
                PickerAction::Config => self.submit_config_selection_during_turn(&value)?,
                PickerAction::SelectModel => {
                    self.refresh_available_auths();
                    match self.resolve_model_selection(
                        &value,
                        &self.info.runtime.provider,
                        &self.info.runtime.auth,
                    ) {
                        Ok(selection) => self.queue_model_selection(selection)?,
                        Err(err) => {
                            self.insert_entry(&Entry::Error(err.to_string()));
                            self.set_status("model switch failed");
                        }
                    }
                }
                PickerAction::SelectTheme => self.submit_theme_selection(&value)?,
                _ => {}
            },
        }
        if let Some((picker, selected_value)) = return_picker {
            self.open_main_config_picker(selected_value, picker.filter)?;
        }
        Ok(())
    }

    pub(in crate::tui) fn handle_picker_escape(&mut self, running: bool) -> anyhow::Result<()> {
        let leaving = match self.input_ui.composer() {
            ComposerMode::Picker(picker) => Some(picker.action),
            _ => None,
        };
        if leaving == Some(PickerAction::EditAgent) {
            let phase = self
                .agent_editor_session
                .as_ref()
                .map(|session| session.phase());
            match phase {
                Some(agent_editor::AgentEditPhase::Fields) | None => {
                    self.cancel_agent_editor();
                    return Ok(());
                }
                Some(_) => {
                    if self.pop_picker_level() {
                        if let Some(session) = &mut self.agent_editor_session {
                            session.set_phase(agent_editor::AgentEditPhase::Fields);
                        }
                        return Ok(());
                    }
                    self.cancel_agent_editor();
                    return Ok(());
                }
            }
        }
        if leaving == Some(PickerAction::SelectTheme) {
            self.cancel_theme_preview();
        }
        let sessions_picker = leaving == Some(PickerAction::ManageSessions);
        let internal_agent_model_picker = leaving == Some(PickerAction::SelectInternalAgentModel);
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
            // Backing all the way out of a setup picker leaves setup too,
            // rather than stranding an empty full-screen shell.
            self.dismiss_setup_screen();
        }
        Ok(())
    }
}
