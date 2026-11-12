use crate::tui::DefaultTerminal;

use rho_providers::credentials::available_auth_modes;
use rho_providers::model::decision_models::lists_decision_models;
use rho_providers::model::provider_models::{
    refresh_provider_models_with_store, ProviderModelEndpoint,
};

use crate::agent::{carry_internal_agent_reasoning, ADVISOR_AGENT_ID};

use super::{
    agent_picker::InternalAgentModelPickerOrigin, catalog, favorites, provider, provider_picker,
    App, ComposerMode, Entry, InteractiveModelSelection, ModelSelection,
};

pub(super) fn refresh_auth_for_provider(
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
    /// Transcript notices wait for an ordered stream boundary during a turn.
    /// Refresh is blocked while the session is busy, so the
    /// wait clause follows `self.turn.is_busy()` rather than the caller.
    /// Repeating the same empty picker from an open `/config` menu re-toasts
    /// without stacking identical transcript rows.
    pub(super) fn report_missing_cached_provider_models(&mut self) {
        let notice = if self.turn.is_busy() {
            "no cached provider models. Run /refresh-models after the current turn ends."
        } else {
            "no cached provider models. Run /refresh-models."
        };
        let last_notice =
            self.streams
                .pending_notices
                .last()
                .or_else(|| match self.history.last() {
                    Some(Entry::Notice(text)) => Some(text),
                    _ => None,
                });
        if last_notice.is_none_or(|text| text != notice) {
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
        let config = self.info.services.config_repository.load()?;
        let providers = if selected_provider == provider_picker::ALL_REFRESHABLE_PROVIDERS {
            self.refresh_available_auths();
            provider_picker::refreshable_providers(&self.available_auths, &config.providers)
                .into_iter()
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
        for (provider, auth) in providers {
            if lists_decision_models(&provider) {
                self.refresh_decision_models(&provider, &auth).await?;
            }
            if !provider::provider_descriptor(&provider)
                .is_some_and(|descriptor| descriptor.supports_model_refresh())
            {
                continue;
            }
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
                Ok(refresh) => self.apply_model_list_refresh(refresh),
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

    /// Provider discovery can change capabilities without changing model identity.
    pub(super) fn apply_model_list_refresh(
        &mut self,
        refresh: rho_providers::model::provider_models::ProviderModelRefresh,
    ) {
        self.refresh_catalog_chrome();
        self.insert_entry(&Entry::Notice(format!(
            "refreshed {} model list: {} models",
            refresh.provider,
            refresh.models.len()
        )));
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
        // Only chat-model rows pin; the permission screen's picker also lists
        // decision models and a classifier row.
        if !matches!(
            super::model_picker::parse_internal_agent_model_row(&value),
            super::model_picker::InternalAgentModelRow::RhoModel(_)
        ) {
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
    /// through as the Claude session `model`; `None` lets Claude Code choose.
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

    /// Saves an agent's `[internal_agents]` selection, or clears it with
    /// `None` so the agent falls back: an internal agent to the conversation
    /// model, a catalog (plugin) agent to the model its file names.
    fn store_internal_agent_model(
        &mut self,
        id: &str,
        config: Option<crate::config::InternalAgentModelConfig>,
    ) -> anyhow::Result<()> {
        let (subject, fallback) = if crate::agent::is_internal_agent_id(id) {
            (format!("internal agent {id}"), "conversation model")
        } else {
            (format!("agent {id}"), "its default model")
        };
        let label = config
            .as_ref()
            .map(|config| self.model_names().internal_agent(config))
            .unwrap_or_else(|| fallback.into());
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
                // The session reads the saved config itself: the advisor's
                // model before its next turn, any other agent's only when a
                // session opens.
                let applies = if id == ADVISOR_AGENT_ID {
                    "this session uses it from the next turn"
                } else {
                    "sessions opened from now on use it"
                };
                self.set_status(format!(
                    "{subject} now uses {label}; saved to config; {applies}"
                ));
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "{subject} now uses {label} for this session, but saving config failed: {err}"
                )));
                self.set_status("config save failed");
            }
        }
        if id == ADVISOR_AGENT_ID {
            self.statusline.update_model(&self.info.runtime);
        }
        Ok(())
    }

    /// Returns to where an internal-agent model picker was opened: the agents
    /// picker reopens, and the startup classifier picker settles Auto.
    pub(in crate::tui) fn finish_internal_agent_model_flow(
        &mut self,
        target: super::agent_picker::AgentModelTarget,
        selected: bool,
    ) -> anyhow::Result<()> {
        match target.origin {
            InternalAgentModelPickerOrigin::AgentsPicker => {
                let status = self.status().to_string();
                self.execute_agents_command()?;
                self.set_status(status);
            }
            InternalAgentModelPickerOrigin::PermissionModeStartup => {
                self.internal_agent_model_target = None;
                self.finish_permission_classifier_startup_selection(selected)?;
            }
        }
        Ok(())
    }

    pub(super) fn refresh_available_auths(&mut self) {
        self.available_auths = available_auth_modes(self.credential_store.as_ref());
        // A session not opened yet lists the models they offer, as an
        // opened one's options follow a credential write.
        self.info
            .session
            .daemon
            .set_draft_auths(&self.available_auths);
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

#[cfg(test)]
#[path = "model_actions_tests.rs"]
mod tests;
