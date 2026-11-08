//! Signing in with an API key and signing out, for hosts that act for a
//! client rather than a person at a terminal: ACP (`authenticate`, `logout`)
//! and the daemon (`rho/auth/key`, `rho/auth/logout`) (#1662).
//!
//! Each host keeps how it reaches its user (ACP's local key page, the
//! daemon's socket); this module owns what a sign-in or sign-out does, so
//! both behave the same:
//!
//! - A request loads config first ([`load_config`]), checks everything it
//!   can, and activates the store config selects
//!   ([`LoadedConfig::activate_store`]) only once it will write. A refused
//!   request thus leaves the process's store and credential generation as
//!   they were. The store written is the one config selects when the request
//!   runs, not the one the process opened at start, so a store a terminal
//!   login chose since then is the one written.
//! - A credential set in the process environment wins over a saved one, so
//!   signing in or out of its auth profile refuses before writing anything.
//! - An API key is checked against the provider's models endpoint and saved
//!   unless the provider refuses it.
//! - Every write moves the credential generation (see
//!   [`AppCredentialStore::generation`]), so the process's running sessions
//!   use the change from their next prompt.

use std::path::PathBuf;

use rho_providers::{
    auth::{key_entry::EnteredKey, login_dispatch::ProviderAuthentication},
    credentials::{CredentialError, CredentialStore},
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

/// Why the config, or the store it selects, could not be opened.
#[derive(Debug, thiserror::Error)]
pub(crate) enum OpenError {
    /// Config could not be read, so which store it selects is unknown.
    #[error("could not load config: {0:#}")]
    Config(anyhow::Error),
    #[error("could not open the credential store: {0:#}")]
    Store(anyhow::Error),
}

/// Config as a request found it, with its custom providers published. The
/// store it selects is not active yet; see [`Self::activate_store`].
pub(crate) struct LoadedConfig {
    config: Config,
    path: PathBuf,
}

/// Loads `repository`'s config and publishes its custom providers, as a
/// session open does: their auth profiles resolve, and their models
/// endpoints answer a key check, only once published. Running sessions keep
/// the providers they were built with.
pub(crate) fn load_config(repository: &ConfigRepository) -> Result<LoadedConfig, OpenError> {
    let config = repository.load().map_err(OpenError::Config)?;
    config.providers.activate().map_err(OpenError::Config)?;
    let path = bootstrap::absolute_config_path(repository).map_err(OpenError::Config)?;
    Ok(LoadedConfig { config, path })
}

impl LoadedConfig {
    pub(crate) fn config(&self) -> &Config {
        &self.config
    }

    /// Makes the store this config selects the one [`AppCredentialStore`]
    /// writes. With no store chosen that is the OS store, which sessions
    /// read by default. Switching stores moves the credential generation, so
    /// call this only once the request will write.
    pub(crate) fn activate_store(&self) -> Result<(), OpenError> {
        credential_store::activate_for_login(&self.config, &self.path).map_err(OpenError::Store)
    }
}

/// A variable in the process environment holds an auth profile's
/// credential, which wins over a saved one: saving or deleting one would
/// change nothing.
#[derive(Debug, thiserror::Error)]
#[error("'{auth}' reads its credential from {variable}")]
pub(crate) struct EnvironmentOverride {
    pub(crate) auth: String,
    pub(crate) variable: &'static str,
}

/// Refuses `auth`, whose mode is `mode`, when the environment overrides its
/// saved credential.
fn refuse_environment_override(auth: &str, mode: &AuthMode) -> Result<(), EnvironmentOverride> {
    match mode.auth_kind.env_var() {
        Some(variable) if ProviderAuthentication::has_environment_override(auth) => {
            Err(EnvironmentOverride {
                auth: auth.into(),
                variable,
            })
        }
        Some(_) | None => Ok(()),
    }
}

/// Why a sign-out wrote nothing, or failed partway.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SignOutError {
    #[error("{0}; unset it to sign out")]
    EnvironmentOverride(EnvironmentOverride),
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

/// Why a client cannot sign out of the auth profile it named.
#[derive(Debug, thiserror::Error)]
pub(crate) enum NamedSignOutError {
    #[error("unknown auth profile '{auth}'")]
    Unknown { auth: String },
    /// A keyless auth profile, which names no credentials and, being every
    /// keyless provider's, no provider.
    #[error("'{auth}' has no credentials to sign out of")]
    NoCredentials { auth: String },
    #[error(transparent)]
    SignOut(#[from] SignOutError),
}

/// Signs out of auth profile `auth` on `provider`, the ones a host's config
/// selects, as ACP `logout` signs out. The keyless profile is shared by
/// every keyless provider, so only `provider` says which endpoint login to
/// forget. `repository` is the config the host reads.
pub(crate) fn sign_out_selected(
    provider: &str,
    auth: &str,
    repository: &ConfigRepository,
) -> Result<(), SignOutError> {
    let loaded = load_config(repository)?;
    if let Some((_, mode)) = provider::resolve_auth_mode(auth) {
        refuse_environment_override(auth, &mode).map_err(SignOutError::EnvironmentOverride)?;
    }
    write_sign_out(
        &loaded,
        auth,
        provider::provider_descriptor(provider),
        repository,
    )
}

/// Signs out of auth profile `auth`, which a client named, on the provider
/// it signs in to, as `rho/auth/logout` signs out.
pub(crate) fn sign_out_named(
    auth: &str,
    repository: &ConfigRepository,
) -> Result<(), NamedSignOutError> {
    let loaded = load_config(repository).map_err(SignOutError::from)?;
    let Some((descriptor, mode)) = provider::resolve_auth_mode(auth) else {
        return Err(NamedSignOutError::Unknown { auth: auth.into() });
    };
    if matches!(mode.auth_kind, ProviderAuthKind::None) {
        return Err(NamedSignOutError::NoCredentials { auth: auth.into() });
    }
    refuse_environment_override(auth, &mode).map_err(SignOutError::EnvironmentOverride)?;
    Ok(write_sign_out(&loaded, auth, Some(descriptor), repository)?)
}

/// Deletes `auth`'s stored credentials from the store `loaded` selects, and
/// forgets `descriptor`'s login when it is logged in through its endpoint
/// (Ollama), whose login counts with or without a key. Success always means
/// the profile needs a sign-in: callers refused an environment override.
fn write_sign_out(
    loaded: &LoadedConfig,
    auth: &str,
    descriptor: Option<&'static ProviderDescriptor>,
    repository: &ConfigRepository,
) -> Result<(), SignOutError> {
    loaded.activate_store()?;
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

/// Why an auth profile cannot take a saved API key.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ProfileError {
    #[error("unknown auth profile '{auth}'")]
    Unknown { auth: String },
    #[error("'{auth}' does not take an API key")]
    NotAnApiKey { auth: String },
    #[error("{0}; unset it to sign in with a saved key")]
    EnvironmentOverride(EnvironmentOverride),
}

/// Why a key that passed every check before the provider's was not saved.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SaveKeyError {
    /// The provider refused the key: 400, 401, or 403 from its models
    /// endpoint.
    #[error("{provider} refused the key")]
    Rejected { provider: &'static str },
    /// The store refused the save. `detail` has any copy of the key removed.
    #[error("could not save the key: {detail}")]
    Save { detail: String },
}

/// The key is saved, but the endpoint login that makes it count was not
/// recorded.
#[derive(Debug, thiserror::Error)]
#[error("saved the key, but could not record the login: {0:#}")]
pub(crate) struct RecordLoginError(anyhow::Error);

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

/// An API key with surrounding whitespace removed; never empty.
#[derive(Clone, Copy)]
pub(crate) struct ApiKey<'a>(&'a str);

impl<'a> ApiKey<'a> {
    /// `key` trimmed, or `None` when it is blank.
    pub(crate) fn parse(key: &'a str) -> Option<Self> {
        let key = key.trim();
        (!key.is_empty()).then_some(Self(key))
    }

    /// A key entered on the key page, which trims it and never posts a
    /// blank one.
    pub(crate) fn entered(key: &'a EnteredKey) -> Self {
        Self(key.expose())
    }
}

/// An auth profile an API key can sign in to: it takes a key, and no
/// environment variable overrides a saved one.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ApiKeyAuth {
    auth: &'static str,
    descriptor: &'static ProviderDescriptor,
    entry_label: &'static str,
}

impl ApiKeyAuth {
    /// Checks that `auth` can take a saved key. A custom provider's profile
    /// resolves once [`load_config`] (or startup) published it.
    pub(crate) fn resolve(auth: &str) -> Result<Self, ProfileError> {
        let Some((descriptor, mode)) = provider::resolve_auth_mode(auth) else {
            return Err(ProfileError::Unknown { auth: auth.into() });
        };
        let ProviderAuthKind::ApiKey { entry_label, .. } = mode.auth_kind else {
            return Err(ProfileError::NotAnApiKey { auth: auth.into() });
        };
        // The variable wins over a saved key, so saving one would change
        // nothing and the check would test the variable's key instead.
        refuse_environment_override(auth, &mode).map_err(ProfileError::EnvironmentOverride)?;
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

    /// Saves `key` to `store` unless the provider refuses it, asking the
    /// models endpoint `config` resolves for the provider. A key the
    /// provider cannot be asked about (no model listing in Rho, an outage, a
    /// gateway without `/models`) is saved as the terminal login saves it,
    /// and the result says it went unchecked, so such a provider can still
    /// be signed in to.
    pub(crate) async fn check_and_save(
        &self,
        key: ApiKey<'_>,
        config: &Config,
        store: &dyn CredentialStore,
    ) -> Result<KeyCheck, SaveKeyError> {
        let check = self.check(key, config).await?;
        self.save(key, store)?;
        Ok(check)
    }

    /// Asks the provider about `key`; fails only [`SaveKeyError::Rejected`].
    async fn check(
        &self,
        ApiKey(key): ApiKey<'_>,
        config: &Config,
    ) -> Result<KeyCheck, SaveKeyError> {
        if !self.descriptor.supports_model_refresh() {
            return Ok(KeyCheck::NoModelListing);
        }
        let base = config.resolved_provider_endpoint(self.descriptor.name);
        let endpoint = base.as_ref().map_or(
            ProviderModelEndpoint::ProviderOwned,
            ProviderModelEndpoint::OpenAiCompatible,
        );
        match check_api_key(self.auth, key, endpoint).await {
            Ok(()) => Ok(KeyCheck::Accepted),
            Err(error) => match check_failure(&error) {
                CheckFailure::Refused => Err(SaveKeyError::Rejected {
                    provider: self.descriptor.display_name,
                }),
                CheckFailure::Unreachable(reason) => {
                    Ok(KeyCheck::Unreachable(redacted(reason, key)))
                }
            },
        }
    }

    /// Saves `key` to `store`; fails only [`SaveKeyError::Save`].
    fn save(
        &self,
        ApiKey(key): ApiKey<'_>,
        store: &dyn CredentialStore,
    ) -> Result<(), SaveKeyError> {
        ProviderAuthentication::save_api_key(store, self.auth, key).map_err(|error| {
            SaveKeyError::Save {
                detail: redacted(error.to_string(), key),
            }
        })
    }

    /// Records the login of a host logged in through its endpoint (Ollama),
    /// which counts only once recorded; other providers are left alone.
    pub(crate) fn record_login(
        &self,
        repository: &ConfigRepository,
    ) -> Result<(), RecordLoginError> {
        endpoint_login::record(repository, *self.descriptor).map_err(RecordLoginError)
    }
}

/// Why a key sent with the request was not saved.
#[derive(Debug, thiserror::Error)]
pub(crate) enum KeyError {
    #[error(transparent)]
    Profile(#[from] ProfileError),
    #[error("the key is blank")]
    Blank,
    #[error(transparent)]
    Open(#[from] OpenError),
    #[error(transparent)]
    Save(#[from] SaveKeyError),
    #[error(transparent)]
    RecordLogin(#[from] RecordLoginError),
}

/// A key sent with a request, as the daemon's `rho/auth/key` receives it,
/// that passed every check before the provider's. The steps are separate so
/// a host can run the blocking ones (config, the store) off its async
/// workers: [`Self::prepare`], then [`Self::check`], then [`Self::save`].
pub(crate) struct KeySave {
    loaded: LoadedConfig,
    login: ApiKeyAuth,
    /// Trimmed and never empty.
    key: String,
    repository: ConfigRepository,
}

impl KeySave {
    /// Loads `repository`'s config and checks that `auth` takes `key`.
    /// Blocks on config.
    pub(crate) fn prepare(
        auth: &str,
        key: &str,
        repository: ConfigRepository,
    ) -> Result<Self, KeyError> {
        let loaded = load_config(&repository)?;
        let login = ApiKeyAuth::resolve(auth)?;
        let ApiKey(key) = ApiKey::parse(key).ok_or(KeyError::Blank)?;
        Ok(Self {
            loaded,
            login,
            key: key.to_owned(),
            repository,
        })
    }

    pub(crate) fn auth(&self) -> &'static str {
        self.login.auth()
    }

    /// Asks the provider about the key, failing when it refuses it.
    pub(crate) async fn check(&self) -> Result<KeyCheck, KeyError> {
        Ok(self
            .login
            .check(ApiKey(&self.key), self.loaded.config())
            .await?)
    }

    /// Saves the key to the store config selects, activating that store,
    /// then records an endpoint login. Blocks on the store and config.
    pub(crate) fn save(self) -> Result<(), KeyError> {
        self.loaded.activate_store()?;
        self.login.save(ApiKey(&self.key), &AppCredentialStore)?;
        self.login.record_login(&self.repository)?;
        Ok(())
    }
}

/// How checking a key failed.
enum CheckFailure {
    /// The provider refused the key.
    Refused,
    /// The provider could not be asked: why.
    Unreachable(String),
}

/// A plain `GET /models` that carries only the key fails 400, 401, or 403
/// only for the key itself; Google answers a malformed key with 400.
/// Response bodies may quote the request, so only statuses show.
fn check_failure(error: &ModelError) -> CheckFailure {
    let status = match error {
        ModelError::MissingCredentials(_) => return CheckFailure::Refused,
        ModelError::HttpStatus { status, .. } => *status,
        ModelError::Request(error) => match error.status() {
            Some(status) => status,
            // A transport error's text may quote the URL, where a key can
            // sit percent-encoded beyond redaction's reach, so it never
            // shows (#1662).
            None if error.is_connect() || error.is_timeout() => {
                return CheckFailure::Unreachable("could not reach the provider".into());
            }
            None => return CheckFailure::Unreachable("the request to the provider failed".into()),
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
            return CheckFailure::Unreachable(error.to_string());
        }
    };
    if matches!(status.as_u16(), 400 | 401 | 403) {
        CheckFailure::Refused
    } else {
        CheckFailure::Unreachable(format!("HTTP {status}"))
    }
}

/// `message` with any copy of `key` removed, in case an error quoted it.
fn redacted(message: String, key: &str) -> String {
    message.replace(key, "[key]")
}
