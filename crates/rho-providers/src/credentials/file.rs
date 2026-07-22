//! Secure local-file credential storage under the Rho home directory.
//!
//! Secrets live in a private directory (`0700` on Unix) as a JSON map written
//! with mode `0600`. Updates take an exclusive lock, write a temporary file,
//! and rename into place so concurrent readers and writers stay consistent.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use super::file_document::{read_document, write_document, SecretDocument};
use super::file_lock::FileLock;
use super::file_permissions::{
    ensure_private_directory, open_private_file, set_private_file_permissions, validate_account,
    validate_owner, PrivateFileOpen,
};
use super::{CredentialError, CredentialResult, CredentialStore};

const CREDENTIALS_DIR_NAME: &str = "credentials";
const SECRETS_FILE_NAME: &str = "secrets.json";
const LOCK_FILE_NAME: &str = "secrets.lock";

/// Whether a locked store operation changed the on-disk document.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DocumentMutation {
    Unchanged,
    Mutated,
}

/// File-backed credential store under a private Rho credentials directory.
#[derive(Debug)]
pub struct FileCredentialStore {
    directory: PathBuf,
    secrets_path: PathBuf,
    lock_path: PathBuf,
    /// Serializes operations within one process; cross-process safety uses the lock file.
    process_lock: Mutex<()>,
}

impl FileCredentialStore {
    /// Opens the default file store under `RHO_HOME` or `~/.rho/credentials`.
    pub fn open() -> CredentialResult<Self> {
        let rho_home = crate::paths::rho_dir().map_err(|err| {
            CredentialError::StoreUnavailable(format!("could not resolve Rho home: {err}"))
        })?;
        Self::with_rho_home(rho_home)
    }

    /// Opens a file store under `{rho_home}/credentials`.
    pub fn with_rho_home(rho_home: impl Into<PathBuf>) -> CredentialResult<Self> {
        Self::with_directory(rho_home.into().join(CREDENTIALS_DIR_NAME))
    }

    /// Opens a file store that uses `directory` as the private credentials root.
    pub fn with_directory(directory: impl Into<PathBuf>) -> CredentialResult<Self> {
        let directory = directory.into();
        ensure_private_directory(&directory)?;
        let store = Self {
            secrets_path: directory.join(SECRETS_FILE_NAME),
            lock_path: directory.join(LOCK_FILE_NAME),
            directory,
            process_lock: Mutex::new(()),
        };
        // Touch the lock file with private permissions so later openers inherit mode 0600.
        store.ensure_lock_file()?;
        Ok(store)
    }

    /// Returns the private credentials directory used by this store.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Returns the secrets file path.
    pub fn secrets_path(&self) -> &Path {
        &self.secrets_path
    }

    fn ensure_lock_file(&self) -> CredentialResult<()> {
        let file = open_private_file(&self.lock_path, PrivateFileOpen::OpenOrCreate)?;
        drop(file);
        set_private_file_permissions(&self.lock_path)?;
        Ok(())
    }

    fn with_locked_store<T>(
        &self,
        op: impl FnOnce(&mut SecretDocument) -> CredentialResult<(T, DocumentMutation)>,
    ) -> CredentialResult<T> {
        let _process_guard = self
            .process_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let lock_file = open_private_file(&self.lock_path, PrivateFileOpen::OpenOrCreate)?;
        let _file_guard = FileLock::acquire(lock_file)?;
        self.cleanup_stale_temp_files()?;
        let mut document = read_document(&self.secrets_path)?;
        let (result, mutation) = op(&mut document)?;
        if mutation == DocumentMutation::Mutated {
            write_document(&self.directory, &self.secrets_path, &document)?;
        }
        Ok(result)
    }

    fn cleanup_stale_temp_files(&self) -> CredentialResult<()> {
        let prefix = format!("{SECRETS_FILE_NAME}.tmp.");
        for entry in fs::read_dir(&self.directory).map_err(|error| {
            CredentialError::StoreUnavailable(format!(
                "could not inspect credential directory {}: {error}",
                self.directory.display()
            ))
        })? {
            let entry = entry.map_err(|error| {
                CredentialError::StoreUnavailable(format!(
                    "could not inspect credential temp file: {error}"
                ))
            })?;
            if !entry.file_name().to_string_lossy().starts_with(&prefix) {
                continue;
            }
            let metadata = entry.metadata().map_err(|error| {
                CredentialError::StoreUnavailable(format!(
                    "could not inspect credential temp file {}: {error}",
                    entry.path().display()
                ))
            })?;
            if metadata.is_file() {
                validate_owner(&metadata, &entry.path())?;
                fs::remove_file(entry.path()).map_err(|error| {
                    CredentialError::StoreUnavailable(format!(
                        "could not remove stale credential temp file: {error}"
                    ))
                })?;
            }
        }
        Ok(())
    }
}

impl CredentialStore for FileCredentialStore {
    fn get_secret(&self, account: &str) -> CredentialResult<Option<String>> {
        validate_account(account)?;
        self.with_locked_store(|document| {
            Ok((
                document.secrets.get(account).cloned(),
                DocumentMutation::Unchanged,
            ))
        })
    }

    fn set_secret(&self, account: &str, secret: &str) -> CredentialResult<()> {
        validate_account(account)?;
        self.with_locked_store(|document| {
            document
                .secrets
                .insert(account.to_string(), secret.to_string());
            Ok(((), DocumentMutation::Mutated))
        })
    }

    fn delete_secret(&self, account: &str) -> CredentialResult<bool> {
        validate_account(account)?;
        self.with_locked_store(|document| {
            let removed = document.secrets.remove(account).is_some();
            let mutation = if removed {
                DocumentMutation::Mutated
            } else {
                DocumentMutation::Unchanged
            };
            Ok((removed, mutation))
        })
    }
}

#[cfg(test)]
#[path = "file_tests.rs"]
mod tests;
