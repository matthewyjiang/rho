//! The configured providers a session built on. Its model's provider, the
//! compaction summarizer, the advisor and Auto's classifier each look a
//! model up on them, and a title job reads them from the turn's config. A
//! daemon session takes the global config's at its next turn (#1685); the
//! prompt that starts then rebuilds each on the new ones before it runs. A
//! turn under way keeps the ones it began with.

use super::{model_scoped_config, SessionRuntime, SettingChangeError};
use crate::{
    config::{Config, ProviderConfigs},
    permission::PermissionMode,
};

impl SessionRuntime {
    /// Rebuilds what the session built on its configured providers when
    /// `process_config`'s differ from them, before the credential renewal
    /// that follows in [`Self::begin_prompt`], which rebuilds the model's
    /// provider when its own host changed. Only the first rebuild can fail,
    /// before anything changed, and then the session keeps its old
    /// providers and the prompt fails; the next prompt tries again. A
    /// renewal that fails leaves the provider stale, for the next prompt.
    ///
    /// A changed host takes effect for what is built from it per provider
    /// or call: its endpoint and request settings. What the tool set read
    /// at open, such as its edit format, waits for the session to reopen.
    pub(super) async fn refresh_providers(
        &mut self,
        process_config: &Config,
    ) -> anyhow::Result<()> {
        if self.providers == process_config.providers {
            return Ok(());
        }
        let current = self.current_model();
        let host = |providers: &ProviderConfigs| providers.host(&current.provider).cloned();
        let (was, now) = (host(&self.providers), host(&process_config.providers));
        if was.is_some() && now.is_none() {
            // A logged-out Ollama asks to sign in again, as before.
            process_config
                .providers
                .require_endpoint_login(&current.provider)?;
            // Otherwise a configuration error, so clients say what to fix
            // rather than blame a host the config no longer names.
            let provider = &current.provider;
            return Err(rho_sdk::Error::InvalidConfiguration {
                message: format!(
                    "the session's model {provider}/{} is on provider '{provider}', which is no longer configured; add it back, or switch the session to another model",
                    current.model
                ),
                source: None,
            }
            .into());
        }
        // Auto's classifier is built with the runtime's approvals, and the
        // compactor's summarizer may be on a host that changed. Auto's
        // rebuild rebuilds the compactor too, as does the renewal when the
        // model's own host changed.
        let rebuilt = if self.permission.mode == PermissionMode::Auto {
            let config = model_scoped_config(&current, process_config);
            self.rebuild_runtime(
                &config,
                self.permission.mode,
                self.permission.writes.clone(),
            )
            .await
        } else if was == now {
            self.set_compaction(process_config)
        } else {
            Ok(())
        };
        rebuilt.map_err(|error: SettingChangeError| {
            anyhow::anyhow!("could not rebuild on the changed providers: {error}")
        })?;
        if let Some(store) = self.built.tools.advisor() {
            store.set_providers(process_config.providers.clone());
        }
        if was != now {
            self.credentials.mark_stale();
        }
        self.providers = process_config.providers.clone();
        Ok(())
    }
}
