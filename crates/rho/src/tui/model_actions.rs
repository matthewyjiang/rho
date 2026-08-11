use ratatui::DefaultTerminal;

use rho_providers::credentials::available_auth_modes;
use rho_providers::model::provider_models::{
    refresh_provider_models_with_store, ProviderModelEndpoint,
};

use crate::agent::{carry_internal_agent_reasoning, ADVISOR_AGENT_ID};

use super::{
    agent_picker::InternalAgentModelPickerOrigin, catalog, config_picker, favorites, model_picker,
    provider, provider_picker, reasoning_metadata, App, CommandInvocation, ComposerMode, Entry,
    InteractiveModelSelection, InteractiveRuntime, ModelSelection, PickerAction, UiPicker,
};

fn refresh_auth_for_provider(
    descriptor: &'static provider::ProviderDescriptor,
    preferred_auth: &str,
    available_auths: &[String],
) -> &'static str {
    descriptor
        .auth_mode(preferred_auth)
        .filter(|mode| available_auths.iter().any(|auth| auth == mode.id))
        .or_else(|| {
            descriptor
                .auth_modes()
                .find(|mode| available_auths.iter().any(|auth| auth == mode.id))
        })
        .unwrap_or_else(|| descriptor.default_auth())
        .id
}

impl App {
    pub(super) fn resolve_model_selection(
        &self,
        reference: &str,
        current_provider: &str,
        current_auth: &str,
    ) -> anyhow::Result<InteractiveModelSelection> {
        let resolved = self.info.runtime.model_aliases.resolve(reference)?;
        let alias = resolved.alias;
        let selection = match resolved.provider {
            Some(provider) => catalog::resolve_model_selection_for_provider(
                &provider,
                &resolved.model,
                catalog::SelectionAuthContext {
                    current: Some(current_auth),
                    available: &self.available_auths,
                },
            )?,
            None if alias.is_some() => catalog::resolve_model_selection_for_provider(
                current_provider,
                &resolved.model,
                catalog::SelectionAuthContext {
                    current: Some(current_auth),
                    available: &self.available_auths,
                },
            )?,
            None => catalog::resolve_model_selection_for_auths(
                &resolved.model,
                current_provider,
                current_auth,
                &self.available_auths,
            )?,
        };
        Ok(InteractiveModelSelection { selection, alias })
    }

    async fn refresh_model_lists(
        &mut self,
        selected_provider: &str,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let providers = if selected_provider == provider_picker::ALL_REFRESHABLE_PROVIDERS {
            self.refresh_available_auths();
            provider::providers()
                .iter()
                .filter(|descriptor| descriptor.model_refresh.is_some())
                .filter(|descriptor| {
                    descriptor
                        .auth_modes()
                        .any(|mode| self.available_auths.iter().any(|auth| auth == mode.id))
                })
                .map(|descriptor| {
                    (
                        descriptor.name.to_string(),
                        refresh_auth_for_provider(
                            descriptor,
                            &self.info.runtime.auth,
                            &self.available_auths,
                        )
                        .to_string(),
                    )
                })
                .collect()
        } else {
            let auth = provider::provider_descriptor(selected_provider)
                .map(|descriptor| {
                    refresh_auth_for_provider(
                        descriptor,
                        &self.info.runtime.auth,
                        &self.available_auths,
                    )
                    .to_string()
                })
                .unwrap_or_else(|| self.info.runtime.auth.clone());
            vec![(selected_provider.to_string(), auth)]
        };

        if providers.is_empty() {
            self.set_status(
                "no refreshable providers are configured. open Config > Log in to provider to add one.",
            );
            return Ok(());
        }

        self.set_status("refreshing model list");
        terminal.draw(|frame| self.draw(frame))?;
        let config = self.info.services.config_repository.load()?;
        for (provider, auth) in providers {
            let endpoint = config.resolved_provider_endpoint(&provider);
            let model_endpoint = endpoint.as_ref().map_or(
                ProviderModelEndpoint::ProviderOwned,
                ProviderModelEndpoint::OpenAiCompatible,
            );
            match refresh_provider_models_with_store(
                &provider,
                &auth,
                self.credential_store.as_ref(),
                model_endpoint,
            )
            .await
            {
                Ok(refresh) => {
                    self.insert_entry(&Entry::Notice(format!(
                        "refreshed {} model list: {} models",
                        refresh.provider,
                        refresh.models.len()
                    )));
                }
                Err(err) => {
                    self.insert_entry(&Entry::Error(format!(
                        "failed to refresh {provider} model list: {err}"
                    )));
                }
            }
        }
        self.set_status("model list refresh complete");
        Ok(())
    }

    pub(super) async fn execute_model_command(
        &mut self,
        invocation: CommandInvocation,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let model = invocation.args.trim();
        if model.is_empty() {
            self.open_model_picker(terminal, agent).await?;
            return Ok(());
        }

        self.refresh_available_auths();
        match self.resolve_model_selection(
            model,
            &self.info.runtime.provider,
            &self.info.runtime.auth,
        ) {
            Ok(selection) => self.request_model_selection(selection, agent).await,
            Err(err) => {
                self.insert_entry(&Entry::Error(err.to_string()));
                self.set_status("model switch failed");
                Ok(())
            }
        }
    }

    async fn open_model_picker(
        &mut self,
        terminal: &mut DefaultTerminal,
        _agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        self.set_status("loading models");
        terminal.draw(|frame| self.draw(frame))?;
        self.refresh_available_auths();
        let picker = model_picker::model_picker(&self.info.runtime, &self.available_auths);

        if picker.items.is_empty() {
            self.set_status("no cached provider models. use Config > Refresh model lists.");
            return Ok(());
        }

        self.input_ui.set_composer(ComposerMode::Picker(picker));
        self.set_status("select model");
        Ok(())
    }

    pub(super) async fn submit_picker_selection(
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
        if !matches!(
            action,
            PickerAction::Config
                | PickerAction::LoginGroup
                | PickerAction::ViewAgent
                | PickerAction::EditAgent
                | PickerAction::SelectRewindCheckpoint
                | PickerAction::ConfirmRewindCheckpoint
                | PickerAction::Workflow
                | PickerAction::ManageSessions
        ) {
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
                // which may name the external runtime rather than a group id.
                if let super::claude_login::SignInTarget::ClaudeCode =
                    super::claude_login::SignInTarget::parse(&value)
                {
                    return self.execute_claude_code_login(terminal).await;
                }
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
            PickerAction::LoginProvider => match super::claude_login::SignInTarget::parse(&value) {
                super::claude_login::SignInTarget::ClaudeCode => {
                    self.execute_claude_code_login(terminal).await
                }
                super::claude_login::SignInTarget::Provider(provider) => {
                    self.start_login_for_provider(&provider, terminal, agent)
                        .await
                }
            },
            PickerAction::LogoutProvider => {
                match super::claude_login::SignInTarget::parse(&value) {
                    super::claude_login::SignInTarget::ClaudeCode => {
                        self.execute_claude_code_logout().await
                    }
                    super::claude_login::SignInTarget::Provider(provider) => {
                        self.logout_provider(&provider, agent).await
                    }
                }
            }
            PickerAction::SwitchAuthMode => self.switch_active_auth_mode(&value, agent),
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
            PickerAction::Config => self.submit_config_selection(&value, agent).await,
            PickerAction::SelectTheme => self.submit_theme_selection(&value),
            PickerAction::ViewAgent => self.submit_view_agent_selection(&value),
            PickerAction::EditAgent => self.submit_edit_agent_selection(&value, terminal).await,
            PickerAction::Workflow => {
                self.submit_workflow_selection(&value, terminal, agent)
                    .await
            }
            PickerAction::Dismiss => Ok(()),
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

    pub(super) fn handle_picker_escape(&mut self, running: bool) -> anyhow::Result<()> {
        let leaving_action = match self.input_ui.composer() {
            ComposerMode::Picker(picker) => Some(picker.action),
            _ => None,
        };
        if matches!(leaving_action, Some(PickerAction::EditAgent)) {
            let phase = self
                .agent_editor_session
                .as_ref()
                .map(|session| session.phase());
            match phase {
                Some(super::agent_editor::AgentEditPhase::Fields) | None => {
                    self.cancel_agent_editor();
                    return Ok(());
                }
                Some(_) => {
                    if self.pop_picker_level() {
                        if let Some(session) = &mut self.agent_editor_session {
                            session.set_phase(super::agent_editor::AgentEditPhase::Fields);
                        }
                        return Ok(());
                    }
                    self.cancel_agent_editor();
                    return Ok(());
                }
            }
        }
        if let Some(action) = leaving_action {
            self.cancel_theme_preview_if_leaving(action);
        }
        let sessions_picker = matches!(leaving_action, Some(PickerAction::ManageSessions));
        if self.pop_picker_level() {
            if sessions_picker {
                self.sessions_hub_state.navigate_back();
            }
        } else {
            if sessions_picker {
                self.sessions_hub_state.clear();
            }
            self.input_ui.set_composer(ComposerMode::Input);
            if !self.cancel_advisor_model_prompt() {
                self.set_status(if running { "running" } else { "ready" });
            }
            // Backing all the way out of a setup picker leaves setup too,
            // rather than stranding an empty full-screen shell.
            self.dismiss_setup_screen();
        }
        Ok(())
    }

    pub(super) fn toggle_selected_model_favorite(&mut self) -> anyhow::Result<()> {
        let Some((action, value)) = self.active_picker_selection() else {
            return Ok(());
        };
        if !matches!(
            action,
            PickerAction::SelectModel | PickerAction::SelectInternalAgentModel
        ) {
            return Ok(());
        }
        let Some(favorite) = favorites::favorite_model_from_value(&value) else {
            return Ok(());
        };

        let filter = match self.input_ui.composer() {
            ComposerMode::Picker(picker) => picker.filter.clone(),
            _ => String::new(),
        };
        let save_result = self.info.services.config_repository.update(|config| {
            let pinned = favorites::toggle_favorite(
                &mut config.favorite_models,
                &favorite.provider,
                &favorite.model,
            );
            (pinned, config.favorite_models.clone())
        });
        let (pinned, favorite_models) = match save_result {
            Ok(saved) => saved,
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not save pinned models: {err}"
                )));
                self.set_status("config save failed");
                return Ok(());
            }
        };
        self.info.runtime.favorite_models = favorite_models;

        self.refresh_available_auths();
        let mut picker = match action {
            // During-run picker queues a model for after the provider turn.
            // Compaction is busy UI without a live run, so keep the idle picker.
            PickerAction::SelectModel if self.is_provider_turn_ui() => {
                model_picker::model_picker_during_run(
                    &self.info.runtime,
                    self.pending_model_selection
                        .as_ref()
                        .map(|pending| &pending.selection),
                    &self.available_auths,
                )
            }
            PickerAction::SelectModel => {
                model_picker::model_picker(&self.info.runtime, &self.available_auths)
            }
            PickerAction::SelectInternalAgentModel => {
                let Some(target) = self.internal_agent_model_target.clone() else {
                    return Ok(());
                };
                self.internal_agent_model_picker(&target.id, target.origin)
            }
            PickerAction::LoginGroup
            | PickerAction::LoginProvider
            | PickerAction::LogoutProvider
            | PickerAction::SwitchAuthMode
            | PickerAction::RefreshModelList
            | PickerAction::InsertSkillCommand
            | PickerAction::ViewAgent
            | PickerAction::ResumeSession
            | PickerAction::ManageSessions
            | PickerAction::SelectTreeNode
            | PickerAction::SelectRewindCheckpoint
            | PickerAction::ConfirmRewindCheckpoint
            | PickerAction::Config
            | PickerAction::SelectTheme
            | PickerAction::EditAgent
            | PickerAction::Workflow
            | PickerAction::Dismiss => return Ok(()),
        };
        Self::restore_picker_position(&mut picker, &value, filter);
        self.input_ui.set_composer(ComposerMode::Picker(picker));
        let action = if pinned { "pinned" } else { "unpinned" };
        self.set_status(format!("{action} {value}"));
        Ok(())
    }

    pub(super) fn picker_space_confirms_selection(&self) -> bool {
        matches!(
            self.input_ui.composer(),
            ComposerMode::Picker(picker) if picker.action.space_confirms_selection()
        )
    }

    pub(super) fn restore_picker_position(
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

    pub(super) fn take_picker_parent_after_selection(
        &mut self,
        action: PickerAction,
    ) -> Option<(UiPicker, &'static str)> {
        let selected_value = match action {
            PickerAction::SelectModel => config_picker::CONVERSATION_MODEL_VALUE,
            PickerAction::SelectTheme => config_picker::THEME_VALUE,
            PickerAction::SelectInternalAgentModel => return None,
            PickerAction::LogoutProvider => config_picker::PROVIDER_LOGOUT_VALUE,
            PickerAction::SwitchAuthMode => config_picker::SWITCH_AUTH_MODE_VALUE,
            PickerAction::RefreshModelList => config_picker::REFRESH_MODEL_LIST_VALUE,
            PickerAction::LoginGroup
            | PickerAction::LoginProvider
            | PickerAction::InsertSkillCommand
            | PickerAction::ViewAgent
            | PickerAction::ResumeSession
            | PickerAction::ManageSessions
            | PickerAction::SelectTreeNode
            | PickerAction::SelectRewindCheckpoint
            | PickerAction::ConfirmRewindCheckpoint
            | PickerAction::Config
            | PickerAction::EditAgent
            | PickerAction::Workflow
            | PickerAction::Dismiss => return None,
        };
        match self.input_ui.composer_mut() {
            ComposerMode::Picker(picker) => {
                picker.take_parent().map(|parent| (parent, selected_value))
            }
            _ => None,
        }
    }

    pub(super) fn active_picker_selection(&self) -> Option<(PickerAction, String)> {
        let ComposerMode::Picker(picker) = self.input_ui.composer() else {
            return None;
        };
        picker
            .selected_item()
            .map(|item| (picker.action, item.value.clone()))
    }

    pub(super) async fn select_model(
        &mut self,
        resolved: InteractiveModelSelection,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let _ = self.select_model_report(resolved, agent).await?;
        Ok(())
    }

    pub(super) async fn select_model_report(
        &mut self,
        resolved: InteractiveModelSelection,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<Option<rho_sdk::model::handoff::HandoffReport>> {
        let InteractiveModelSelection { selection, alias } = resolved;
        let provider = selection.provider;
        let model = selection.model;
        let auth = selection.auth;
        let provider_model = rho_providers::provider::model_reference(&provider, &model);
        let capabilities =
            rho_providers::model::models_dev::current_reasoning_capabilities(&provider, &model);
        let reasoning = match reasoning_metadata::resolve_model_switch_reasoning(
            &capabilities,
            self.info.runtime.reasoning,
            self.info.runtime.reasoning_source,
        ) {
            Ok(reasoning) => reasoning,
            Err(requested) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not switch to {provider_model}: reasoning level '{requested}' is not supported"
                )));
                self.set_status("model switch rejected");
                return Ok(None);
            }
        };
        let new_provider = match self.build_provider_for_selection(
            &provider,
            &model,
            reasoning.effective,
            &auth,
        ) {
            Ok(provider) => provider,
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not switch to {provider_model}: {err}"
                )));
                self.set_status("model switch failed");
                return Ok(None);
            }
        };

        let previous_identity = crate::model_identity::ModelIdentity::Rho {
            provider: self.info.runtime.provider.clone(),
            model: self.info.runtime.model.clone(),
        };
        // A first selection on an empty session is not a switch: the system
        // prompt has yet to be built and will name the chosen model itself.
        let session_started = !agent.history().is_empty();
        let handoff = agent.replace_provider(new_provider, reasoning.effective, &auth)?;
        self.info.runtime.provider = provider.clone();
        self.info.runtime.model = model.clone();
        // The system prompt named the model this session started on and then
        // stayed fixed, so a later switch has to reach the model as context.
        let current_identity = crate::model_identity::ModelIdentity::Rho {
            provider: provider.clone(),
            model: model.clone(),
        };
        if session_started && current_identity != previous_identity {
            let (context, display) = crate::prompt::model_switch_context(&current_identity);
            if let Err(error) = agent.append_user_context_with_display(context, display) {
                self.insert_entry(&Entry::Error(format!(
                    "switched to {provider_model}, but could not record the switch for the model: {error}"
                )));
            }
        }
        self.info
            .set_reasoning(reasoning.effective, reasoning.source);
        self.info.runtime.auth = auth.clone();
        self.info.services.auth_unavailable = None;
        self.using_unavailable_provider = false;
        self.start_model_metadata_fetch(agent);
        match self.info.services.config_repository.update(|config| {
            config.provider = provider.clone();
            config.model = model.clone();
            config.model_alias = alias.clone();
            config.reasoning = reasoning.effective;
            config.auth = auth.clone();
        }) {
            Ok(()) => {
                self.set_status(format!(
                    "model switched to {provider_model} with reasoning {} and saved to config",
                    reasoning.effective
                ));
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "model switched to {provider_model} with reasoning {} for this session, but saving config failed: {err}",
                    reasoning.effective
                )));
                self.set_status("config save failed");
            }
        }
        // Auto edit preference follows the new provider while the session is
        // idle (model switches never land mid-run).
        self.apply_auto_edit_tool_for_provider(&provider, agent)
            .await?;
        self.finish_setup_screen();
        Ok(Some(handoff))
    }

    pub(super) fn select_internal_agent_model(
        &mut self,
        id: &str,
        selection: Option<ModelSelection>,
    ) -> anyhow::Result<()> {
        let config = selection.map(|selection| {
            crate::config::InternalAgentModelConfig::new(
                selection.provider,
                selection.model,
                selection.auth,
            )
        });
        self.store_internal_agent_model(id, config)
    }

    /// Points an internal agent at the Claude Code CLI. `model` is passed
    /// through as `--model`; `None` lets Claude Code choose.
    pub(super) fn select_internal_agent_claude_model(
        &mut self,
        id: &str,
        model: Option<String>,
    ) -> anyhow::Result<()> {
        self.store_internal_agent_model(
            id,
            Some(crate::config::InternalAgentModelConfig::claude_cli(model)),
        )
    }

    /// Saves an internal agent's selection, or clears it with `None` so the
    /// agent falls back to the conversation model.
    fn store_internal_agent_model(
        &mut self,
        id: &str,
        config: Option<crate::config::InternalAgentModelConfig>,
    ) -> anyhow::Result<()> {
        let label = config
            .as_ref()
            .map(crate::config::InternalAgentModelConfig::display_reference)
            .unwrap_or_else(|| "conversation model".into());
        let previous = self.info.runtime.internal_agents.get(id).cloned();
        let save_result = match config {
            Some(mut config) => {
                config.reasoning = carry_internal_agent_reasoning(&config, previous.as_ref());
                self.info
                    .runtime
                    .internal_agents
                    .insert(id.to_string(), config.clone());
                self.info.services.config_repository.update(|saved| {
                    saved.set_internal_agent_model_config(id, config);
                })
            }
            None => {
                self.info.runtime.internal_agents.remove(id);
                self.info
                    .services
                    .config_repository
                    .update(|config| config.clear_internal_agent_model(id))
            }
        };
        match save_result {
            Ok(()) => {
                self.set_status(format!(
                    "internal agent {id} now uses {label}; saved to config"
                ));
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "internal agent {id} now uses {label} for this session, but saving config failed: {err}"
                )));
                self.set_status("config save failed");
            }
        }
        if id == ADVISOR_AGENT_ID {
            self.statusline.update_model(&self.info.runtime);
        }
        Ok(())
    }

    async fn finish_internal_agent_model_flow(
        &mut self,
        target: super::agent_picker::InternalAgentModelTarget,
        selected: bool,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let id = target.id.as_str();
        match target.origin {
            InternalAgentModelPickerOrigin::AgentsPicker => {
                if id == ADVISOR_AGENT_ID {
                    self.sync_advisor_runtime(agent).await;
                }
                let status = self.status().to_string();
                self.execute_agents_command()?;
                self.set_status(status);
            }
            InternalAgentModelPickerOrigin::AdvisorCommand => {
                self.internal_agent_model_target = None;
                self.finish_advisor_model_selection(selected, agent).await?;
            }
            InternalAgentModelPickerOrigin::AdvisorConfigRow => {
                self.internal_agent_model_target = None;
                self.finish_advisor_model_selection(selected, agent).await?;
                let status = self.status().to_string();
                self.open_main_config_picker_selected(config_picker::ADVISOR_MODE_VALUE)?;
                self.set_status(status);
            }
            InternalAgentModelPickerOrigin::AdvisorModelConfigRow => {
                self.internal_agent_model_target = None;
                if selected && self.info.runtime.advisor_mode {
                    self.sync_advisor_runtime(agent).await;
                }
                let status = self.status().to_string();
                self.open_main_config_picker_selected(config_picker::ADVISOR_MODEL_VALUE)?;
                self.set_status(status);
            }
        }
        Ok(())
    }

    pub(super) fn refresh_available_auths(&mut self) {
        self.available_auths = available_auth_modes(self.credential_store.as_ref());
    }

    pub(super) fn internal_agent_model_selection(
        &self,
        id: &str,
    ) -> crate::config::InternalAgentModelConfig {
        self.info
            .runtime
            .internal_agents
            .get(id)
            .cloned()
            .unwrap_or_else(|| self.conversation_internal_agent_model())
    }

    /// Provider and auth context for resolving a partially-qualified Rho model
    /// reference against an agent's current selection.
    ///
    /// A delegating selection carries no Rho provider or auth, so the
    /// conversation's stack is the answer here: this is the path that switches
    /// an agent off Claude Code and back onto a Rho model.
    pub(super) fn internal_agent_rho_model_or_conversation(
        &self,
        id: &str,
    ) -> crate::config::RhoInternalAgentModel {
        self.info
            .runtime
            .internal_agents
            .get(id)
            .and_then(crate::config::InternalAgentModelConfig::rho)
            .cloned()
            .unwrap_or_else(|| {
                self.conversation_internal_agent_model()
                    .rho()
                    .cloned()
                    .expect("conversation selection is always a rho selection")
            })
    }

    fn conversation_internal_agent_model(&self) -> crate::config::InternalAgentModelConfig {
        crate::config::InternalAgentModelConfig::new(
            self.info.runtime.provider.clone(),
            self.info.runtime.model.clone(),
            self.info.runtime.auth.clone(),
        )
    }
}

/// The Rho half of a selection that is about to run on Rho's provider stack.
///
/// Only agents declaring `accepts_claude_runtime: false` reach here, and neither
/// the config loader nor the picker will build a delegating selection for one.
/// Failing loudly beats running a model this path never asked for.
pub(super) fn expect_rho_internal_agent_model(
    id: &str,
    selection: crate::config::InternalAgentModelConfig,
) -> crate::config::RhoInternalAgentModel {
    match selection.target {
        crate::config::InternalAgentTarget::Rho(model) => model,
        crate::config::InternalAgentTarget::ClaudeCli { .. } => {
            panic!("internal agent '{id}' declares accepts_claude_runtime: false")
        }
    }
}

#[cfg(test)]
#[path = "model_actions_tests.rs"]
mod tests;
