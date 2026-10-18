//! Picks up a new login after the provider refused a session's credentials.
//!
//! Providers capture their key or OAuth tokens when built, so a prompt that
//! fails with `auth_required` leaves the session holding the refused ones.
//! The next prompt rebuilds the provider from the credential store, so a
//! host's sign-in (or a `rho --login` elsewhere) applies without reloading.

use std::sync::Arc;

use super::{model_scoped_config, SessionHost};
use crate::{
    app::acp::auth::CredentialFailure,
    config::Config,
    credential_store::{build_provider_from_config_ensuring_catalog, AppCredentialStore},
};

/// Whether the session's provider still holds credentials it was built with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SessionCredentials {
    Current,
    /// A prompt failed on a credential failure; reacquire before the next.
    Stale,
}

impl SessionHost {
    /// Rebuilds a [`SessionCredentials::Stale`] provider for the same model
    /// and auth. Still-missing credentials fail with the typed provider error,
    /// which the agent maps to `auth_required` again.
    pub(super) async fn renew_stale_credentials(
        &mut self,
        process_config: &Config,
    ) -> anyhow::Result<()> {
        if self.credentials == SessionCredentials::Current {
            return Ok(());
        }
        let mut config = model_scoped_config(&self.current_model(), process_config);
        config.reasoning = self.built.session.reasoning_level();
        let provider =
            build_provider_from_config_ensuring_catalog(&config, Arc::new(AppCredentialStore))
                .await?;
        // The model switch path also rebinds the compactor, which holds its
        // own provider. The model is unchanged, so no switch notice is saved.
        self.switch_provider(provider, &config)
            .map_err(|error| match error.data {
                Some(serde_json::Value::String(detail)) => {
                    anyhow::anyhow!("could not renew credentials: {detail}")
                }
                _ => anyhow::anyhow!("could not renew credentials: {}", error.message),
            })?;
        Ok(())
    }

    /// Marks the provider stale when `result` failed on its credentials.
    pub(super) fn note_credential_failure<T>(&mut self, result: &anyhow::Result<T>) {
        if result
            .as_ref()
            .is_err_and(|error| CredentialFailure::find(error).is_some())
        {
            self.credentials = SessionCredentials::Stale;
        }
    }
}
