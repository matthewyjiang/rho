//! Operating system credential store backed by the platform keyring.
//!
//! Large secrets are chunked on platforms with entry-size limits. Behavior is
//! unchanged from the historical [`super::OsCredentialStore`] implementation.

use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

use super::{CredentialError, CredentialResult, CredentialStore};

pub(super) const SERVICE: &str = "rho";
const CHUNK_MANIFEST_SUFFIX: &str = ":chunks";
const CHUNK_ACCOUNT_INFIX: &str = ":chunk:";

#[cfg(windows)]
const MAX_SECRET_CHUNK_UTF16_UNITS: usize = 1000;

pub(super) const CHUNK_MANIFEST_VERSION: &str = "v2";

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
        // Drop the manifest first so readers fall through to the plain entry.
        // Orphan chunk bodies are harmless without a manifest.
        let deleted = Self::delete_entry_secret(&manifest_account)?;
        match delete_chunks_for_manifest(account, &manifest) {
            Ok(chunks_deleted) => Ok(deleted | chunks_deleted),
            Err(_) if deleted => Ok(true),
            Err(err) => Err(err),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ChunkSet {
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

pub(super) struct UncachedOsCredentialStore;

impl CredentialStore for UncachedOsCredentialStore {
    fn get_secret(&self, account: &str) -> CredentialResult<Option<String>> {
        OsCredentialStore::get_entry_secret(account)
    }

    fn set_secret(&self, account: &str, secret: &str) -> CredentialResult<()> {
        OsCredentialStore::set_entry_secret(account, secret)
    }

    fn delete_secret(&self, account: &str) -> CredentialResult<bool> {
        OsCredentialStore::delete_entry_secret(account)
    }
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

pub(super) fn chunk_secret_with_max(secret: &str, max_utf16_units: usize) -> Vec<String> {
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
#[path = "os_tests.rs"]
mod tests;
