use ratatui::DefaultTerminal;

use rho_providers::credentials::available_auth_modes;
use rho_providers::model::provider_models::{
    refresh_provider_models_with_store, ProviderModelEndpoint,
};

use crate::agent::{
    carry_internal_agent_reasoning, ADVISOR_AGENT_ID, PERMISSION_CLASSIFIER_AGENT_ID,
};

use super::{
    agent_picker::InternalAgentModelPickerOrigin, catalog, config_picker, favorites, provider,
    provider_picker, reasoning_metadata, App, CommandInvocation, ComposerMode, Entry,
    InteractiveModelSelection, InteractiveRuntime, ModelSelection,
};

fn refresh_auth_for_provider(
    descriptor: &'static provider::ProviderDescriptor,
    preferred_auth: &str,
    available_auths: &[String],
) -> &'static str {
    let modes: Vec<String> = descriptor
        .auth_modes()
        .map(|mode| mode.id.to_string())
        .collect();
    let selected = catalog::SelectionAuthContext {
        current: Some(preferred_auth),
        available: available_auths,
    }
    .select(&modes);
    descriptor
        .auth_mode(&selected)
        .unwrap_or_else(|| descriptor.default_auth())
        .id
}

impl App {
    /// `/model` and config model pickers with an empty cache: keep the refresh
    /// path in the transcript after the 2-second toast is gone.
    ///
    /// Flush any live stream first so the notice cannot split an in-flight
    /// assistant row. Refresh is blocked while the session is busy, so the
    /// wait clause follows `self.turn.is_busy()` rather than the caller.
    /// Repeating the same empty picker from an open `/config` menu re-toasts
    /// without stacking identical transcript rows.
    pub(super) fn report_missing_cached_provider_models(&mut self) {
        self.finish_streams();
        let notice = if self.turn.is_busy() {
            "no cached provider models. Open /config > Providers > Refresh model lists after the current turn ends."
        } else {
            "no cached provider models. Open /config > Providers > Refresh model lists."
        };
        if !matches!(self.history.last(), Some(Entry::Notice(text)) if text == notice) {
            self.insert_entry(&Entry::Notice(notice.into()));
        }
        self.set_status(notice);
    }

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

    pub(in crate::tui) async fn refresh_model_lists(
        &mut self,
        selected_provider: &str,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let providers = if selected_provider == provider_picker::ALL_REFRESHABLE_PROVIDERS {
            self.refresh_available_auths();
            provider::providers()
                .iter()
                .filter(|descriptor| descriptor.supports_model_refresh())
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
                        "could not refresh {provider} model list: {err}"
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
        let picker = self.conversation_model_picker();

        if picker.items.is_empty() {
            self.report_missing_cached_provider_models();
            return Ok(());
        }

        self.input_ui.set_composer(ComposerMode::Picker(picker));
        self.set_status("select model");
        Ok(())
    }

    pub(super) fn toggle_selected_model_favorite(&mut self) -> anyhow::Result<()> {
        let (value, filter) = match self.input_ui.composer() {
            ComposerMode::Picker(picker) if picker.is_model_list() => (
                picker
                    .selected_item()
                    .map(|item| item.value.clone())
                    .unwrap_or_default(),
                picker.filter.clone(),
            ),
            _ => return Ok(()),
        };
        if value.is_empty() {
            return Ok(());
        }
        let Some(favorite) = favorites::favorite_model_from_value(&value) else {
            return Ok(());
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
        // The pinned view degrades to all on its own when the last usable pin
        // goes away, so no scope reconciliation is needed here.
        self.rebuild_open_model_picker(&value, filter);
        let action = if pinned { "pinned" } else { "unpinned" };
        self.set_status(format!("{action} {value}"));
        Ok(())
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
        let new_provider = match self
            .build_provider_for_selection(&provider, &model, reasoning.effective, &auth)
            .await
        {
            Ok(provider) => provider,
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not switch to {provider_model}: {err}"
                )));
                self.set_status("model switch failed");
                return Ok(None);
            }
        };

        let handoff = agent.replace_provider(new_provider, reasoning.effective, &auth)?;
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
            .await;
        self.finish_setup_screen();
        self.reconcile_auto_classifier_gate(agent).await?;
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

    pub(in crate::tui) async fn finish_internal_agent_model_flow(
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
                } else if id == PERMISSION_CLASSIFIER_AGENT_ID {
                    self.sync_permission_classifier_runtime_config(agent);
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
            InternalAgentModelPickerOrigin::PermissionModeConfigRow => {
                let origin = target.origin;
                self.internal_agent_model_target = None;
                self.finish_permission_classifier_model_selection(selected, origin, agent)
                    .await?;
                let status = self.status().to_string();
                self.open_main_config_picker_selected(config_picker::PERMISSION_MODE_VALUE)?;
                self.set_status(status);
            }
            InternalAgentModelPickerOrigin::PermissionModeStartup => {
                let origin = target.origin;
                self.internal_agent_model_target = None;
                self.finish_permission_classifier_model_selection(selected, origin, agent)
                    .await?;
            }
            InternalAgentModelPickerOrigin::PermissionClassifierModelConfigRow => {
                self.internal_agent_model_target = None;
                if selected
                    && self.info.runtime.permission_mode == crate::permission::PermissionMode::Auto
                {
                    self.sync_permission_classifier_runtime_config(agent);
                }
                let status = self.status().to_string();
                self.open_main_config_picker_selected(
                    config_picker::PERMISSION_CLASSIFIER_MODEL_VALUE,
                )?;
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
