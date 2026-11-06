//! Picks up a new login after the provider refused a session's credentials,
//! and drops credentials a host logout deleted.
//!
//! Providers capture their key or OAuth tokens when built, so a prompt that
//! fails on its credentials leaves the session holding the refused ones, and
//! a logout leaves it holding deleted ones. The next prompt rebuilds the
//! provider from the credential store, so a host's sign-in (or a
//! `rho --login` elsewhere) applies without reloading, and a logout fails it
//! with the typed provider error, as missing credentials do.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use rho_providers::model::ModelError;

use super::{model_scoped_config, SessionRuntime};
use crate::{
    config::Config,
    credential_store::{build_provider_from_config_ensuring_catalog, AppCredentialStore},
};

/// How many logouts the host has completed. The host records each one; the
/// runtimes of its sessions compare against it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Logouts(Arc<AtomicU64>);

/// A [`Logouts`] reading, taken before a provider reads credentials.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LogoutCount(u64);

impl Logouts {
    pub(crate) fn count(&self) -> LogoutCount {
        LogoutCount(self.0.load(Ordering::Acquire))
    }

    /// Call once the credentials are deleted, so a provider that read them
    /// first holds an older count.
    pub(crate) fn record(&self) {
        self.0.fetch_add(1, Ordering::Release);
    }

    /// Whether a logout finished after `read` was taken.
    pub(crate) fn since(&self, read: LogoutCount) -> bool {
        self.count() != read
    }
}

/// A failure that signing in can fix. This follows the provider layer, which
/// files each of these under `ProviderErrorKind::Authentication` when it
/// converts them for the SDK, so startup and prompt failures agree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CredentialFailure {
    /// Nothing is saved or set in the environment for the selected auth. The
    /// message comes from the provider descriptor table, never from a
    /// credential, and names the provider and how to sign in.
    Missing(&'static str),
    /// The credential store failed to read or refresh the saved credentials;
    /// signing in again saves a fresh copy.
    Unreadable,
    /// The provider answered 401/403 or reported an authentication failure:
    /// an expired, revoked, or wrong token or key.
    Refused,
}

impl CredentialFailure {
    /// Finds a credential failure in `error`'s cause chain. Only typed causes
    /// count, so rewording an error message cannot reclassify it.
    pub(crate) fn find(error: &anyhow::Error) -> Option<Self> {
        error.chain().find_map(Self::of_cause)
    }

    fn of_cause(cause: &(dyn std::error::Error + 'static)) -> Option<Self> {
        if let Some(error) = cause.downcast_ref::<ModelError>() {
            return match error {
                ModelError::MissingCredentials(message) => Some(Self::Missing(message)),
                ModelError::Credentials(_) => Some(Self::Unreadable),
                ModelError::HttpStatus { status, .. } if matches!(status.as_u16(), 401 | 403) => {
                    Some(Self::Refused)
                }
                ModelError::Request(_)
                | ModelError::HttpStatus { .. }
                | ModelError::Io(_)
                | ModelError::Interrupted
                | ModelError::StreamIdleTimeout { .. }
                | ModelError::StreamFailedAfterOutput { .. }
                | ModelError::InvalidResponse(_)
                | ModelError::RetryableInvalidResponse { .. }
                | ModelError::ProviderReported { .. }
                | ModelError::UnsupportedReasoning { .. }
                | ModelError::UnsupportedProvider(_) => None,
            };
        }
        // The SDK error no longer says which credential failure it was.
        // `rho_sdk::Error` is non-exhaustive; other variants are not
        // credential failures.
        match cause.downcast_ref::<rho_sdk::Error>()? {
            rho_sdk::Error::Authentication { .. } => Some(Self::Refused),
            rho_sdk::Error::Provider(error)
                if error.kind() == rho_sdk::ProviderErrorKind::Authentication =>
            {
                Some(Self::Refused)
            }
            _ => None,
        }
    }
}

/// Whether the session's provider still holds credentials the store has.
#[derive(Clone, Debug)]
pub(crate) struct SessionCredentials {
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
    /// the host's install gate, which a logout waits out, so none reads
    /// credentials before a logout and counts after it.
    pub(super) fn just_read(logouts: &Logouts) -> Self {
        Self {
            state: CredentialState::Read(logouts.count()),
            logouts: logouts.clone(),
        }
    }

    /// Take before building a provider and pass to
    /// [`SessionRuntime::switch_provider`], so a logout during the build
    /// still counts as after it.
    pub(crate) fn before_read(&self) -> LogoutCount {
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

impl SessionRuntime {
    /// Rebuilds a stale provider for the same model and auth. Still-missing
    /// credentials fail with the typed provider error, which hosts report as
    /// a sign-in request again.
    pub(super) async fn renew_stale_credentials(
        &mut self,
        process_config: &Config,
        config_path: &std::path::Path,
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
        // A keyless login lives in config, which `logout` edits on disk.
        let login_config = crate::app::endpoint_login::with_disk_login(&config, config_path)?;
        let provider = build_provider_from_config_ensuring_catalog(
            &login_config,
            Arc::new(AppCredentialStore),
        )
        .await?;
        // The model switch path also rebinds the compactor, which holds its
        // own provider. The model is unchanged, so no switch notice is saved.
        self.switch_provider(provider, &config, read)
            .map_err(|error| anyhow::anyhow!("could not renew credentials: {error}"))?;
        Ok(())
    }

    /// Marks the provider stale when `result` failed on its credentials, so
    /// the next prompt renews it.
    pub(super) fn note_credential_failure<T>(&mut self, result: &anyhow::Result<T>) {
        if result
            .as_ref()
            .is_err_and(|error| CredentialFailure::find(error).is_some())
        {
            self.credentials.state = CredentialState::Refused;
        }
    }
}
