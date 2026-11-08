//! Signing in with an API key and signing out, for hosts that act for a
//! client rather than a person at a terminal: ACP (`authenticate`, `logout`)
//! and the daemon (`rho/auth/key`, `rho/auth/logout`) (#1662).
//!
//! Each host keeps how it reaches its user (ACP's local key page, the
//! daemon's socket); this module owns what a sign-in or sign-out does, so
//! both behave the same:
//!
//! - A request starts with [`load_config_and_store`], so it writes the store
//!   config selects when it runs, not the one the process opened at start:
//!   a store a terminal login chose since then is the one written.
//! - A credential set in the process environment wins over a saved one, so
//!   signing in or out of its auth profile refuses before writing anything.
//! - An API key is checked against the provider's models endpoint and saved
//!   unless the provider refuses it.
//! - Every write moves the credential generation (see
//!   [`AppCredentialStore::generation`]), so the process's running sessions
//!   use the change from their next prompt. A refused request writes
//!   nothing.

use std::fmt;

use anyhow::Context;
use rho_providers::{
    auth::login_dispatch::ProviderAuthentication,
    credentials::CredentialStore,
    model::{
        provider_models::{check_api_key, ProviderModelEndpoint},
        ModelError,
    },
    provider::{self, AuthMode, ProviderAuthKind, ProviderDescriptor},
};

use super::{bootstrap, config_repository::ConfigRepository, endpoint_login};
use crate::{
    config::Config,
    credential_store::{self, AppCredentialStore},
};

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
    /// The provider refused the key: 400, 401, or 403 from its models
    /// endpoint.
    #[error("{provider} refused the key")]
    KeyRefused { provider: &'static str },
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
    SignOut,
}

/// What the user does once the variable is unset.
impl fmt::Display for CredentialWrite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::SignIn => "sign in with a saved key",
            Self::SignOut => "sign out",
        })
    }
}

/// Loads `repository`'s config, publishes its custom providers, as a
/// session open does, and makes the store it selects the one
/// [`AppCredentialStore`] uses (with no store chosen, the OS store).
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
    credential_store::activate_for_login(&config, &path)
        .context("could not open the credential store")
        .map_err(CredentialWriteError::Store)?;
    Ok(config)
}

/// Refuses `auth`, whose mode is `mode`, when the environment overrides its
/// saved credential.
fn refuse_environment_override(
    auth: &str,
    mode: &AuthMode,
    write: CredentialWrite,
) -> Result<(), CredentialWriteError> {
    match mode.auth_kind.env_var() {
        Some(variable) if ProviderAuthentication::has_environment_override(auth) => {
            Err(CredentialWriteError::EnvironmentOverride {
                auth: auth.into(),
                variable,
                write,
            })
        }
        Some(_) | None => Ok(()),
    }
}

/// Signs out of auth profile `auth` on `provider`, the ones a host's config
/// selects, as ACP `logout` signs out. The keyless profile is shared by
/// every keyless provider, so only `provider` says which endpoint login to
/// forget. `repository` is the config the host reads.
pub(crate) fn sign_out_selected(
    provider: &str,
    auth: &str,
    repository: &ConfigRepository,
) -> Result<(), CredentialWriteError> {
    load_config_and_store(repository)?;
    if let Some((_, mode)) = provider::resolve_auth_mode(auth) {
        refuse_environment_override(auth, &mode, CredentialWrite::SignOut)?;
    }
    write_sign_out(auth, provider::provider_descriptor(provider), repository)
}

/// Signs out of auth profile `auth`, which a client named, on the provider
/// it signs in to, as `rho/auth/logout` signs out.
pub(crate) fn sign_out_named(
    auth: &str,
    repository: &ConfigRepository,
) -> Result<(), CredentialWriteError> {
    load_config_and_store(repository)?;
    let Some((descriptor, mode)) = provider::resolve_auth_mode(auth) else {
        return Err(CredentialWriteError::UnknownAuth { auth: auth.into() });
    };
    if matches!(mode.auth_kind, ProviderAuthKind::None) {
        return Err(CredentialWriteError::NoCredentials { auth: auth.into() });
    }
    refuse_environment_override(auth, &mode, CredentialWrite::SignOut)?;
    write_sign_out(auth, Some(descriptor), repository)
}

/// Deletes `auth`'s stored credentials, and forgets `descriptor`'s login
/// when it is logged in through its endpoint (Ollama), whose login counts
/// with or without a key. Success always means the profile needs a sign-in:
/// callers refused an environment override.
fn write_sign_out(
    auth: &str,
    descriptor: Option<&'static ProviderDescriptor>,
    repository: &ConfigRepository,
) -> Result<(), CredentialWriteError> {
    ProviderAuthentication::delete_credentials(&AppCredentialStore, auth)
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
}

/// An auth profile an API key can sign in to: it takes a key, and no
/// environment variable overrides a saved one. Keys passed to its methods
/// are trimmed and never empty.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ApiKeyAuth {
    auth: &'static str,
    descriptor: &'static ProviderDescriptor,
    entry_label: &'static str,
}

impl ApiKeyAuth {
    /// Checks that `auth` can take a saved key. A custom provider's profile
    /// resolves once [`load_config_and_store`] (or startup) published it.
    pub(crate) fn resolve(auth: &str) -> Result<Self, CredentialWriteError> {
        let Some((descriptor, mode)) = provider::resolve_auth_mode(auth) else {
            return Err(CredentialWriteError::UnknownAuth { auth: auth.into() });
        };
        let ProviderAuthKind::ApiKey { entry_label, .. } = mode.auth_kind else {
            return Err(CredentialWriteError::NotAnApiKey { auth: auth.into() });
        };
        // The variable wins over a saved key, so saving one would change
        // nothing and the check would test the variable's key instead.
        refuse_environment_override(auth, &mode, CredentialWrite::SignIn)?;
        Ok(Self {
            auth: mode.id,
            descriptor,
            entry_label,
        })
    }

    pub(crate) fn auth(&self) -> &'static str {
        self.auth
    }

    pub(crate) fn descriptor(&self) -> &'static ProviderDescriptor {
        self.descriptor
    }

    /// What the provider calls the key, such as "OpenAI API key".
    pub(crate) fn entry_label(&self) -> &'static str {
        self.entry_label
    }

    /// Asks the models endpoint `config` resolves for the provider about
    /// `key`, failing only [`CredentialWriteError::KeyRefused`]. Callers
    /// save a key the provider cannot be asked about (no model listing in
    /// Rho, an outage, a gateway without `/models`), as the terminal login
    /// saves it, and say it went unchecked, so such a provider can still be
    /// signed in to.
    ///
    /// A plain `GET /models` that carries only the key fails 400, 401, or
    /// 403 only for the key itself; Google answers a malformed key with
    /// 400. Response bodies may quote the request, so only statuses show.
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
        let refused = CredentialWriteError::KeyRefused {
            provider: self.descriptor.display_name,
        };
        let status = match &error {
            ModelError::MissingCredentials(_) => return Err(refused),
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
        if matches!(status.as_u16(), 400 | 401 | 403) {
            Err(refused)
        } else {
            Ok(KeyCheck::Unreachable(format!("HTTP {status}")))
        }
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

/// `message` with any copy of `key` removed, in case an error quoted it.
fn redacted(message: String, key: &str) -> String {
    message.replace(key, "[key]")
}
