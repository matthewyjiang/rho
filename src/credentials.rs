use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::model::registry::{self, ProviderAuthKind};

const SERVICE: &str = "rho";
#[cfg(test)]
const OPENAI_API_KEY_ACCOUNT: &str = "provider:openai:api-key";
#[cfg(test)]
const ANTHROPIC_API_KEY_ACCOUNT: &str = "provider:anthropic:api-key";
const CODEX_TOKENS_ACCOUNT: &str = "provider:openai-codex:tokens";
const GITHUB_COPILOT_TOKENS_ACCOUNT: &str = "provider:github-copilot:tokens";
const CHUNK_MANIFEST_SUFFIX: &str = ":chunks";
const CHUNK_ACCOUNT_INFIX: &str = ":chunk:";

#[cfg(windows)]
const MAX_SECRET_CHUNK_UTF16_UNITS: usize = 1000;

const CHUNK_MANIFEST_VERSION: &str = "v2";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CodexTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub account_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
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

#[derive(Clone, Debug, Error)]
pub enum CredentialError {
    #[error("OS credential store is unavailable: {0}. Configure your OS keychain, or use CI/dev env overrides such as OPENAI_API_KEY, ANTHROPIC_API_KEY, CODEX_ACCESS_TOKEN, or GITHUB_COPILOT_TOKEN.")]
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
    fn secret_cache() -> &'static Mutex<HashMap<String, Option<String>>> {
        static CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
        CACHE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn cached_secret(account: &str) -> Option<Option<String>> {
        Self::secret_cache().lock().ok()?.get(account).cloned()
    }

    fn remember_secret(account: &str, secret: Option<String>) {
        if let Ok(mut cache) = Self::secret_cache().lock() {
            cache.insert(account.to_string(), secret);
        }
    }

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

    fn chunk_batch_account(account: &str, batch_id: &str, index: usize) -> String {
        format!("{account}{CHUNK_ACCOUNT_INFIX}{batch_id}:{index}")
    }

    fn load_chunked_secret(account: &str) -> CredentialResult<Option<String>> {
        let Some(manifest) = Self::get_entry_secret(&Self::chunk_manifest_account(account))? else {
            return Ok(None);
        };
        let chunk_set = ChunkSet::parse(&manifest)?;
        let mut secret = String::new();
        for index in 0..chunk_set.count() {
            let chunk = Self::get_entry_secret(&chunk_set.account_name(account, index))?
                .ok_or_else(|| {
                    CredentialError::InvalidData(format!("missing credential chunk {index}"))
                })?;
            secret.push_str(&chunk);
        }
        Ok(Some(secret))
    }

    fn set_chunked_secret(account: &str, secret: &str) -> CredentialResult<()> {
        let old_manifest = Self::get_entry_secret(&Self::chunk_manifest_account(account))?;
        let chunks = chunk_secret(secret);
        let batch_id = new_chunk_batch_id();
        let new_manifest = ChunkSet::Current {
            batch_id: batch_id.clone(),
            count: chunks.len(),
        }
        .manifest();
        let written_accounts = write_chunk_batch(account, &batch_id, &chunks)?;

        if let Err(err) =
            Self::set_entry_secret(&Self::chunk_manifest_account(account), &new_manifest)
        {
            cleanup_accounts(&written_accounts);
            return Err(err);
        }

        match Self::load_chunked_secret(account) {
            Ok(Some(saved_secret)) if saved_secret == secret => {}
            Ok(Some(_)) => {
                restore_chunk_manifest(account, old_manifest.as_deref(), &written_accounts)?;
                return Err(CredentialError::InvalidData(
                    "saved credential chunks did not round trip".to_string(),
                ));
            }
            Ok(None) => {
                restore_chunk_manifest(account, old_manifest.as_deref(), &written_accounts)?;
                return Err(CredentialError::InvalidData(
                    "saved credential chunk manifest was not readable".to_string(),
                ));
            }
            Err(err) => {
                restore_chunk_manifest(account, old_manifest.as_deref(), &written_accounts)?;
                return Err(err);
            }
        }

        Self::delete_entry_secret(account)?;
        if let Some(manifest) = old_manifest {
            delete_chunks_for_manifest(account, &manifest)?;
        }
        Ok(())
    }

    fn delete_chunked_secret(account: &str) -> CredentialResult<bool> {
        let manifest_account = Self::chunk_manifest_account(account);
        let Some(manifest) = Self::get_entry_secret(&manifest_account)? else {
            return Ok(false);
        };
        let mut deleted = delete_chunks_for_manifest(account, &manifest)?;
        deleted |= Self::delete_entry_secret(&manifest_account)?;
        Ok(deleted)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ChunkSet {
    Legacy { count: usize },
    Current { batch_id: String, count: usize },
}

impl ChunkSet {
    fn parse(manifest: &str) -> CredentialResult<Self> {
        if let Some(rest) = manifest
            .strip_prefix(CHUNK_MANIFEST_VERSION)
            .and_then(|rest| rest.strip_prefix(':'))
        {
            let (batch_id, count) = rest.rsplit_once(':').ok_or_else(|| {
                CredentialError::InvalidData("invalid credential chunk manifest".to_string())
            })?;
            if batch_id.is_empty() {
                return Err(CredentialError::InvalidData(
                    "invalid credential chunk manifest".to_string(),
                ));
            }
            let count = parse_chunk_count(count)?;
            return Ok(Self::Current {
                batch_id: batch_id.to_string(),
                count,
            });
        }

        Ok(Self::Legacy {
            count: parse_chunk_count(manifest)?,
        })
    }

    fn manifest(&self) -> String {
        match self {
            Self::Legacy { count } => count.to_string(),
            Self::Current { batch_id, count } => {
                format!("{CHUNK_MANIFEST_VERSION}:{batch_id}:{count}")
            }
        }
    }

    fn count(&self) -> usize {
        match self {
            Self::Legacy { count } | Self::Current { count, batch_id: _ } => *count,
        }
    }

    fn account_name(&self, account: &str, index: usize) -> String {
        match self {
            Self::Legacy { count: _ } => OsCredentialStore::chunk_account(account, index),
            Self::Current { batch_id, count: _ } => {
                OsCredentialStore::chunk_batch_account(account, batch_id, index)
            }
        }
    }
}

fn parse_chunk_count(count: &str) -> CredentialResult<usize> {
    count.parse::<usize>().map_err(|err| {
        CredentialError::InvalidData(format!("invalid credential chunk manifest: {err}"))
    })
}

fn new_chunk_batch_id() -> String {
    let process_id = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{process_id:x}{nanos:x}")
}

fn write_chunk_batch(
    account: &str,
    batch_id: &str,
    chunks: &[String],
) -> CredentialResult<Vec<String>> {
    let mut written_accounts = Vec::new();
    for (index, chunk) in chunks.iter().enumerate() {
        let chunk_account = OsCredentialStore::chunk_batch_account(account, batch_id, index);
        if let Err(err) = OsCredentialStore::set_entry_secret(&chunk_account, chunk) {
            cleanup_accounts(&written_accounts);
            return Err(err);
        }
        written_accounts.push(chunk_account);
    }
    Ok(written_accounts)
}

fn restore_chunk_manifest(
    account: &str,
    old_manifest: Option<&str>,
    new_chunk_accounts: &[String],
) -> CredentialResult<()> {
    let result = match old_manifest {
        Some(manifest) => OsCredentialStore::set_entry_secret(
            &OsCredentialStore::chunk_manifest_account(account),
            manifest,
        ),
        None => OsCredentialStore::delete_entry_secret(&OsCredentialStore::chunk_manifest_account(
            account,
        ))
        .map(|_| ()),
    };
    cleanup_accounts(new_chunk_accounts);
    result
}

fn cleanup_accounts(accounts: &[String]) {
    for account in accounts {
        let _ = OsCredentialStore::delete_entry_secret(account);
    }
}

fn delete_chunks_for_manifest(account: &str, manifest: &str) -> CredentialResult<bool> {
    let chunk_set = ChunkSet::parse(manifest)?;
    let mut deleted = false;
    for index in 0..chunk_set.count() {
        deleted |= OsCredentialStore::delete_entry_secret(&chunk_set.account_name(account, index))?;
    }
    Ok(deleted)
}

impl CredentialStore for OsCredentialStore {
    fn get_secret(&self, account: &str) -> CredentialResult<Option<String>> {
        if let Some(secret) = Self::cached_secret(account) {
            return Ok(secret);
        }

        let secret = match Self::load_chunked_secret(account)? {
            Some(secret) => Some(secret),
            None => Self::get_entry_secret(account)?,
        };
        Self::remember_secret(account, secret.clone());
        Ok(secret)
    }

    fn set_secret(&self, account: &str, secret: &str) -> CredentialResult<()> {
        let result = if should_chunk_secret(secret) {
            Self::set_chunked_secret(account, secret)
        } else {
            match Self::set_entry_secret(account, secret) {
                Ok(()) => {
                    Self::delete_chunked_secret(account)?;
                    Ok(())
                }
                Err(err) => {
                    if should_retry_as_chunked(&err) {
                        Self::set_chunked_secret(account, secret)
                    } else {
                        Err(err)
                    }
                }
            }
        };
        if result.is_ok() {
            Self::remember_secret(account, Some(secret.to_string()));
        }
        result
    }

    fn delete_secret(&self, account: &str) -> CredentialResult<bool> {
        let deleted = Self::delete_entry_secret(account)? | Self::delete_chunked_secret(account)?;
        Self::remember_secret(account, None);
        Ok(deleted)
    }
}

fn chunk_secret(secret: &str) -> Vec<String> {
    chunk_secret_with_max(secret, max_secret_chunk_utf16_units())
}

fn chunk_secret_with_max(secret: &str, max_utf16_units: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_len = 0;
    for ch in secret.chars() {
        let char_len = ch.len_utf16();
        if current_len > 0 && current_len + char_len > max_utf16_units {
            chunks.push(current);
            current = String::new();
            current_len = 0;
        }
        current.push(ch);
        current_len += char_len;
    }
    chunks.push(current);
    chunks
}

fn should_chunk_secret(secret: &str) -> bool {
    secret.encode_utf16().count() > max_secret_chunk_utf16_units()
}

fn should_retry_as_chunked(err: &CredentialError) -> bool {
    cfg!(windows) && err.to_string().contains("platform limit")
}

fn max_secret_chunk_utf16_units() -> usize {
    #[cfg(windows)]
    {
        MAX_SECRET_CHUNK_UTF16_UNITS
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

pub fn load_provider_api_key(
    store: &dyn CredentialStore,
    provider: &str,
) -> CredentialResult<Option<String>> {
    let Some(ProviderAuthKind::ApiKey { account, .. }) =
        registry::provider_descriptor(provider).map(|descriptor| descriptor.auth_kind)
    else {
        return Ok(None);
    };
    store.get_secret(account)
}

pub fn save_provider_api_key(
    store: &dyn CredentialStore,
    provider: &str,
    key: &str,
) -> CredentialResult<()> {
    let Some(ProviderAuthKind::ApiKey { account, .. }) =
        registry::provider_descriptor(provider).map(|descriptor| descriptor.auth_kind)
    else {
        return Err(CredentialError::InvalidData(format!(
            "provider '{provider}' does not use API key credentials"
        )));
    };
    store.set_secret(account, key)
}

pub fn delete_provider_credentials(
    store: &dyn CredentialStore,
    provider: &str,
) -> CredentialResult<bool> {
    match registry::provider_descriptor(provider).map(|descriptor| descriptor.auth_kind) {
        Some(ProviderAuthKind::ApiKey { account, .. }) => store.delete_secret(account),
        Some(ProviderAuthKind::CodexOAuth { account, .. }) => store.delete_secret(account),
        Some(ProviderAuthKind::GithubCopilotDevice { account, .. }) => store.delete_secret(account),
        None => Ok(false),
    }
}

#[allow(dead_code)]
pub fn load_openai_api_key(store: &dyn CredentialStore) -> CredentialResult<Option<String>> {
    load_provider_api_key(store, "openai")
}

#[allow(dead_code)]
pub fn save_openai_api_key(store: &dyn CredentialStore, key: &str) -> CredentialResult<()> {
    save_provider_api_key(store, "openai", key)
}

#[allow(dead_code)]
pub fn delete_openai_api_key(store: &dyn CredentialStore) -> CredentialResult<bool> {
    delete_provider_credentials(store, "openai")
}

#[allow(dead_code)]
pub fn load_anthropic_api_key(store: &dyn CredentialStore) -> CredentialResult<Option<String>> {
    load_provider_api_key(store, "anthropic")
}

#[allow(dead_code)]
pub fn save_anthropic_api_key(store: &dyn CredentialStore, key: &str) -> CredentialResult<()> {
    save_provider_api_key(store, "anthropic", key)
}

#[allow(dead_code)]
pub fn delete_anthropic_api_key(store: &dyn CredentialStore) -> CredentialResult<bool> {
    delete_provider_credentials(store, "anthropic")
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

#[allow(dead_code)]
pub fn delete_codex_tokens(store: &dyn CredentialStore) -> CredentialResult<bool> {
    store.delete_secret(CODEX_TOKENS_ACCOUNT)
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

#[allow(dead_code)]
pub fn delete_github_copilot_tokens(store: &dyn CredentialStore) -> CredentialResult<bool> {
    store.delete_secret(GITHUB_COPILOT_TOKENS_ACCOUNT)
}

pub fn provider_has_env_override(provider: &str) -> bool {
    provider_has_env_override_from(provider, |env_var| std::env::var(env_var).ok())
}

fn provider_has_env_override_from(
    provider: &str,
    env_value: impl FnOnce(&str) -> Option<String>,
) -> bool {
    let Some(descriptor) = registry::provider_descriptor(provider) else {
        return false;
    };
    let env_var = match descriptor.auth_kind {
        ProviderAuthKind::ApiKey { env_var, .. }
        | ProviderAuthKind::CodexOAuth { env_var, .. }
        | ProviderAuthKind::GithubCopilotDevice { env_var, .. } => env_var,
    };
    env_value(env_var).is_some_and(|value| !value.trim().is_empty())
}

pub fn provider_has_stored_credentials(
    store: &dyn CredentialStore,
    provider: &str,
) -> CredentialResult<bool> {
    match registry::provider_descriptor(provider).map(|descriptor| descriptor.auth_kind) {
        Some(ProviderAuthKind::ApiKey { account, .. })
        | Some(ProviderAuthKind::CodexOAuth { account, .. })
        | Some(ProviderAuthKind::GithubCopilotDevice { account, .. }) => {
            Ok(store.get_secret(account)?.is_some())
        }
        None => Ok(false),
    }
}

pub fn provider_has_credentials(
    store: &dyn CredentialStore,
    provider: &str,
) -> CredentialResult<bool> {
    if provider_has_env_override(provider) {
        return Ok(true);
    }
    match registry::provider_descriptor(provider).map(|descriptor| descriptor.auth_kind) {
        Some(ProviderAuthKind::ApiKey { account, .. }) => Ok(store.get_secret(account)?.is_some()),
        Some(ProviderAuthKind::CodexOAuth { .. }) => Ok(load_codex_tokens(store)?.is_some()),
        Some(ProviderAuthKind::GithubCopilotDevice { .. }) => {
            Ok(load_github_copilot_tokens(store)?.is_some())
        }
        None => Ok(false),
    }
}

pub fn available_auth_modes(store: &dyn CredentialStore) -> Vec<String> {
    registry::providers()
        .iter()
        .filter(|provider| provider_has_credentials(store, provider.name).unwrap_or(false))
        .map(|provider| provider.auth.to_string())
        .collect()
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
    fn anthropic_api_key_round_trips_through_memory_store() {
        let store = MemoryCredentialStore::default();

        assert_eq!(load_anthropic_api_key(&store).unwrap(), None);
        save_anthropic_api_key(&store, "sk-ant-test").unwrap();
        assert_eq!(
            load_anthropic_api_key(&store).unwrap(),
            Some("sk-ant-test".into())
        );
        assert!(delete_anthropic_api_key(&store).unwrap());
        assert_eq!(load_anthropic_api_key(&store).unwrap(), None);
    }

    #[test]
    fn available_auth_modes_includes_anthropic_api_key() {
        let store = MemoryCredentialStore::default();
        save_anthropic_api_key(&store, "sk-ant-test").unwrap();

        assert!(available_auth_modes(&store).contains(&"anthropic-api-key".into()));
    }

    #[test]
    fn malformed_oauth_tokens_are_not_available_auth() {
        let store = MemoryCredentialStore::default();
        store.set_secret(CODEX_TOKENS_ACCOUNT, "not-json").unwrap();
        store
            .set_secret(GITHUB_COPILOT_TOKENS_ACCOUNT, "not-json")
            .unwrap();

        assert!(provider_has_stored_credentials(&store, "openai-codex").unwrap());
        assert!(provider_has_stored_credentials(&store, "github-copilot").unwrap());
        assert!(provider_has_credentials(&store, "openai-codex").is_err());
        assert!(provider_has_credentials(&store, "github-copilot").is_err());
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
    fn github_copilot_tokens_round_trip_with_cached_token_fields() {
        let store = MemoryCredentialStore::default();
        let tokens = GitHubCopilotTokens {
            github_access_token: "github-access".into(),
            github_refresh_token: Some("github-refresh".into()),
            github_expires_at_unix: Some(2_500),
            copilot_token: Some("copilot".into()),
            copilot_expires_at_unix: Some(2_000),
            copilot_refresh_after_unix: Some(1_500),
            copilot_token_endpoint: Some("https://api.github.com/copilot_internal/v2/token".into()),
            copilot_chat_endpoint: Some("https://api.githubcopilot.com/chat/completions".into()),
            copilot_models_endpoint: Some("https://api.githubcopilot.com/models".into()),
        };

        save_github_copilot_tokens(&store, &tokens).unwrap();

        assert_eq!(load_github_copilot_tokens(&store).unwrap(), Some(tokens));
        assert!(provider_has_credentials(&store, "github-copilot").unwrap());
        assert!(available_auth_modes(&store).contains(&"github-copilot".into()));
        assert!(delete_provider_credentials(&store, "github-copilot").unwrap());
        assert_eq!(load_github_copilot_tokens(&store).unwrap(), None);
    }

    #[test]
    fn empty_github_copilot_env_override_is_not_active() {
        assert!(!provider_has_env_override_from(
            "github-copilot",
            |env_var| {
                assert_eq!(env_var, "GITHUB_COPILOT_TOKEN");
                Some(" \t\n ".into())
            }
        ));
        assert!(provider_has_env_override_from(
            "github-copilot",
            |env_var| {
                assert_eq!(env_var, "GITHUB_COPILOT_TOKEN");
                Some("copilot-token".into())
            }
        ));
    }

    #[test]
    fn credential_account_names_are_stable() {
        assert_eq!(SERVICE, "rho");
        assert_eq!(OPENAI_API_KEY_ACCOUNT, "provider:openai:api-key");
        assert_eq!(ANTHROPIC_API_KEY_ACCOUNT, "provider:anthropic:api-key");
        assert_eq!(CODEX_TOKENS_ACCOUNT, "provider:openai-codex:tokens");
        assert_eq!(
            GITHUB_COPILOT_TOKENS_ACCOUNT,
            "provider:github-copilot:tokens"
        );
    }

    #[test]
    fn chunk_secret_preserves_unicode_boundaries() {
        let chunks = chunk_secret_with_max("ab🙂cd", 2);

        assert_eq!(chunks, vec!["ab", "🙂", "cd"]);
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
        assert_eq!(
            OsCredentialStore::chunk_batch_account(CODEX_TOKENS_ACCOUNT, "abc", 3),
            "provider:openai-codex:tokens:chunk:abc:3"
        );
    }

    #[test]
    fn chunk_manifest_supports_legacy_and_current_chunks() {
        assert_eq!(ChunkSet::parse("2").unwrap(), ChunkSet::Legacy { count: 2 });
        assert_eq!(
            ChunkSet::parse("v2:batch:3").unwrap(),
            ChunkSet::Current {
                batch_id: "batch".to_string(),
                count: 3,
            }
        );
    }
}
