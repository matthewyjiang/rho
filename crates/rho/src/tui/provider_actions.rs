use super::*;
use rho_providers::auth::login_dispatch::ProviderAuthentication;

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
        self.info.runtime.provider = activation.provider;
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
            .await?;
        Ok(outcome)
    }

    pub(super) async fn switch_active_auth_mode(
        &mut self,
        auth: &str,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let provider_name = self.info.runtime.provider.clone();
        let Some(descriptor) = rho_providers::provider::provider_descriptor(&provider_name) else {
            self.insert_entry(&Entry::Error(format!(
                "unsupported provider '{provider_name}'"
            )));
            self.set_status("auth switch failed");
            return Ok(());
        };
        let Some(mode) = descriptor.auth_mode(auth) else {
            self.insert_entry(&Entry::Error(format!(
                "auth mode '{auth}' does not belong to {provider_name}"
            )));
            self.set_status("auth switch failed");
            return Ok(());
        };
        if !ProviderAuthentication::has_credentials(self.credential_store.as_ref(), mode.id)? {
            self.insert_entry(&Entry::Error(format!(
                "credentials for {} are unavailable. Run /login {} to sign in again.",
                mode.login_label, mode.id
            )));
            self.set_status("auth switch failed");
            return Ok(());
        }
        if self.info.runtime.auth == mode.id {
            self.set_status(format!("active auth: {}", mode.login_label));
            return Ok(());
        }

        let model = self.info.runtime.model.clone();
        let reasoning = self.info.runtime.reasoning;
        let new_provider = match self
            .build_provider_for_selection(&provider_name, &model, reasoning, mode.id)
            .await
        {
            Ok(provider) => provider,
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not switch to {}: {err}. Run /login {} to sign in again.",
                    mode.login_label, mode.id
                )));
                self.set_status("auth switch failed");
                return Ok(());
            }
        };

        let activation = ProviderActivation {
            provider: provider_name,
            model,
            reasoning: reasoning_metadata::ModelSwitchReasoningResolution {
                effective: reasoning,
                source: self.info.runtime.reasoning_source,
            },
            auth: mode.id.into(),
            replacement: new_provider,
        };
        let outcome = self.activate_provider(activation, agent).await?;
        self.refresh_available_auths();
        match outcome {
            ProviderActivationOutcome::Saved => {
                self.set_status(format!(
                    "switched {} to {}",
                    descriptor.display_name, mode.login_label
                ));
            }
            ProviderActivationOutcome::ConfigSaveFailed(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "auth mode switched, but saving config failed: {err}"
                )));
                self.set_status("config save failed");
            }
        }
        Ok(())
    }
}
