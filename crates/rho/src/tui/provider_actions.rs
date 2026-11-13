use super::*;
use rho_providers::model::decision_models::{
    lists_decision_models, refresh_decision_models_with_store,
};
use rho_providers::model::provider_models::ProviderModelEndpoint;

impl App {
    /// Makes `target`'s auth profile the config's when its provider is the
    /// config's default, and changes nothing else there. All a login
    /// writes: the session's own model is no default.
    pub(super) fn persist_default_login_auth(&mut self, target: &LoginTarget) {
        if target.auth == rho_providers::provider::KEYLESS_AUTH {
            return;
        }
        let result = self.info.services.config_repository.update(|config| {
            if config.provider == target.provider {
                config.auth = target.auth.clone();
            }
        });
        if let Err(err) = result {
            self.insert_entry(&Entry::Error(format!(
                "stored credentials, but saving auth mode failed: {err}"
            )));
        }
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
}
