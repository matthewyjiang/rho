//! Picks up a new login after the provider refused a session's credentials,
//! and drops credentials an ACP `logout` deleted.
//!
//! Providers capture their key or OAuth tokens when built, so a prompt that
//! fails with `auth_required` leaves the session holding the refused ones,
//! and a logout leaves it holding deleted ones. The next prompt rebuilds the
//! provider from the credential store, so a host's sign-in (or a
//! `rho --login` elsewhere) applies without reloading, and a logout fails it
//! with `auth_required` as missing credentials do.

use std::sync::Arc;

use super::{model_scoped_config, SessionHost};
use crate::{
    app::acp::{
        auth::CredentialFailure,
        auth_logout::{LogoutCount, Logouts},
    },
    config::Config,
    credential_store::{build_provider_from_config_ensuring_catalog, AppCredentialStore},
};

/// Whether the session's provider still holds credentials the store has.
#[derive(Clone, Debug)]
pub(super) struct SessionCredentials {
    logouts: Logouts,
    state: CredentialState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CredentialState {
    /// Read from the store when [`Logouts`] stood at this count; stale once
    /// a later logout finishes.
    Read(LogoutCount),
    /// A prompt failed on a credential failure; reacquire before the next.
    Refused,
}

impl SessionCredentials {
    /// For a provider that has just read credentials. Session builds hold
    /// the agent's install gate, which `logout` waits out, so none reads
    /// credentials before a logout and counts after it.
    pub(super) fn just_read(logouts: &Logouts) -> Self {
        Self {
            state: CredentialState::Read(logouts.count()),
            logouts: logouts.clone(),
        }
    }

    /// Take before building a provider and pass to [`SessionHost::switch_provider`],
    /// so a logout during the build still counts as after it.
    pub(super) fn before_read(&self) -> LogoutCount {
        self.logouts.count()
    }

    pub(super) fn mark_read(&mut self, read: LogoutCount) {
        self.state = CredentialState::Read(read);
    }

    fn stale(&self) -> bool {
        match self.state {
            CredentialState::Read(read) => self.logouts.since(read),
            CredentialState::Refused => true,
        }
    }
}

impl SessionHost {
    /// Rebuilds a stale provider for the same model and auth. Still-missing
    /// credentials fail with the typed provider error, which the agent maps
    /// to `auth_required` again.
    pub(super) async fn renew_stale_credentials(
        &mut self,
        process_config: &Config,
    ) -> anyhow::Result<()> {
        if !self.credentials.stale() {
            return Ok(());
        }
        let read = self.credentials.before_read();
        let mut config = model_scoped_config(&self.current_model(), process_config);
        config.reasoning = self.built.session.reasoning_level();
        // The login that fixes it ran in another process, such as the
        // editor's terminal login.
        AppCredentialStore::reread_external_logins();
        let provider =
            build_provider_from_config_ensuring_catalog(&config, Arc::new(AppCredentialStore))
                .await?;
        // The model switch path also rebinds the compactor, which holds its
        // own provider. The model is unchanged, so no switch notice is saved.
        self.switch_provider(provider, &config, read)
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
            self.credentials.state = CredentialState::Refused;
        }
    }
}
