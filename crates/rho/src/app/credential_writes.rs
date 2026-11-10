//! Signing in with an API key and signing out, for the daemon, which acts
//! for a client rather than a person at a terminal (`rho/auth/key`,
//! `rho/auth/logout`, and the environment and store checks of
//! `rho/auth/login`), and the shell check of `rho login` (#1662). Unix-only, as the daemon is
//! (#1665):
//!
//! - A request starts with [`load_config_and_store`], so it writes the store
//!   config selects when it runs, not the one the process opened at start:
//!   a store a terminal login chose since then is the one written.
//! - A credential set in the process environment wins over a saved one, so
//!   signing in or out of its auth profile refuses before writing anything.
//! - An API key is checked against the provider's models endpoint and saved
//!   unless the provider refuses it.
//! - Every write moves the credential generation (see
//!   [`credential_store::AppCredentialStore::generation`]), so the process's running sessions
//!   use the change from their next prompt. A refused request writes
//!   nothing.

use std::fmt;

use anyhow::Context;
use http::StatusCode;
use rho_providers::{
    auth::login_dispatch::ProviderAuthentication,
    credentials::{self, CredentialError, CredentialStore},
    model::{
        provider_models::{check_api_key, ProviderModelEndpoint},
        ModelError,
    },
    provider::{self, ProviderAuthKind, ProviderDescriptor},
};

use super::{bootstrap, config_repository::ConfigRepository, endpoint_login};
use crate::{config::Config, credential_store};

/// Why a sign-in or sign-out wrote nothing, or failed partway. No variant's
/// message holds a key.
#[derive(Debug, thiserror::Error)]
pub(crate) enum CredentialWriteError {
    #[error("unknown auth profile '{auth}'")]
    UnknownAuth { auth: String },
    #[error("'{auth}' does not take an API key")]
    NotAnApiKey { auth: String },
    /// A keyless auth profile, which names no credentials and, being every
    /// keyless provider's, no provider.
    #[error("'{auth}' has no credentials to sign out of")]
    NoCredentials { auth: String },
    /// A variable in the process environment holds `auth`'s credential,
    /// which wins over a saved one: saving or deleting one would change
    /// nothing.
    #[error("'{auth}' reads its credential from {variable}; unset it to {write}")]
    EnvironmentOverride {
        auth: String,
        variable: &'static str,
        write: CredentialWrite,
    },
    #[error("the key is blank")]
    BlankKey,
    /// The provider refused the key: 400 or 401 from its models endpoint.
    /// `reason` says what failed.
    #[error("{provider} refused the key: {reason}")]
    KeyRefused {
        provider: &'static str,
        reason: String,
    },
    /// Config could not be read or updated.
    #[error("{0:#}")]
    Config(anyhow::Error),
    /// The credential store could not be opened, read, or written.
    #[error("{0:#}")]
    Store(anyhow::Error),
}

/// What an environment override refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CredentialWrite {
    SignIn,
    /// An interactive login, whose credential is not a key.
    Login,
    SignOut,
}

/// What the user does once the variable is unset.
impl fmt::Display for CredentialWrite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::SignIn => "sign in with a saved key",
            Self::Login => "sign in with a login",
            Self::SignOut => "sign out",
        })
    }
}

/// Loads `repository`'s config, publishes its custom providers, as a
/// session open does, and makes the store it selects the one
/// [`credential_store::AppCredentialStore`] uses (with no store chosen, the OS store).
/// Custom providers' auth profiles resolve, and their models endpoints
/// answer a key check, only once published; running sessions keep the
/// providers they were built with. Switching stores moves no session to it:
/// providers keep what they read until a write moves the generation or
/// their credentials are refused.
pub(crate) fn load_config_and_store(
    repository: &ConfigRepository,
) -> Result<Config, CredentialWriteError> {
    let unloaded =
        |error: anyhow::Error| CredentialWriteError::Config(error.context("could not load config"));
    let config = repository.load().map_err(unloaded)?;
    config.providers.activate().map_err(unloaded)?;
    let path = bootstrap::absolute_config_path(repository).map_err(unloaded)?;
    credential_store::activate_from_config(&config, &path)
        .context("could not open the credential store")
        .map_err(CredentialWriteError::Store)?;
    Ok(config)
}

/// The variable that holds auth profile `auth`'s credential in the
/// environment `environment` reads, where it wins over a saved one. A host
/// checks its own ([`refuse_environment_override`]); `rho login` checks its
/// shell's too, since every rho started there reads it.
pub(crate) fn environment_override(
    auth: &str,
    environment: impl Fn(&str) -> Option<String>,
) -> Option<&'static str> {
    let (_, mode) = provider::resolve_auth_mode(auth)?;
    mode.auth_kind
        .env_var()
        .filter(|_| credentials::auth_has_env_override_from(auth, environment))
}

/// Refuses auth profile `auth` when the environment of whoever
/// `credentials` belong to overrides its saved credential: this process's,
/// or a daemon client's.
pub(crate) fn refuse_environment_override(
    auth: &str,
    write: CredentialWrite,
    credentials: &dyn CredentialStore,
) -> Result<(), CredentialWriteError> {
    match environment_override(auth, |variable| credentials.environment_variable(variable)) {
        Some(variable) => Err(CredentialWriteError::EnvironmentOverride {
            auth: auth.into(),
            variable,
            write,
        }),
        None => Ok(()),
    }
}

/// An account nothing saves, looked up to ask whether a store answers.
const REACHABILITY_ACCOUNT: &str = "rho-probe:reachable";

/// Refuses when the store `credentials` are kept in cannot be reached, as
/// the OS store of a daemon started without a session D-Bus cannot. For a
/// sign-in the user works for before anything is saved, an interactive
/// login, so it fails before the provider is asked rather than after the
/// user approved it. Looks up an account that does not exist, past the OS
/// store's cache, as [`credentials::check_credential_store`] does for the
/// daemon's status: it writes nothing, and a locked keyring asks for no
/// unlock. Only why goes in the error; choosing another store is the
/// client's advice to give. Blocks.
pub(crate) fn refuse_unreachable_store(
    credentials: &dyn CredentialStore,
) -> Result<(), CredentialWriteError> {
    credentials
        .get_secret_fresh(REACHABILITY_ACCOUNT)
        .map(drop)
        .map_err(|error| {
            CredentialWriteError::Store(match error {
                CredentialError::StoreUnavailable(reason) => {
                    anyhow::anyhow!("the daemon cannot reach the credential store: {reason}")
                }
                error @ CredentialError::InvalidData(_) => error.into(),
            })
        })
}

/// Signs out of auth profile `auth`, which a client named, on the provider
/// it signs in to, as `rho/auth/logout` signs out, deleting from
/// `credentials`.
pub(crate) fn sign_out_named(
    auth: &str,
    repository: &ConfigRepository,
    credentials: &dyn CredentialStore,
) -> Result<(), CredentialWriteError> {
    load_config_and_store(repository)?;
    let Some((descriptor, mode)) = provider::resolve_auth_mode(auth) else {
        return Err(CredentialWriteError::UnknownAuth { auth: auth.into() });
    };
    if matches!(mode.auth_kind, ProviderAuthKind::None) {
        return Err(CredentialWriteError::NoCredentials { auth: auth.into() });
    }
    refuse_environment_override(auth, CredentialWrite::SignOut, credentials)?;
    write_sign_out(auth, Some(descriptor), repository, credentials)
}

/// Deletes `auth`'s stored credentials, and forgets `descriptor`'s login
/// when it is logged in through its endpoint (Ollama), whose login counts
/// with or without a key. Success always means the profile needs a sign-in:
/// callers refused an environment override.
fn write_sign_out(
    auth: &str,
    descriptor: Option<&'static ProviderDescriptor>,
    repository: &ConfigRepository,
    credentials: &dyn CredentialStore,
) -> Result<(), CredentialWriteError> {
    ProviderAuthentication::delete_credentials(credentials, auth)
        .with_context(|| format!("could not delete {auth} credentials"))
        .map_err(CredentialWriteError::Store)?;
    if let Some(descriptor) = descriptor {
        endpoint_login::forget(repository, *descriptor)
            .with_context(|| format!("could not remove the {} login", descriptor.name))
            .map_err(CredentialWriteError::Config)?;
    }
    Ok(())
}

/// How a saved key was checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum KeyCheck {
    /// The provider's models endpoint accepted it.
    Accepted,
    /// Rho lists no models for the provider, so it cannot ask about a key.
    NoModelListing,
    /// The provider could not be asked (an outage, a gateway without
    /// `/models`): why, with any copy of the key removed.
    Unreachable(String),
    /// The models endpoint answered 403: the key may not list models, as a
    /// key restricted to running them may not, which says nothing about
    /// whether it works (#1662).
    Forbidden,
}

/// An auth profile an API key can sign in to: it takes a key, and no
/// environment variable overrides a saved one. Keys passed to its methods
/// are trimmed and never empty.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ApiKeyAuth {
    auth: &'static str,
    descriptor: &'static ProviderDescriptor,
}

impl ApiKeyAuth {
    /// Checks that `auth` can take a saved key. A custom provider's profile
    /// resolves once [`load_config_and_store`] (or startup) published it.
    /// Refused when the environment of whoever `credentials` belong to
    /// already holds the profile's key.
    pub(crate) fn resolve(
        auth: &str,
        credentials: &dyn CredentialStore,
    ) -> Result<Self, CredentialWriteError> {
        let Some((descriptor, mode)) = provider::resolve_auth_mode(auth) else {
            return Err(CredentialWriteError::UnknownAuth { auth: auth.into() });
        };
        let ProviderAuthKind::ApiKey { .. } = mode.auth_kind else {
            return Err(CredentialWriteError::NotAnApiKey { auth: auth.into() });
        };
        // The variable wins over a saved key in that party's sessions, so
        // saving one would change nothing for them.
        refuse_environment_override(auth, CredentialWrite::SignIn, credentials)?;
        Ok(Self {
            auth: mode.id,
            descriptor,
        })
    }

    pub(crate) fn auth(&self) -> &'static str {
        self.auth
    }

    /// Asks the models endpoint `config` resolves for the provider about
    /// `key`, failing only [`CredentialWriteError::KeyRefused`]. Callers
    /// save a key the provider cannot be asked about (no model listing in
    /// Rho, an outage, a gateway without `/models`, a key that may not list
    /// models) and say it went unchecked, so such a provider can still be
    /// signed in to.
    ///
    /// Response bodies may quote the request, so only statuses show (see
    /// [`status_check`]).
    pub(crate) async fn check(
        &self,
        key: &str,
        config: &Config,
    ) -> Result<KeyCheck, CredentialWriteError> {
        if !self.descriptor.supports_model_refresh() {
            return Ok(KeyCheck::NoModelListing);
        }
        let base = config.resolved_provider_endpoint(self.descriptor.name);
        let endpoint = base.as_ref().map_or(
            ProviderModelEndpoint::ProviderOwned,
            ProviderModelEndpoint::OpenAiCompatible,
        );
        let Err(error) = check_api_key(self.auth, key, endpoint).await else {
            return Ok(KeyCheck::Accepted);
        };
        let provider = self.descriptor.display_name;
        let status = match &error {
            ModelError::MissingCredentials(message) => {
                return Err(CredentialWriteError::KeyRefused {
                    provider,
                    reason: (*message).to_owned(),
                });
            }
            ModelError::HttpStatus { status, .. } => *status,
            ModelError::Request(error) => match error.status() {
                Some(status) => status,
                // A transport error's text may quote the URL, where a key
                // can sit percent-encoded beyond redaction's reach, so it
                // never shows (#1662).
                None if error.is_connect() || error.is_timeout() => {
                    return Ok(KeyCheck::Unreachable("could not reach the provider".into()));
                }
                None => {
                    return Ok(KeyCheck::Unreachable(
                        "the request to the provider failed".into(),
                    ));
                }
            },
            ModelError::Credentials(_)
            | ModelError::Io(_)
            | ModelError::Interrupted
            | ModelError::StreamIdleTimeout { .. }
            | ModelError::StreamFailedAfterOutput { .. }
            | ModelError::InvalidResponse(_)
            | ModelError::RetryableInvalidResponse { .. }
            | ModelError::ProviderReported { .. }
            | ModelError::UnsupportedReasoning { .. }
            | ModelError::UnsupportedProvider(_) => {
                return Ok(KeyCheck::Unreachable(redacted(error.to_string(), key)));
            }
        };
        status_check(status, provider)
    }

    /// Saves `key` to `store`.
    pub(crate) fn save(
        &self,
        key: &str,
        store: &dyn CredentialStore,
    ) -> Result<(), CredentialWriteError> {
        ProviderAuthentication::save_api_key(store, self.auth, key).map_err(|error| {
            CredentialWriteError::Store(anyhow::anyhow!(
                "could not save the key: {}",
                redacted(error.to_string(), key)
            ))
        })
    }

    /// Records the login of a host logged in through its endpoint (Ollama),
    /// which counts only once recorded; other providers are left alone.
    pub(crate) fn record_login(
        &self,
        repository: &ConfigRepository,
    ) -> Result<(), CredentialWriteError> {
        endpoint_login::record(repository, *self.descriptor)
            .context("saved the key, but could not record the login")
            .map_err(CredentialWriteError::Config)
    }
}

/// What a failing `status` from `provider`'s models endpoint says about the
/// key it was asked with. A plain `GET /models` that carries only the key
/// fails 400 or 401 only for the key itself; Google answers a malformed key
/// with 400. A 403 says the key may not list models, not that it cannot run
/// them, so the key is saved unchecked (#1662).
fn status_check(
    status: StatusCode,
    provider: &'static str,
) -> Result<KeyCheck, CredentialWriteError> {
    match status.as_u16() {
        400 | 401 => Err(CredentialWriteError::KeyRefused {
            provider,
            reason: format!("its models endpoint answered HTTP {status}"),
        }),
        403 => Ok(KeyCheck::Forbidden),
        _ => Ok(KeyCheck::Unreachable(format!("HTTP {status}"))),
    }
}

/// `message` with any copy of `key` removed, in case an error quoted it.
fn redacted(message: String, key: &str) -> String {
    message.replace(key, "[key]")
}

#[cfg(test)]
#[path = "credential_writes_tests.rs"]
mod tests;
