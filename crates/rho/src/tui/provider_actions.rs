use super::*;
use rho_providers::model::decision_models::{
    lists_decision_models, refresh_decision_models_with_store,
};
use rho_providers::model::provider_models::ProviderModelEndpoint;

pub(super) struct ProviderActivation {
    pub(super) provider: String,
    pub(super) model: String,
    pub(super) reasoning: reasoning_metadata::ModelSwitchReasoningResolution,
    pub(super) auth: String,
    pub(super) replacement: std::sync::Arc<dyn rho_sdk::provider::ModelProvider>,
}

pub(super) enum ProviderActivationOutcome {
    Saved,
    ConfigSaveFailed(anyhow::Error),
}

impl App {
    /// Builds a provider selection on top of persisted application config so
    /// transport settings (notably custom endpoints) survive live rebuilds.
    ///
    /// Awaits models.dev hydrate when the selected provider maps adapters from
    /// catalog npm metadata, so a mid-session switch does not start on the
    /// Chat Completions fallback.
    pub(super) async fn build_provider_for_selection(
        &self,
        provider: &str,
        model: &str,
        reasoning: rho_providers::reasoning::ReasoningLevel,
        auth: &str,
    ) -> anyhow::Result<std::sync::Arc<dyn rho_sdk::provider::ModelProvider>> {
        let mut config = self.info.services.config_repository.load()?;
        config.provider = provider.into();
        config.model = model.into();
        config.reasoning = reasoning;
        config.auth = auth.into();
        Ok(
            crate::credential_store::build_provider_from_config_ensuring_catalog(
                &config,
                std::sync::Arc::clone(&self.credential_store),
            )
            .await?,
        )
    }

    pub(super) async fn activate_provider(
        &mut self,
        activation: ProviderActivation,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<ProviderActivationOutcome> {
        let provider = activation.provider.clone();
        agent.replace_provider(
            activation.replacement,
            activation.reasoning.effective,
            &activation.auth,
        )?;
        self.cache_warm_provider_replaced();
        self.info.runtime.provider = activation.provider;
        self.info.runtime.prompt_cache_ttl = agent.prompt_cache_ttl();
        self.info.runtime.model = activation.model;
        self.info
            .set_reasoning(activation.reasoning.effective, activation.reasoning.source);
        self.info.runtime.auth = activation.auth;
        self.info.services.auth_unavailable = None;
        self.using_unavailable_provider = false;
        self.start_model_metadata_fetch(agent);
        let outcome = match self.save_current_config() {
            Ok(()) => ProviderActivationOutcome::Saved,
            Err(error) => ProviderActivationOutcome::ConfigSaveFailed(error),
        };
        self.apply_auto_edit_tool_for_provider(&provider, agent)
            .await;
        Ok(outcome)
    }

    /// Writes the login target's auth profile so a stored custom key is not
    /// left behind as `auth = "none"` after restart.
    pub(super) fn persist_login_auth(&mut self, target: &LoginTarget) {
        if target.auth == rho_providers::provider::KEYLESS_AUTH {
            return;
        }
        let result = if target.provider == self.info.runtime.provider {
            self.info.runtime.auth = target.auth.clone();
            self.save_current_config()
        } else {
            self.info.services.config_repository.update(|config| {
                if config.provider == target.provider {
                    config.auth = target.auth.clone();
                }
            })
        };
        if let Err(err) = result {
            self.insert_entry(&Entry::Error(format!(
                "stored credentials, but saving auth mode failed: {err}"
            )));
        }
    }

    /// Removes the login of a host logged in through its endpoint (Ollama)
    /// and drops its cached models. Returns whether a login was recorded.
    pub(super) fn forget_endpoint_login(&mut self, provider: &str) -> anyhow::Result<bool> {
        let Some(descriptor) = provider::provider_descriptor(provider) else {
            return Ok(false);
        };
        let forgot =
            crate::app::endpoint_login::forget(&self.info.services.config_repository, *descriptor)?;
        if forgot {
            self.refresh_catalog_chrome();
        }
        Ok(forgot)
    }

    /// Refreshes a provider's model list right after login, when it has a
    /// refreshable list, and reports the count or the failure.
    pub(super) async fn refresh_model_list_after_login(
        &mut self,
        target: &LoginTarget,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let Some(descriptor) = provider::provider_descriptor(&target.provider) else {
            return Ok(());
        };
        if lists_decision_models(&target.provider) {
            self.refresh_decision_models(&target.provider, &target.auth)
                .await?;
        }
        if !descriptor.supports_model_refresh() {
            return Ok(());
        }

        self.set_status(format!("refreshing {} model list", target.provider));
        terminal.draw(|frame| self.draw(frame))?;
        let config = self.info.services.config_repository.load()?;
        let endpoint = config.resolved_provider_endpoint(&target.provider);
        let model_endpoint = endpoint.as_ref().map_or(
            ProviderModelEndpoint::ProviderOwned,
            ProviderModelEndpoint::OpenAiCompatible,
        );
        match refresh_provider_models_with_store(
            &target.provider,
            &target.auth,
            self.credential_store.as_ref(),
            model_endpoint,
        )
        .await
        {
            Ok(refresh) => self.apply_model_list_refresh(refresh),
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "stored credentials for {}, but failed to refresh its model list: {err}",
                    target.provider
                )));
            }
        }
        Ok(())
    }

    /// Refreshes the decision models `provider` lists, for the permission
    /// screen's picker, and reports the count or the failure. A chat host that
    /// lists none (most Ollama servers) stays quiet.
    pub(super) async fn refresh_decision_models(
        &mut self,
        provider: &str,
        auth: &str,
    ) -> anyhow::Result<()> {
        let config = self.info.services.config_repository.load()?;
        let Some(api_base) = config.resolved_provider_endpoint(provider) else {
            return Ok(());
        };
        let serves_chat = provider::provider_descriptor(provider)
            .is_some_and(|descriptor| descriptor.serves_chat());
        match refresh_decision_models_with_store(
            provider,
            auth,
            self.credential_store.as_ref(),
            &api_base,
        )
        .await
        {
            Ok(models) if !models.is_empty() || !serves_chat => {
                self.insert_entry(&Entry::Notice(format!(
                    "refreshed {provider} decision models: {} models",
                    models.len()
                )));
            }
            Ok(_) => {}
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not refresh {provider} decision models: {err}"
                )));
            }
        }
        Ok(())
    }

    /// Saves the running provider, model, auth, and reasoning as the
    /// defaults new sessions start with.
    pub(super) fn save_current_config(&self) -> anyhow::Result<()> {
        self.info.services.config_repository.update(|config| {
            config.provider = self.info.runtime.provider.clone();
            config.model = self.info.runtime.model.clone();
            config.auth = self.info.runtime.auth.clone();
            config.reasoning = self.info.runtime.reasoning;
        })
    }

    /// When the edit tool preference is Auto, advertises the preferred
    /// format for `provider`. Nothing is saved; a failure shows as an error
    /// row and leaves the switch.
    pub(super) async fn apply_auto_edit_tool_for_provider(
        &mut self,
        provider: &str,
        agent: &mut InteractiveRuntime,
    ) {
        let config = match self.info.services.config_repository.load() {
            Ok(config) => config,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not follow auto edit tool: {error}"
                )));
                return;
            }
        };
        if config.edit_tool != crate::config::EditTool::Auto {
            return;
        }
        let resolved = config.resolved_edit_tool_for_provider(provider);
        match agent.set_edit_tool(resolved, config.max_output_bytes).await {
            Ok(Some(change)) => {
                // Leave the caller's status alone: a model switch keeps its
                // own feedback.
                self.info
                    .services
                    .diagnostics
                    .update_tools(&agent.tool_specs());
                self.insert_entry(&Entry::Notice(change.display));
            }
            Ok(None) => {}
            Err(error) => self.insert_entry(&Entry::Error(format!(
                "could not follow auto edit tool: {error}"
            ))),
        }
    }
}
