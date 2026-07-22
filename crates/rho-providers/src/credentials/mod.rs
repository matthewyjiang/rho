//! Provider credential storage backends and helpers.
//!
//! Credentials are stored under stable account names in the rho service
//! namespace. Callers choose a backend explicitly through
//! [`CredentialStoreBackend`]:
//!
//! - [`CredentialStoreBackend::Auto`] probes and opens the OS keyring only. It
//!   never falls back to local files.
//! - [`CredentialStoreBackend::Os`] always uses the OS keyring.
//! - [`CredentialStoreBackend::File`] stores secrets in private files under the
//!   Rho home directory (`RHO_HOME` or `~/.rho`). File storage is never selected
//!   implicitly; callers must request it.
//!
//! Use [`open_credential_store`] to construct a backend and
//! [`probe_credential_store`] for a non-destructive availability check.

mod backend;
mod file;
mod file_lock;
#[cfg(windows)]
mod file_windows;
mod memory;
mod os;

#[cfg(test)]
#[path = "../credentials_openrouter_tests.rs"]
mod openrouter_tests;

#[cfg(test)]
#[path = "store_tests.rs"]
mod store_tests;

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::provider::{self, ProviderAuthKind};

pub use backend::{
    open_credential_store, probe_credential_store, CredentialStoreBackend, CredentialStoreProbe,
};
pub use file::FileCredentialStore;
#[cfg(any(test, debug_assertions))]
pub use memory::MemoryCredentialStore;
pub use os::OsCredentialStore;

const CODEX_TOKENS_ACCOUNT: &str = provider::CODEX_TOKENS_ACCOUNT;
const GITHUB_COPILOT_TOKENS_ACCOUNT: &str = provider::GITHUB_COPILOT_TOKENS_ACCOUNT;
const XAI_TOKENS_ACCOUNT: &str = provider::XAI_TOKENS_ACCOUNT;
const WEB_SEARCH_OPENAI_API_KEY_ACCOUNT: &str = "web-search:openai:api-key";
const WEB_SEARCH_EXA_API_KEY_ACCOUNT: &str = "web-search:exa:api-key";
const WEB_SEARCH_BRAVE_API_KEY_ACCOUNT: &str = "web-search:brave:api-key";

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct CodexTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub account_id: Option<String>,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct GitHubCopilotTokens {
    pub github_access_token: String,
    pub github_refresh_token: Option<String>,
    pub github_expires_at_unix: Option<i64>,
    pub copilot_token: Option<String>,
    pub copilot_expires_at_unix: Option<i64>,
    pub copilot_refresh_after_unix: Option<i64>,
    pub copilot_token_endpoint: Option<String>,
    pub copilot_chat_endpoint: Option<String>,
    pub copilot_models_endpoint: Option<String>,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct KimiTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at_unix: Option<i64>,
    #[serde(default)]
    pub scope: String,
    #[serde(default = "default_bearer_token_type")]
    pub token_type: String,
    #[serde(default)]
    pub expires_in: Option<u64>,
}

fn default_bearer_token_type() -> String {
    "Bearer".into()
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct XaiTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at_unix: Option<i64>,
    pub id_token: Option<String>,
}

macro_rules! redacted_token_debug {
    ($type:ty, $($visible:ident),* $(,)?) => {
        impl fmt::Debug for $type {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                let mut debug = formatter.debug_struct(stringify!($type));
                debug.field("credentials", &"[REDACTED]");
                $(debug.field(stringify!($visible), &self.$visible);)*
                debug.finish()
            }
        }
    };
}

redacted_token_debug!(CodexTokens, account_id);
redacted_token_debug!(
    GitHubCopilotTokens,
    github_expires_at_unix,
    copilot_expires_at_unix,
    copilot_refresh_after_unix,
    copilot_token_endpoint,
    copilot_chat_endpoint,
    copilot_models_endpoint,
);
redacted_token_debug!(KimiTokens, expires_at_unix);
redacted_token_debug!(XaiTokens, expires_at_unix);

#[derive(Clone, Debug, Error)]
pub enum CredentialError {
    #[error("credential store is unavailable: {0}. Configure your OS keychain, explicitly select the file backend, or use CI/dev env overrides such as OPENAI_API_KEY, ANTHROPIC_API_KEY, MOONSHOT_API_KEY, CODEX_ACCESS_TOKEN, GITHUB_COPILOT_TOKEN, KIMI_ACCESS_TOKEN, or XAI_ACCESS_TOKEN.")]
    StoreUnavailable(String),
    #[error("stored credential data is invalid: {0}")]
    InvalidData(String),
}

pub type CredentialResult<T> = Result<T, CredentialError>;

/// Stores provider credentials under stable rho account names.
///
/// Implementors should treat the `account` argument as an opaque key within the
/// rho service namespace and return `Ok(None)` or `Ok(false)` for missing
/// entries. Backend access, permission, serialization, or availability failures
/// should be reported as `CredentialError` values instead of panicking.
pub trait CredentialStore: Send + Sync {
    fn get_secret(&self, account: &str) -> CredentialResult<Option<String>>;
    fn set_secret(&self, account: &str, secret: &str) -> CredentialResult<()>;
    fn delete_secret(&self, account: &str) -> CredentialResult<bool>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebSearchCredential {
    OpenAi,
    Exa,
    Brave,
}

impl WebSearchCredential {
    pub const ALL: [Self; 3] = [Self::OpenAi, Self::Exa, Self::Brave];

    pub const fn account(self) -> &'static str {
        match self {
            Self::OpenAi => WEB_SEARCH_OPENAI_API_KEY_ACCOUNT,
            Self::Exa => WEB_SEARCH_EXA_API_KEY_ACCOUNT,
            Self::Brave => WEB_SEARCH_BRAVE_API_KEY_ACCOUNT,
        }
    }
}

pub fn load_web_search_api_key(
    store: &dyn CredentialStore,
    credential: WebSearchCredential,
) -> CredentialResult<Option<String>> {
    store.get_secret(credential.account())
}

pub fn save_web_search_api_key(
    store: &dyn CredentialStore,
    credential: WebSearchCredential,
    key: &str,
) -> CredentialResult<()> {
    store.set_secret(credential.account(), key)
}

pub fn delete_web_search_api_key(
    store: &dyn CredentialStore,
    credential: WebSearchCredential,
) -> CredentialResult<bool> {
    store.delete_secret(credential.account())
}

pub fn load_provider_api_key(
    store: &dyn CredentialStore,
    provider: &str,
) -> CredentialResult<Option<String>> {
    let Some(auth_kind @ ProviderAuthKind::ApiKey { .. }) =
        provider::provider_descriptor(provider).map(|descriptor| descriptor.auth_kind)
    else {
        return Ok(None);
    };
    store.get_secret(
        auth_kind
            .account()
            .expect("API key provider must declare a credential account"),
    )
}

pub fn save_provider_api_key(
    store: &dyn CredentialStore,
    provider: &str,
    key: &str,
) -> CredentialResult<()> {
    let Some(auth_kind @ ProviderAuthKind::ApiKey { .. }) =
        provider::provider_descriptor(provider).map(|descriptor| descriptor.auth_kind)
    else {
        return Err(CredentialError::InvalidData(format!(
            "provider '{provider}' does not use API key credentials"
        )));
    };
    store.set_secret(
        auth_kind
            .account()
            .expect("API key provider must declare a credential account"),
        key,
    )
}

pub fn save_openrouter_oauth_key(store: &dyn CredentialStore, key: &str) -> CredentialResult<()> {
    if key.trim().is_empty() {
        return Err(CredentialError::InvalidData(
            "OpenRouter OAuth key cannot be empty".into(),
        ));
    }
    store.set_secret(provider::OPENROUTER_OAUTH_KEY_ACCOUNT, key)
}

pub fn delete_provider_credentials(
    store: &dyn CredentialStore,
    provider: &str,
) -> CredentialResult<bool> {
    let Some(auth_kind) =
        provider::provider_descriptor(provider).map(|descriptor| descriptor.auth_kind)
    else {
        return Ok(false);
    };
    let Some(account) = auth_kind.account() else {
        return Ok(false);
    };
    store.delete_secret(account)
}

pub fn load_codex_tokens(store: &dyn CredentialStore) -> CredentialResult<Option<CodexTokens>> {
    let Some(secret) = store.get_secret(CODEX_TOKENS_ACCOUNT)? else {
        return Ok(None);
    };
    serde_json::from_str(&secret).map(Some).map_err(|err| {
        CredentialError::InvalidData(format!("invalid stored Codex token JSON: {err}"))
    })
}

pub fn save_codex_tokens(
    store: &dyn CredentialStore,
    tokens: &CodexTokens,
) -> CredentialResult<()> {
    let secret = serde_json::to_string(tokens)
        .map_err(|err| CredentialError::InvalidData(format!("could not encode tokens: {err}")))?;
    store.set_secret(CODEX_TOKENS_ACCOUNT, &secret)
}

pub fn load_kimi_tokens(store: &dyn CredentialStore) -> CredentialResult<Option<KimiTokens>> {
    let Some(secret) = store.get_secret(crate::provider::KIMI_TOKENS_ACCOUNT)? else {
        return Ok(None);
    };
    serde_json::from_str(&secret).map(Some).map_err(|err| {
        CredentialError::InvalidData(format!("invalid stored Kimi token JSON: {err}"))
    })
}

pub fn save_kimi_tokens(store: &dyn CredentialStore, tokens: &KimiTokens) -> CredentialResult<()> {
    let secret = serde_json::to_string(tokens)
        .map_err(|err| CredentialError::InvalidData(format!("could not encode tokens: {err}")))?;
    store.set_secret(crate::provider::KIMI_TOKENS_ACCOUNT, &secret)
}

pub fn load_xai_tokens(store: &dyn CredentialStore) -> CredentialResult<Option<XaiTokens>> {
    let Some(secret) = store.get_secret(XAI_TOKENS_ACCOUNT)? else {
        return Ok(None);
    };
    serde_json::from_str(&secret).map(Some).map_err(|err| {
        CredentialError::InvalidData(format!("invalid stored xAI token JSON: {err}"))
    })
}

pub fn save_xai_tokens(store: &dyn CredentialStore, tokens: &XaiTokens) -> CredentialResult<()> {
    let secret = serde_json::to_string(tokens)
        .map_err(|err| CredentialError::InvalidData(format!("could not encode tokens: {err}")))?;
    store.set_secret(XAI_TOKENS_ACCOUNT, &secret)
}

pub fn load_github_copilot_tokens(
    store: &dyn CredentialStore,
) -> CredentialResult<Option<GitHubCopilotTokens>> {
    let Some(secret) = store.get_secret(GITHUB_COPILOT_TOKENS_ACCOUNT)? else {
        return Ok(None);
    };
    serde_json::from_str(&secret).map(Some).map_err(|err| {
        CredentialError::InvalidData(format!("invalid stored GitHub Copilot token JSON: {err}"))
    })
}

pub fn save_github_copilot_tokens(
    store: &dyn CredentialStore,
    tokens: &GitHubCopilotTokens,
) -> CredentialResult<()> {
    let secret = serde_json::to_string(tokens)
        .map_err(|err| CredentialError::InvalidData(format!("could not encode tokens: {err}")))?;
    store.set_secret(GITHUB_COPILOT_TOKENS_ACCOUNT, &secret)
}

pub fn provider_has_env_override(provider: &str) -> bool {
    provider_has_env_override_from(provider, |env_var| std::env::var(env_var).ok())
}

fn provider_has_env_override_from(
    provider: &str,
    env_value: impl FnOnce(&str) -> Option<String>,
) -> bool {
    let Some(descriptor) = provider::provider_descriptor(provider) else {
        return false;
    };
    let Some(env_var) = descriptor.auth_kind.env_var() else {
        return false;
    };
    env_value(env_var).is_some_and(|value| !value.trim().is_empty())
}

pub fn provider_has_stored_credentials(
    store: &dyn CredentialStore,
    provider: &str,
) -> CredentialResult<bool> {
    let Some(auth_kind) =
        provider::provider_descriptor(provider).map(|descriptor| descriptor.auth_kind)
    else {
        return Ok(false);
    };
    let Some(account) = auth_kind.account() else {
        return Ok(false);
    };
    Ok(store.get_secret(account)?.is_some())
}

pub fn provider_has_credentials(
    store: &dyn CredentialStore,
    provider: &str,
) -> CredentialResult<bool> {
    if provider_has_env_override(provider) {
        return Ok(true);
    }
    match provider::provider_descriptor(provider).map(|descriptor| descriptor.auth_kind) {
        Some(ProviderAuthKind::None) => Ok(true),
        Some(auth_kind @ ProviderAuthKind::ApiKey { .. }) => Ok(store
            .get_secret(
                auth_kind
                    .account()
                    .expect("API key provider must declare a credential account"),
            )?
            .is_some()),
        Some(ProviderAuthKind::CodexOAuth { .. }) => Ok(load_codex_tokens(store)?.is_some()),
        Some(ProviderAuthKind::GithubCopilotDevice { .. }) => {
            Ok(load_github_copilot_tokens(store)?.is_some())
        }
        Some(ProviderAuthKind::XaiOAuth { .. }) => Ok(load_xai_tokens(store)?.is_some()),
        Some(ProviderAuthKind::BearerCredential { account, .. }) => Ok(store
            .get_secret(account)?
            .is_some_and(|key| !key.trim().is_empty())),
        Some(ProviderAuthKind::KimiOAuth { .. }) => Ok(load_kimi_tokens(store)?.is_some()),
        None => Ok(false),
    }
}

pub fn available_auth_modes(store: &dyn CredentialStore) -> Vec<String> {
    provider::providers()
        .iter()
        .filter(|provider| provider_has_credentials(store, provider.name).unwrap_or(false))
        .map(|provider| provider.auth.to_string())
        .collect()
}
