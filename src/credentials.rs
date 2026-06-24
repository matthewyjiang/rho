#[cfg(test)]
use std::{collections::HashMap, sync::Mutex};

use serde::{Deserialize, Serialize};
use thiserror::Error;

const SERVICE: &str = "rho";
const OPENAI_API_KEY_ACCOUNT: &str = "provider:openai:api-key";
const CODEX_TOKENS_ACCOUNT: &str = "provider:openai-codex:tokens";
const CHUNK_MANIFEST_SUFFIX: &str = ":chunks";
const CHUNK_ACCOUNT_INFIX: &str = ":chunk:";

#[cfg(windows)]
const MAX_SECRET_CHUNK_CHARS: usize = 2000;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CodexTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub account_id: Option<String>,
}

#[derive(Clone, Debug, Error)]
pub enum CredentialError {
    #[error("OS credential store is unavailable: {0}. Configure your OS keychain, or use CI/dev env overrides such as OPENAI_API_KEY or CODEX_ACCESS_TOKEN.")]
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

#[derive(Clone, Debug, Default)]
pub struct OsCredentialStore;

impl OsCredentialStore {
    fn entry(account: &str) -> CredentialResult<keyring::Entry> {
        keyring::Entry::new(SERVICE, account)
            .map_err(|err| CredentialError::StoreUnavailable(err.to_string()))
    }

    fn get_entry_secret(account: &str) -> CredentialResult<Option<String>> {
        match Self::entry(account)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(CredentialError::StoreUnavailable(err.to_string())),
        }
    }

    fn set_entry_secret(account: &str, secret: &str) -> CredentialResult<()> {
        Self::entry(account)?
            .set_password(secret)
            .map_err(|err| CredentialError::StoreUnavailable(err.to_string()))
    }

    fn delete_entry_secret(account: &str) -> CredentialResult<bool> {
        match Self::entry(account)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(err) => Err(CredentialError::StoreUnavailable(err.to_string())),
        }
    }

    fn chunk_manifest_account(account: &str) -> String {
        format!("{account}{CHUNK_MANIFEST_SUFFIX}")
    }

    fn chunk_account(account: &str, index: usize) -> String {
        format!("{account}{CHUNK_ACCOUNT_INFIX}{index}")
    }

    fn load_chunked_secret(account: &str) -> CredentialResult<Option<String>> {
        let Some(manifest) = Self::get_entry_secret(&Self::chunk_manifest_account(account))? else {
            return Ok(None);
        };
        let chunk_count = manifest.parse::<usize>().map_err(|err| {
            CredentialError::InvalidData(format!("invalid credential chunk manifest: {err}"))
        })?;
        let mut secret = String::new();
        for index in 0..chunk_count {
            let chunk =
                Self::get_entry_secret(&Self::chunk_account(account, index))?.ok_or_else(|| {
                    CredentialError::InvalidData(format!("missing credential chunk {index}"))
                })?;
            secret.push_str(&chunk);
        }
        Ok(Some(secret))
    }

    fn set_chunked_secret(account: &str, secret: &str) -> CredentialResult<()> {
        let chunks = chunk_secret(secret);
        for (index, chunk) in chunks.iter().enumerate() {
            Self::set_entry_secret(&Self::chunk_account(account, index), chunk)?;
        }
        Self::set_entry_secret(
            &Self::chunk_manifest_account(account),
            &chunks.len().to_string(),
        )?;
        let _ = Self::delete_entry_secret(account);
        Ok(())
    }

    fn delete_chunked_secret(account: &str) -> CredentialResult<bool> {
        let Some(manifest) = Self::get_entry_secret(&Self::chunk_manifest_account(account))? else {
            return Ok(false);
        };
        let chunk_count = manifest.parse::<usize>().map_err(|err| {
            CredentialError::InvalidData(format!("invalid credential chunk manifest: {err}"))
        })?;
        let mut deleted = Self::delete_entry_secret(&Self::chunk_manifest_account(account))?;
        for index in 0..chunk_count {
            deleted |= Self::delete_entry_secret(&Self::chunk_account(account, index))?;
        }
        Ok(deleted)
    }
}

impl CredentialStore for OsCredentialStore {
    fn get_secret(&self, account: &str) -> CredentialResult<Option<String>> {
        match Self::get_entry_secret(account)? {
            Some(secret) => Ok(Some(secret)),
            None => Self::load_chunked_secret(account),
        }
    }

    fn set_secret(&self, account: &str, secret: &str) -> CredentialResult<()> {
        Self::delete_chunked_secret(account)?;
        if should_chunk_secret(secret) {
            return Self::set_chunked_secret(account, secret);
        }
        match Self::set_entry_secret(account, secret) {
            Ok(()) => Ok(()),
            Err(err) => {
                if should_retry_as_chunked(&err) {
                    Self::set_chunked_secret(account, secret)
                } else {
                    Err(err)
                }
            }
        }
    }

    fn delete_secret(&self, account: &str) -> CredentialResult<bool> {
        Ok(Self::delete_entry_secret(account)? | Self::delete_chunked_secret(account)?)
    }
}

fn chunk_secret(secret: &str) -> Vec<String> {
    chunk_secret_with_max(secret, max_secret_chunk_chars())
}

fn chunk_secret_with_max(secret: &str, max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_len = 0;
    for ch in secret.chars() {
        if current_len == max_chars {
            chunks.push(current);
            current = String::new();
            current_len = 0;
        }
        current.push(ch);
        current_len += 1;
    }
    chunks.push(current);
    chunks
}

fn should_chunk_secret(secret: &str) -> bool {
    secret.chars().count() > max_secret_chunk_chars()
}

fn should_retry_as_chunked(err: &CredentialError) -> bool {
    cfg!(windows) && err.to_string().contains("platform limit")
}

fn max_secret_chunk_chars() -> usize {
    #[cfg(windows)]
    {
        MAX_SECRET_CHUNK_CHARS
    }
    #[cfg(not(windows))]
    {
        usize::MAX
    }
}

#[cfg(test)]
#[derive(Debug, Default)]
pub struct MemoryCredentialStore {
    secrets: Mutex<HashMap<String, String>>,
}

#[cfg(test)]
impl CredentialStore for MemoryCredentialStore {
    fn get_secret(&self, account: &str) -> CredentialResult<Option<String>> {
        Ok(self.secrets.lock().unwrap().get(account).cloned())
    }

    fn set_secret(&self, account: &str, secret: &str) -> CredentialResult<()> {
        self.secrets
            .lock()
            .unwrap()
            .insert(account.to_string(), secret.to_string());
        Ok(())
    }

    fn delete_secret(&self, account: &str) -> CredentialResult<bool> {
        Ok(self.secrets.lock().unwrap().remove(account).is_some())
    }
}

pub fn load_openai_api_key(store: &dyn CredentialStore) -> CredentialResult<Option<String>> {
    store.get_secret(OPENAI_API_KEY_ACCOUNT)
}

pub fn save_openai_api_key(store: &dyn CredentialStore, key: &str) -> CredentialResult<()> {
    store.set_secret(OPENAI_API_KEY_ACCOUNT, key)
}

pub fn delete_openai_api_key(store: &dyn CredentialStore) -> CredentialResult<bool> {
    store.delete_secret(OPENAI_API_KEY_ACCOUNT)
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

pub fn delete_codex_tokens(store: &dyn CredentialStore) -> CredentialResult<bool> {
    store.delete_secret(CODEX_TOKENS_ACCOUNT)
}

pub fn provider_has_env_override(provider: &str) -> bool {
    match provider {
        "openai" => std::env::var_os("OPENAI_API_KEY").is_some(),
        "openai-codex" => std::env::var_os("CODEX_ACCESS_TOKEN").is_some(),
        _ => false,
    }
}

pub fn provider_has_credentials(
    store: &dyn CredentialStore,
    provider: &str,
) -> CredentialResult<bool> {
    if provider_has_env_override(provider) {
        return Ok(true);
    }
    match provider {
        "openai" => Ok(load_openai_api_key(store)?.is_some()),
        "openai-codex" => Ok(load_codex_tokens(store)?.is_some()),
        _ => Ok(false),
    }
}

pub fn available_auth_modes(store: &dyn CredentialStore) -> Vec<String> {
    let mut modes = Vec::new();
    if provider_has_credentials(store, "openai").unwrap_or(false) {
        modes.push("api-key".to_string());
    }
    if provider_has_credentials(store, "openai-codex").unwrap_or(false) {
        modes.push("codex".to_string());
    }
    modes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_key_round_trips_through_memory_store() {
        let store = MemoryCredentialStore::default();

        assert_eq!(load_openai_api_key(&store).unwrap(), None);
        save_openai_api_key(&store, "sk-test").unwrap();
        assert_eq!(load_openai_api_key(&store).unwrap(), Some("sk-test".into()));
        assert!(delete_openai_api_key(&store).unwrap());
        assert_eq!(load_openai_api_key(&store).unwrap(), None);
    }

    #[test]
    fn codex_tokens_round_trip_with_optional_fields() {
        let store = MemoryCredentialStore::default();
        let tokens = CodexTokens {
            access_token: "access".into(),
            refresh_token: Some("refresh".into()),
            id_token: Some("id".into()),
            account_id: Some("account".into()),
        };

        save_codex_tokens(&store, &tokens).unwrap();

        assert_eq!(load_codex_tokens(&store).unwrap(), Some(tokens));
    }

    #[test]
    fn codex_tokens_allow_missing_optional_fields() {
        let store = MemoryCredentialStore::default();
        store
            .set_secret(CODEX_TOKENS_ACCOUNT, r#"{"access_token":"access"}"#)
            .unwrap();

        assert_eq!(
            load_codex_tokens(&store).unwrap(),
            Some(CodexTokens {
                access_token: "access".into(),
                refresh_token: None,
                id_token: None,
                account_id: None,
            })
        );
    }

    #[test]
    fn credential_account_names_are_stable() {
        assert_eq!(SERVICE, "rho");
        assert_eq!(OPENAI_API_KEY_ACCOUNT, "provider:openai:api-key");
        assert_eq!(CODEX_TOKENS_ACCOUNT, "provider:openai-codex:tokens");
    }

    #[test]
    fn chunk_secret_preserves_unicode_boundaries() {
        let chunks = chunk_secret_with_max("ab🙂cd", 2);

        assert_eq!(chunks, vec!["ab", "🙂c", "d"]);
        assert_eq!(chunks.concat(), "ab🙂cd");
    }

    #[test]
    fn chunk_account_names_are_derived_from_stable_base_account() {
        assert_eq!(
            OsCredentialStore::chunk_manifest_account(CODEX_TOKENS_ACCOUNT),
            "provider:openai-codex:tokens:chunks"
        );
        assert_eq!(
            OsCredentialStore::chunk_account(CODEX_TOKENS_ACCOUNT, 3),
            "provider:openai-codex:tokens:chunk:3"
        );
    }
}
