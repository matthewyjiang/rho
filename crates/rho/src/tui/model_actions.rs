use ratatui::DefaultTerminal;
use rho_providers::{
    credentials::available_auth_modes, model::provider_models::refresh_provider_models_with_store,
    providers::build_sdk_provider,
};

use super::{
    catalog, config_picker, favorites, model_picker, provider, provider_picker, reasoning_metadata,
    App, CommandInvocation, ComposerMode, Entry, InteractiveRuntime, ModelSelection, PickerAction,
    UiPicker,
};

impl App {
    async fn refresh_model_lists(
        &mut self,
        selected_provider: &str,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let providers = if selected_provider == provider_picker::ALL_REFRESHABLE_PROVIDERS {
            self.refresh_available_auths();
            provider::providers()
                .iter()
                .filter(|provider| provider.model_refresh.is_some())
                .filter(|provider| {
                    self.available_auths
                        .iter()
                        .any(|auth| auth == provider.auth)
                })
                .map(|provider| provider.name.to_string())
                .collect()
        } else {
            vec![selected_provider.to_string()]
        };

        if providers.is_empty() {
            self.insert_entry(&Entry::Notice(
                    "no refreshable providers are configured. open Config > Log in to provider to add one."
                        .into(),
                ),
            );
            self.status = "model refresh skipped".into();
            return Ok(());
        }

        self.status = "refreshing model list".into();
        terminal.draw(|frame| self.draw(frame))?;
        for provider in providers {
            match refresh_provider_models_with_store(&provider, self.credential_store.as_ref())
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
        self.status = "model list refresh complete".into();
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
        match catalog::resolve_model_selection_for_auths(
            model,
            &self.info.runtime.provider,
            &self.info.runtime.auth,
            &self.available_auths,
        ) {
            Ok(selection) => self.select_model(selection, agent),
            Err(err) => {
                self.insert_entry(&Entry::Error(err.to_string()));
                self.status = "model switch failed".into();
                Ok(())
            }
        }
    }

    async fn open_model_picker(
        &mut self,
        terminal: &mut DefaultTerminal,
        _agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        self.status = "loading models".into();
        terminal.draw(|frame| self.draw(frame))?;
        self.refresh_available_auths();
        let picker = model_picker::model_picker(&self.info.runtime, &self.available_auths);

        if picker.items.is_empty() {
            self.insert_entry(&Entry::Notice(
                "no cached API models. use Config > Refresh model lists after signing in.".into(),
            ));
            self.status = "ready".into();
            return Ok(());
        }

        self.composer = ComposerMode::Picker(picker);
        self.status = "select model".into();
        Ok(())
    }

    pub(super) async fn submit_picker_selection(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let Some((action, value)) = self.active_picker_selection() else {
            self.composer = ComposerMode::Input;
            self.status = "ready".into();
            return Ok(());
        };

        let return_picker = self.take_picker_parent_after_selection(action);
        if !matches!(action, PickerAction::Config | PickerAction::LoginGroup) {
            self.composer = ComposerMode::Input;
        }
        let result = match action {
            PickerAction::SelectModel => {
                self.refresh_available_auths();
                match catalog::resolve_model_selection_for_auths(
                    &value,
                    &self.info.runtime.provider,
                    &self.info.runtime.auth,
                    &self.available_auths,
                ) {
                    Ok(selection) => self.select_model(selection, agent),
                    Err(err) => {
                        self.insert_entry(&Entry::Error(err.to_string()));
                        self.status = "model switch failed".into();
                        Ok(())
                    }
                }
            }
            PickerAction::SelectTitleModel => {
                self.refresh_available_auths();
                let (provider, _model, auth) = self.title_model_selection();
                match catalog::resolve_model_selection_for_auths(
                    &value,
                    &provider,
                    &auth,
                    &self.available_auths,
                ) {
                    Ok(selection) => self.select_title_model(selection),
                    Err(err) => {
                        self.insert_entry(&Entry::Error(err.to_string()));
                        self.status = "title model switch failed".into();
                        Ok(())
                    }
                }
            }
            PickerAction::LoginGroup => {
                let Some(mut group) = catalog::login_group(&value) else {
                    self.insert_entry(&Entry::Error(format!(
                        "unsupported login provider group '{value}'"
                    )));
                    self.status = "login failed".into();
                    return Ok(());
                };
                if group.methods.len() == 1 {
                    let target = group.methods.remove(0).target;
                    self.start_login_for_provider(&target.provider, terminal, agent)
                        .await
                } else {
                    let child = provider_picker::login_method_picker(group);
                    self.open_child_picker(child);
                    Ok(())
                }
            }
            PickerAction::LoginProvider => {
                self.start_login_for_provider(&value, terminal, agent).await
            }
            PickerAction::LogoutProvider => self.logout_provider(&value, agent).await,
            PickerAction::RefreshModelList => self.refresh_model_lists(&value, terminal).await,
            PickerAction::InsertSkillCommand => {
                self.input = format!("/skill:{value}");
                self.input_cursor = self.input_char_len();
                self.command_palette_dismissed = true;
                self.status = "skill command inserted".into();
                Ok(())
            }
            PickerAction::ResumeSession => {
                self.submit_resume_selection(&value, terminal, agent).await
            }
            PickerAction::Config => self.submit_config_selection(&value, agent).await,
            PickerAction::Doctor | PickerAction::ViewAgent => Ok(()),
        };
        if let (true, Some((picker, selected_value))) = (result.is_ok(), return_picker) {
            self.open_main_config_picker(selected_value, picker.filter)?;
        }
        result
    }

    pub(super) fn handle_picker_escape(&mut self, running: bool) -> anyhow::Result<()> {
        if !self.pop_picker_level() {
            self.composer = ComposerMode::Input;
            self.status = if running { "running" } else { "ready" }.into();
        }
        Ok(())
    }

    pub(super) fn model_picker_is_open(&self) -> bool {
        matches!(
            &self.composer,
            ComposerMode::Picker(picker)
                if matches!(
                    picker.action,
                    PickerAction::SelectModel | PickerAction::SelectTitleModel
                )
        )
    }

    pub(super) fn toggle_selected_model_favorite(&mut self) -> anyhow::Result<()> {
        let Some((action, value)) = self.active_picker_selection() else {
            return Ok(());
        };
        if !matches!(
            action,
            PickerAction::SelectModel | PickerAction::SelectTitleModel
        ) {
            return Ok(());
        }
        let Some(favorite) = favorites::favorite_model_from_value(&value) else {
            return Ok(());
        };

        let filter = match &self.composer {
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
                self.status = "config save failed".into();
                return Ok(());
            }
        };
        self.info.runtime.favorite_models = favorite_models;

        self.refresh_available_auths();
        let mut picker = match action {
            PickerAction::SelectModel if self.running => model_picker::model_picker_during_run(
                &self.info.runtime,
                self.pending_model_selection.as_ref(),
                &self.available_auths,
            ),
            PickerAction::SelectModel => {
                model_picker::model_picker(&self.info.runtime, &self.available_auths)
            }
            PickerAction::SelectTitleModel => {
                let (provider, model, _auth) = self.title_model_selection();
                model_picker::title_model_picker(
                    &provider,
                    &model,
                    &self.info.runtime.favorite_models,
                    &self.available_auths,
                )
            }
            PickerAction::LoginGroup
            | PickerAction::LoginProvider
            | PickerAction::LogoutProvider
            | PickerAction::RefreshModelList
            | PickerAction::InsertSkillCommand
            | PickerAction::ViewAgent
            | PickerAction::ResumeSession
            | PickerAction::Config
            | PickerAction::Doctor => return Ok(()),
        };
        Self::restore_picker_position(&mut picker, &value, filter);
        self.composer = ComposerMode::Picker(picker);
        let action = if pinned { "pinned" } else { "unpinned" };
        self.insert_entry(&Entry::Notice(format!("{action} {value}")));
        self.status = format!("{action} model");
        Ok(())
    }

    pub(super) fn picker_space_confirms_selection(&self) -> bool {
        matches!(
            &self.composer,
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
            PickerAction::SelectTitleModel => config_picker::TITLE_MODEL_VALUE,
            PickerAction::LogoutProvider => config_picker::PROVIDER_LOGOUT_VALUE,
            PickerAction::RefreshModelList => config_picker::REFRESH_MODEL_LIST_VALUE,
            PickerAction::LoginGroup
            | PickerAction::LoginProvider
            | PickerAction::InsertSkillCommand
            | PickerAction::ViewAgent
            | PickerAction::ResumeSession
            | PickerAction::Config
            | PickerAction::Doctor => return None,
        };
        match &mut self.composer {
            ComposerMode::Picker(picker) => {
                picker.take_parent().map(|parent| (parent, selected_value))
            }
            _ => None,
        }
    }

    pub(super) fn active_picker_selection(&self) -> Option<(PickerAction, String)> {
        let ComposerMode::Picker(picker) = &self.composer else {
            return None;
        };
        picker
            .selected_item()
            .map(|item| (picker.action, item.value.clone()))
    }

    pub(super) fn select_model(
        &mut self,
        selection: ModelSelection,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let provider = selection.provider;
        let model = selection.model;
        let auth = selection.auth;
        let provider_model = format!("{provider}/{model}");
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
                self.status = "model switch rejected".into();
                return Ok(());
            }
        };
        let new_provider = match build_sdk_provider(&provider, &model, reasoning.effective) {
            Ok(provider) => provider,
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not switch to {provider_model}: {err}"
                )));
                self.status = "model switch failed".into();
                return Ok(());
            }
        };

        let handoff = agent.replace_provider(new_provider, reasoning.effective)?;
        if handoff.has_omissions() {
            let kinds = handoff.omitted_kinds.join(", ");
            self.insert_entry(&Entry::Notice(format!(
                "model handoff omitted {} nonportable provider context block(s): {kinds}; assistant text, tool history, and reasoning summaries were preserved",
                handoff.omitted_provider_context
            )));
        }
        self.info.runtime.provider = provider.clone();
        self.info.runtime.model = model.clone();
        self.info
            .set_reasoning(reasoning.effective, reasoning.source);
        self.info.runtime.auth = auth.clone();
        self.info.services.auth_unavailable = None;
        self.using_unavailable_provider = false;
        self.start_model_metadata_fetch(agent);
        match self.info.services.config_repository.update(|config| {
            config.provider = provider.clone();
            config.model = model.clone();
            config.reasoning = reasoning.effective;
            config.auth = auth.clone();
        }) {
            Ok(()) => {
                self.insert_entry(&Entry::Notice(format!(
                    "model switched to {provider_model} with reasoning {} and saved to config",
                    reasoning.effective
                )));
                self.status = format!("model: {provider_model}");
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "model switched to {provider_model} with reasoning {} for this session, but saving config failed: {err}",
                    reasoning.effective
                )));
                self.status = "config save failed".into();
            }
        }
        Ok(())
    }

    pub(super) fn select_title_model(&mut self, selection: ModelSelection) -> anyhow::Result<()> {
        let provider = selection.provider;
        let model = selection.model;
        let auth = selection.auth;
        let provider_model = format!("{provider}/{model}");
        self.info.runtime.title_provider = Some(provider.clone());
        self.info.runtime.title_model = Some(model.clone());
        self.info.runtime.title_auth = Some(auth.clone());
        match self.info.services.config_repository.update(|config| {
            config.title_provider = Some(provider.clone());
            config.title_model = Some(model.clone());
            config.title_auth = Some(auth.clone());
        }) {
            Ok(()) => {
                self.insert_entry(&Entry::Notice(format!(
                    "session title model switched to {provider_model} and saved to config"
                )));
                self.status = format!("title model: {provider_model}");
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                        "session title model switched to {provider_model} for this session, but saving config failed: {err}"
                    )),
                );
                self.status = "config save failed".into();
            }
        }
        Ok(())
    }

    pub(super) fn refresh_available_auths(&mut self) {
        self.available_auths = available_auth_modes(self.credential_store.as_ref());
    }
}
