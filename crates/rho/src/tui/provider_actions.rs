use super::*;
use rho_providers::auth::login_dispatch::ProviderAuthentication;

impl App {
    /// Builds a provider selection on top of persisted application config so
    /// transport settings (notably custom endpoints) survive live rebuilds.
    pub(super) fn build_provider_for_selection(
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
        Ok(crate::credential_store::build_provider_from_config(
            &config,
            std::sync::Arc::clone(&self.credential_store),
        )?)
    }

    pub(super) fn switch_active_auth_mode(
        &mut self,
        auth: &str,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let provider_name = self.info.runtime.provider.clone();
        let Some(descriptor) = rho_providers::provider::provider_descriptor(&provider_name) else {
            self.insert_entry(&Entry::Error(format!(
                "unsupported provider '{provider_name}'"
            )));
            self.status = "auth switch failed".into();
            return Ok(());
        };
        let Some(mode) = descriptor.auth_mode(auth) else {
            self.insert_entry(&Entry::Error(format!(
                "auth mode '{auth}' does not belong to {provider_name}"
            )));
            self.status = "auth switch failed".into();
            return Ok(());
        };
        if !ProviderAuthentication::has_credentials(self.credential_store.as_ref(), mode.id)
            .unwrap_or(false)
        {
            self.insert_entry(&Entry::Error(format!(
                "credentials for {} are unavailable. Run /login {} to sign in again.",
                mode.login_label, mode.id
            )));
            self.status = "auth switch failed".into();
            return Ok(());
        }
        if self.info.runtime.auth == mode.id {
            self.status = format!("active auth: {}", mode.login_label);
            return Ok(());
        }

        let model = self.info.runtime.model.clone();
        let reasoning = self.info.runtime.reasoning;
        let new_provider =
            match self.build_provider_for_selection(&provider_name, &model, reasoning, mode.id) {
                Ok(provider) => provider,
                Err(err) => {
                    self.insert_entry(&Entry::Error(format!(
                        "could not switch to {}: {err}. Run /login {} to sign in again.",
                        mode.login_label, mode.id
                    )));
                    self.status = "auth switch failed".into();
                    return Ok(());
                }
            };

        agent.replace_provider(new_provider, reasoning, mode.id)?;
        self.info.runtime.auth = mode.id.into();
        self.info.services.auth_unavailable = None;
        self.using_unavailable_provider = false;
        self.refresh_available_auths();
        self.start_model_metadata_fetch(agent);
        match self.save_current_config() {
            Ok(()) => {
                self.insert_entry(&Entry::Notice(format!(
                    "switched {} to {}",
                    descriptor.display_name, mode.login_label
                )));
                self.status = format!("active auth: {}", mode.login_label);
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "auth mode switched, but saving config failed: {err}"
                )));
                self.status = "config save failed".into();
            }
        }
        Ok(())
    }
}
