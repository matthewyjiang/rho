//! Picks up credential changes in a running session: a key saved or a
//! logout made by any client of the process, and a new login after the
//! provider refused the session's credentials.
//!
//! Providers capture their key or OAuth tokens when built, so a session
//! would otherwise keep presenting what it read at build time: a replaced
//! key the provider now refuses, or a deleted one it still accepts. Before
//! each prompt the session compares the process's credential generation
//! (see [`AppCredentialStore::generation`]) with the one its provider read
//! at, and rebuilds the provider from the store once it moved or a prompt
//! failed on its credentials (#1662). A save or logout from another client
//! of the same process (ACP, the daemon) thus applies from the next prompt,
//! a refusal picks up a `rho login` run elsewhere, and a logout fails the
//! prompt with the typed provider error, as missing credentials do, until a
//! later prompt finds a login, whichever process saved it.

use rho_providers::model::ModelError;

use super::{model_scoped_config, SessionRuntime};
use crate::app::session_assembly::ProviderCredentials;
use crate::{
    config::Config,
    credential_store::{
        build_provider_from_config_ensuring_catalog, AppCredentialStore, CredentialGeneration,
    },
};

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

    /// What to tell a client, in Rho's words: provider messages and
    /// diagnostics may quote the request, so none are passed on.
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Missing(message) => message,
            Self::Unreadable => "could not read the stored credentials; sign in again",
            // After SDK conversion a 401, a 403 (which can mean the account
            // lacks access), and a store failure look alike.
            Self::Refused => {
                "could not authenticate with the provider; sign in again or check the account's access"
            }
        }
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionCredentials {
    /// Read from the store at this generation, taken with
    /// [`AppCredentialStore::generation`]; stale once it moves.
    Read(CredentialGeneration),
    /// A prompt failed on a credential failure, or the session opened
    /// without credentials; reacquire before the next prompt.
    Refused,
    /// A test's scripted provider, which reads no credentials and cannot be
    /// rebuilt from config, so another test moving the process-wide
    /// generation must not make it stale.
    #[cfg(test)]
    Scripted,
}

impl SessionCredentials {
    /// A session just opened on `built`, whose credentials were read at
    /// `read`. One that opened without them rereads other processes' logins
    /// at its first prompt, as after a refusal, so a `rho login` run
    /// elsewhere since applies to that prompt.
    pub(super) fn opened(read: CredentialGeneration, built: ProviderCredentials) -> Self {
        match built {
            ProviderCredentials::Found => Self::Read(read),
            ProviderCredentials::Missing => Self::Refused,
        }
    }

    /// Records that the provider was rebuilt after `read` was taken. A
    /// scripted provider stays scripted: a test that switches it swaps one
    /// scripted provider for another.
    pub(super) fn mark_read(&mut self, read: CredentialGeneration) {
        match *self {
            Self::Read(_) | Self::Refused => *self = Self::Read(read),
            #[cfg(test)]
            Self::Scripted => {}
        }
    }
}

impl SessionRuntime {
    /// Rebuilds a stale provider for the same model and auth. Still-missing
    /// credentials fail with the typed provider error, which hosts report as
    /// a sign-in request again; the caller notes the failure
    /// ([`Self::note_credential_failure`]), so the next prompt rereads.
    pub(super) async fn renew_stale_credentials(
        &mut self,
        process_config: &Config,
    ) -> anyhow::Result<()> {
        match self.credentials {
            SessionCredentials::Read(read) if read == AppCredentialStore::generation() => {
                return Ok(());
            }
            // This process's writes keep the OS store's cache current, so
            // a moved generation needs no reread.
            SessionCredentials::Read(_) => {}
            // The login that fixes a refusal, or the credentials a renewal
            // did not find, may have run in another process, such as the
            // editor's terminal login.
            SessionCredentials::Refused => AppCredentialStore::reread_external_logins(),
            #[cfg(test)]
            SessionCredentials::Scripted => return Ok(()),
        }
        let read = AppCredentialStore::generation();
        let mut config = model_scoped_config(&self.current_model(), process_config);
        config.reasoning = self.built.session.reasoning_level();
        // A keyless login lives in config, which `logout` edits on disk.
        let login_config =
            crate::app::endpoint_login::with_disk_login(&config, &self.build.launch.config_path)?;
        let provider = build_provider_from_config_ensuring_catalog(
            &login_config,
            self.build.host.environment.credentials(),
        )
        .await?;
        // The model switch path also rebinds the compactor, which holds its
        // own provider. The model is unchanged, so no switch notice is saved.
        self.switch_provider(provider, &config, read)
            .map_err(|error| anyhow::anyhow!("could not renew credentials: {error}"))?;
        Ok(())
    }

    /// Marks the provider stale when `result` failed on its credentials, so
    /// the next prompt renews it from what the store holds then.
    pub(super) fn note_credential_failure<T>(&mut self, result: &anyhow::Result<T>) {
        if result
            .as_ref()
            .is_err_and(|error| CredentialFailure::find(error).is_some())
        {
            self.credentials = SessionCredentials::Refused;
        }
    }
}

#[cfg(test)]
#[path = "credentials_tests.rs"]
mod tests;
