//! Signing in with an API key and signing out, for hosts that act for a
//! client rather than a person at a terminal: ACP (`authenticate`, `logout`)
//! and the daemon (`rho/auth/key`, `rho/auth/logout`) (#1662).
//!
//! Each host keeps how it reaches its user (ACP's local key page, the
//! daemon's socket); this module owns what a sign-in or sign-out does, so
//! both behave the same:
//!
//! - The store written is the one config selects when the request runs (see
//!   [`open_credential_store`]), not the one the process opened at start, so
//!   a store a terminal login chose since then is the one written.
//! - A credential set in the process environment wins over a saved one, so
//!   signing in or out of its auth profile refuses before writing anything.
//! - An API key is checked against the provider's models endpoint and saved
//!   unless the provider refuses it.
//! - Every write moves the credential generation (see
//!   [`AppCredentialStore::generation`]), so the process's running sessions
//!   use the change from their next prompt.

use rho_providers::{
    auth::login_dispatch::ProviderAuthentication,
    credentials::{CredentialError, CredentialStore},
    model::{
        provider_models::{check_api_key, ProviderModelEndpoint},
        ModelError,
    },
    provider::{self, ProviderAuthKind, ProviderDescriptor},
};
use url::Url;

use super::{bootstrap, config_repository::ConfigRepository, endpoint_login};
use crate::{
    config::Config,
    credential_store::{self, AppCredentialStore},
};

/// Why the store config selects could not be opened.
#[derive(Debug, thiserror::Error)]
pub(crate) enum OpenError {
    /// Config could not be read, so which store it selects is unknown.
    #[error("could not load config: {0:#}")]
    Config(anyhow::Error),
    #[error("could not open the credential store: {0:#}")]
    Store(anyhow::Error),
}

/// Activates the credential store as `repository`'s config selects it now,
/// and returns that config. With no store chosen that is the OS store, which
/// sessions read by default.
///
/// Also publishes the config's custom providers, as a session open does:
/// their auth profiles resolve, and their models endpoints answer a key
/// check, only once published. Running sessions keep the providers they
/// were built with.
pub(crate) fn open_credential_store(repository: &ConfigRepository) -> Result<Config, OpenError> {
    let config = repository.load().map_err(OpenError::Config)?;
    config.providers.activate().map_err(OpenError::Config)?;
    let path = bootstrap::absolute_config_path(repository).map_err(OpenError::Config)?;
    credential_store::activate_for_login(&config, &path).map_err(OpenError::Store)?;
    Ok(config)
}

/// The variable that overrides `auth`'s saved credential in this process's
/// environment, when one is set.
fn environment_override(auth: &str) -> Option<&'static str> {
    let (_, mode) = provider::resolve_auth_mode(auth)?;
    let variable = mode.auth_kind.env_var()?;
    ProviderAuthentication::has_environment_override(auth).then_some(variable)
}

/// What a sign-out signs out of.
#[derive(Clone, Copy, Debug)]
pub(crate) enum SignOutTarget<'a> {
    /// The auth profile a host's config selects, on the provider it selects,
    /// as ACP `logout` signs out. The keyless profile is shared by every
    /// keyless provider, so only the config's provider says which endpoint
    /// login to forget.
    Selected { provider: &'a str, auth: &'a str },
    /// An auth profile a client named, on the provider it signs in to, as
    /// `rho/auth/logout` signs out.
    Named(&'a str),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum SignOutError {
    #[error("unknown auth profile '{auth}'")]
    UnknownAuth { auth: String },
    /// A keyless auth profile, which names no credentials and, being every
    /// keyless provider's, no provider.
    #[error("'{auth}' has no credentials to sign out of")]
    NoCredentials { auth: String },
    #[error("'{auth}' reads its credential from {variable}; unset it to sign out")]
    EnvironmentOverride {
        auth: String,
        variable: &'static str,
    },
    #[error(transparent)]
    Open(#[from] OpenError),
    #[error("could not delete {auth} credentials: {error}")]
    Delete {
        auth: String,
        error: CredentialError,
    },
    #[error("could not remove the {provider} login: {error:#}")]
    ForgetLogin {
        provider: &'static str,
        error: anyhow::Error,
    },
}

/// Signs out of `target`: deletes the auth profile's stored credentials,
/// and forgets the provider's login when it is logged in through its
/// endpoint (Ollama), whose login counts with or without a key. `repository`
/// is the config the host reads.
///
/// A credential set in the environment cannot be removed, so that refuses
/// before anything is deleted: success always means the profile needs a
/// sign-in.
pub(crate) fn sign_out(
    target: SignOutTarget<'_>,
    repository: &ConfigRepository,
) -> Result<(), SignOutError> {
    open_credential_store(repository)?;
    let (descriptor, auth) = match target {
        SignOutTarget::Selected { provider, auth } => {
            (provider::provider_descriptor(provider), auth)
        }
        SignOutTarget::Named(auth) => match provider::resolve_auth_mode(auth) {
            None => return Err(SignOutError::UnknownAuth { auth: auth.into() }),
            Some((_, mode)) if matches!(mode.auth_kind, ProviderAuthKind::None) => {
                return Err(SignOutError::NoCredentials { auth: auth.into() });
            }
            Some((descriptor, _)) => (Some(descriptor), auth),
        },
    };
    if let Some(variable) = environment_override(auth) {
        return Err(SignOutError::EnvironmentOverride {
            auth: auth.into(),
            variable,
        });
    }
    ProviderAuthentication::delete_credentials(&AppCredentialStore, auth).map_err(|error| {
        SignOutError::Delete {
            auth: auth.into(),
            error,
        }
    })?;
    if let Some(descriptor) = descriptor {
        endpoint_login::forget(repository, *descriptor).map_err(|error| {
            SignOutError::ForgetLogin {
                provider: descriptor.name,
                error,
            }
        })?;
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum KeyError {
    #[error("unknown auth profile '{auth}'")]
    UnknownAuth { auth: String },
    #[error("'{auth}' does not take an API key")]
    NotAnApiKeyAuth { auth: String },
    #[error("'{auth}' reads its key from {variable}; unset it to sign in with a saved key")]
    EnvironmentOverride {
        auth: String,
        variable: &'static str,
    },
    #[error("the key is blank")]
    Blank,
    /// The provider refused the key: 400, 401, or 403 from its models
    /// endpoint.
    #[error("{provider} refused the key")]
    Rejected { provider: &'static str },
    #[error(transparent)]
    Open(#[from] OpenError),
    /// The store refused the save. `detail` has any copy of the key removed.
    #[error("could not save the key: {detail}")]
    Save { detail: String },
    #[error("saved the key, but could not record the login: {0:#}")]
    RecordLogin(anyhow::Error),
}

/// How a saved key was checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum KeyCheck {
    /// The provider's models endpoint accepted it.
    Accepted,
    /// Rho lists no models for the provider, so it cannot ask about a key.
    Unsupported,
    /// The provider could not be asked (an outage, a gateway without
    /// `/models`): why, with any copy of the key removed.
    Unreachable(String),
}

impl KeyCheck {
    /// Why the key was saved unchecked, or `None` when the provider accepted
    /// it.
    pub(crate) fn unchecked_reason(&self) -> Option<String> {
        match self {
            Self::Accepted => None,
            Self::Unsupported => {
                Some("rho lists no models for this provider, so it cannot ask about a key".into())
            }
            Self::Unreachable(reason) => Some(reason.clone()),
        }
    }
}

/// An auth profile an API key can sign in to: it takes a key, and no
/// environment variable overrides a saved one.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ApiKeyAuth<'a> {
    auth: &'a str,
    descriptor: &'static ProviderDescriptor,
    entry_label: &'static str,
}

impl<'a> ApiKeyAuth<'a> {
    /// Checks that `auth` can take a saved key. A custom provider's profile
    /// resolves once [`open_credential_store`] (or startup) published it.
    pub(crate) fn resolve(auth: &'a str) -> Result<Self, KeyError> {
        let Some((descriptor, mode)) = provider::resolve_auth_mode(auth) else {
            return Err(KeyError::UnknownAuth { auth: auth.into() });
        };
        let ProviderAuthKind::ApiKey { entry_label, .. } = mode.auth_kind else {
            return Err(KeyError::NotAnApiKeyAuth { auth: auth.into() });
        };
        // The variable wins over a saved key, so saving one would change
        // nothing and the check would test the variable's key instead.
        if let Some(variable) = environment_override(auth) {
            return Err(KeyError::EnvironmentOverride {
                auth: auth.into(),
                variable,
            });
        }
        Ok(Self {
            auth,
            descriptor,
            entry_label,
        })
    }

    pub(crate) fn auth(&self) -> &'a str {
        self.auth
    }

    pub(crate) fn descriptor(&self) -> &'static ProviderDescriptor {
        self.descriptor
    }

    /// What the provider calls the key, such as "OpenAI API key".
    pub(crate) fn entry_label(&self) -> &'static str {
        self.entry_label
    }

    /// The API base `config` resolves for an OpenAI-compatible provider,
    /// where its models endpoint lives; `None` for a provider that owns its
    /// endpoint.
    pub(crate) fn endpoint(&self, config: &Config) -> Option<Url> {
        config.resolved_provider_endpoint(self.descriptor.name)
    }

    /// Saves `key` to `store` unless the provider refuses it. A key the
    /// provider cannot be asked about (no model listing in Rho, an outage, a
    /// gateway without `/models`) is saved as the terminal login saves it,
    /// and the result says it went unchecked, so such a provider can still
    /// be signed in to. `endpoint` is [`Self::endpoint`].
    pub(crate) async fn check_and_save(
        &self,
        key: &str,
        endpoint: Option<&Url>,
        store: &dyn CredentialStore,
    ) -> Result<KeyCheck, KeyError> {
        let key = key.trim();
        if key.is_empty() {
            return Err(KeyError::Blank);
        }
        let check = if self.descriptor.supports_model_refresh() {
            let endpoint = endpoint.map_or(
                ProviderModelEndpoint::ProviderOwned,
                ProviderModelEndpoint::OpenAiCompatible,
            );
            match check_api_key(self.auth, key, endpoint).await {
                Ok(()) => KeyCheck::Accepted,
                Err(error) => match unchecked_reason(&error) {
                    None => {
                        return Err(KeyError::Rejected {
                            provider: self.descriptor.display_name,
                        })
                    }
                    Some(reason) => KeyCheck::Unreachable(redacted(reason, key)),
                },
            }
        } else {
            KeyCheck::Unsupported
        };
        ProviderAuthentication::save_api_key(store, self.auth, key).map_err(|error| {
            KeyError::Save {
                detail: redacted(error.to_string(), key),
            }
        })?;
        Ok(check)
    }

    /// Records the login of a host logged in through its endpoint (Ollama),
    /// which counts only once recorded; other providers are left alone.
    pub(crate) fn record_login(&self, repository: &ConfigRepository) -> Result<(), KeyError> {
        endpoint_login::record(repository, *self.descriptor).map_err(KeyError::RecordLogin)
    }
}

/// Signs `auth` in with `key` for a host the key reached directly, as it
/// reaches the daemon: checks and saves it to the store `repository`'s
/// config selects now, then records an endpoint login.
pub(crate) async fn save_api_key(
    auth: &str,
    key: &str,
    repository: &ConfigRepository,
) -> Result<KeyCheck, KeyError> {
    let config = open_credential_store(repository)?;
    let login = ApiKeyAuth::resolve(auth)?;
    let endpoint = login.endpoint(&config);
    let check = login
        .check_and_save(key, endpoint.as_ref(), &AppCredentialStore)
        .await?;
    login.record_login(repository)?;
    Ok(check)
}

/// Why checking a key failed without the provider refusing it, or `None`
/// when it refused. A plain `GET /models` that carries only the key fails
/// 400, 401, or 403 only for the key itself; Google answers a malformed key
/// with 400. Response bodies may quote the request, so only statuses show.
fn unchecked_reason(error: &ModelError) -> Option<String> {
    let status = match error {
        ModelError::MissingCredentials(_) => return None,
        ModelError::HttpStatus { status, .. } => Some(*status),
        ModelError::Request(error) => match error.status() {
            Some(status) => Some(status),
            None if error.is_connect() || error.is_timeout() => {
                return Some("could not reach the provider".into());
            }
            None => return Some(error.to_string()),
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
        | ModelError::UnsupportedProvider(_) => None,
    };
    match status {
        Some(status) if matches!(status.as_u16(), 400 | 401 | 403) => None,
        Some(status) => Some(format!("HTTP {status}")),
        None => Some(error.to_string()),
    }
}

/// `message` with any copy of `key` removed, in case an error quoted it.
fn redacted(message: String, key: &str) -> String {
    message.replace(key, "[key]")
}
