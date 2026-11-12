//! The configured providers a session built on. Its model's provider, the
//! compaction summarizer, the advisor and Auto's classifier each look a
//! model up on them, and a title job reads them from the turn's config. A
//! daemon session takes the global config's at its next turn (#1685); the
//! prompt that starts then rebuilds each on the new ones before it runs. A
//! turn under way keeps the ones it began with.

use super::{model_scoped_config, SessionRuntime};
use crate::{
    config::{Config, ProviderConfigs},
    permission::PermissionMode,
};

impl SessionRuntime {
    /// Rebuilds what the session built on its configured providers when
    /// `process_config`'s differ from them. On error the session keeps its
    /// old ones and the prompt fails, so it never runs on providers its
    /// config no longer has; the next prompt tries again.
    pub(super) async fn refresh_providers(
        &mut self,
        process_config: &Config,
    ) -> anyhow::Result<()> {
        if self.providers == process_config.providers {
            return Ok(());
        }
        let current = self.current_model();
        let host = |providers: &ProviderConfigs| providers.host(&current.provider).cloned();
        match (host(&self.providers), host(&process_config.providers)) {
            (Some(_), None) => {
                // A configuration error, so clients say what to fix rather
                // than blame a host the config no longer names.
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
            // The model's own host changed: its provider is rebuilt, which
            // rebinds the compactor too.
            (was, now) if was != now => self.rebuild_provider(process_config).await?,
            // Only other hosts changed; the summarizer may be on one.
            _ => self
                .set_compaction(process_config)
                .map_err(|error| anyhow::anyhow!("could not rebuild the compactor: {error}"))?,
        }
        if let Some(store) = self.built.tools.advisor() {
            store.set_providers(process_config.providers.clone());
        }
        // Auto's classifier is built with the runtime's approvals.
        if self.permission.mode == PermissionMode::Auto {
            let config = model_scoped_config(&self.current_model(), process_config);
            self.rebuild_runtime(
                &config,
                self.permission.mode,
                self.permission.writes.clone(),
            )
            .await
            .map_err(|error| {
                anyhow::anyhow!("could not rebuild the permission classifier: {error}")
            })?;
        }
        self.providers = process_config.providers.clone();
        Ok(())
    }
}
