//! Secure local-file credential storage under the Rho home directory.
//!
//! Secrets live in a private directory (`0700` on Unix) as a JSON map written
//! with mode `0600`. Updates take an exclusive lock, write a temporary file,
//! and rename into place so concurrent readers and writers stay consistent.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};

#[cfg(windows)]
use super::file_windows::set_private_windows_acl;
use super::{file_lock::FileLock, CredentialError, CredentialResult, CredentialStore};

const CREDENTIALS_DIR_NAME: &str = "credentials";
const SECRETS_FILE_NAME: &str = "secrets.json";
const LOCK_FILE_NAME: &str = "secrets.lock";
const STORE_VERSION: u32 = 1;

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
        let file = open_private_file(
            &self.lock_path,
            /*create*/ true,
            /*truncate*/ false,
            /*exclusive*/ false,
        )?;
        drop(file);
        set_private_file_permissions(&self.lock_path)?;
        Ok(())
    }

    fn with_locked_store<T>(
        &self,
        write: bool,
        op: impl FnOnce(&mut SecretDocument) -> CredentialResult<T>,
    ) -> CredentialResult<T> {
        let _process_guard = self
            .process_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let lock_file = open_private_file(
            &self.lock_path,
            /*create*/ true,
            /*truncate*/ false,
            /*exclusive*/ false,
        )?;
        let _file_guard = FileLock::acquire(lock_file)?;
        self.cleanup_stale_temp_files()?;
        let mut document = self.read_document()?;
        let result = op(&mut document)?;
        if write {
            self.write_document(&document)?;
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

    fn read_document(&self) -> CredentialResult<SecretDocument> {
        if !self.secrets_path.exists() {
            return Ok(SecretDocument::default());
        }
        let mut file = open_private_file(
            &self.secrets_path,
            /*create*/ false,
            /*truncate*/ false,
            /*exclusive*/ false,
        )?;
        let mut contents = String::new();
        file.read_to_string(&mut contents).map_err(|err| {
            CredentialError::StoreUnavailable(format!(
                "could not read credential file {}: {err}",
                self.secrets_path.display()
            ))
        })?;
        if contents.trim().is_empty() {
            return Ok(SecretDocument::default());
        }
        let document: SecretDocument = serde_json::from_str(&contents).map_err(|err| {
            CredentialError::InvalidData(format!(
                "credential file {} is invalid: {err}",
                self.secrets_path.display()
            ))
        })?;
        if document.version != STORE_VERSION {
            return Err(CredentialError::InvalidData(format!(
                "unsupported credential file version {}",
                document.version
            )));
        }
        Ok(document)
    }

    fn write_document(&self, document: &SecretDocument) -> CredentialResult<()> {
        ensure_private_directory(&self.directory)?;
        let payload = serde_json::to_vec_pretty(document).map_err(|err| {
            CredentialError::InvalidData(format!("could not encode credentials: {err}"))
        })?;
        let temp_name = format!(
            "{SECRETS_FILE_NAME}.tmp.{}-{:x}",
            std::process::id(),
            random_u64()
        );
        let temp_path = self.directory.join(temp_name);
        let mut temp_guard = TempFileGuard::new(temp_path.clone());
        {
            let mut file = open_private_file(
                &temp_path, /*create*/ true, /*truncate*/ true, /*exclusive*/ true,
            )?;
            file.write_all(&payload).map_err(|err| {
                CredentialError::StoreUnavailable(format!(
                    "could not write credential temp file {}: {err}",
                    temp_path.display()
                ))
            })?;
            file.sync_all().map_err(|err| {
                CredentialError::StoreUnavailable(format!(
                    "could not sync credential temp file {}: {err}",
                    temp_path.display()
                ))
            })?;
        }
        set_private_file_permissions(&temp_path)?;
        publish_temp_file(&temp_path, &self.secrets_path).map_err(|err| {
            CredentialError::StoreUnavailable(format!(
                "could not publish credential file {}: {err}",
                self.secrets_path.display()
            ))
        })?;
        temp_guard.disarm();
        set_private_file_permissions(&self.secrets_path)?;
        Ok(())
    }
}

impl CredentialStore for FileCredentialStore {
    fn get_secret(&self, account: &str) -> CredentialResult<Option<String>> {
        validate_account(account)?;
        self.with_locked_store(
            /*write*/ false,
            |document| Ok(document.secrets.get(account).cloned()),
        )
    }

    fn set_secret(&self, account: &str, secret: &str) -> CredentialResult<()> {
        validate_account(account)?;
        self.with_locked_store(/*write*/ true, |document| {
            document
                .secrets
                .insert(account.to_string(), secret.to_string());
            Ok(())
        })
    }

    fn delete_secret(&self, account: &str) -> CredentialResult<bool> {
        validate_account(account)?;
        self.with_locked_store(/*write*/ true, |document| {
            Ok(document.secrets.remove(account).is_some())
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SecretDocument {
    version: u32,
    secrets: BTreeMap<String, String>,
}

impl Default for SecretDocument {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            secrets: BTreeMap::new(),
        }
    }
}

fn publish_temp_file(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(not(windows))]
    {
        fs::rename(source, destination)
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;

        #[link(name = "kernel32")]
        extern "system" {
            fn MoveFileExW(
                existing_file_name: *const u16,
                new_file_name: *const u16,
                flags: u32,
            ) -> i32;
        }

        const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
        const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
        let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let destination: Vec<u16> = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let result = unsafe {
            MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if result == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

fn validate_account(account: &str) -> CredentialResult<()> {
    if account.is_empty() {
        return Err(CredentialError::InvalidData(
            "credential account name cannot be empty".into(),
        ));
    }
    if account.contains('\0') {
        return Err(CredentialError::InvalidData(
            "credential account name cannot contain NUL".into(),
        ));
    }
    Ok(())
}

fn ensure_private_directory(path: &Path) -> CredentialResult<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).map_err(|err| {
        CredentialError::StoreUnavailable(format!(
            "could not create credential directory {}: {err}",
            path.display()
        ))
    })?;
    validate_private_directory(path)?;
    set_private_directory_permissions(path)?;
    Ok(())
}

fn validate_private_directory(path: &Path) -> CredentialResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        CredentialError::StoreUnavailable(format!(
            "could not inspect credential directory {}: {error}",
            path.display()
        ))
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(CredentialError::StoreUnavailable(format!(
            "credential directory {} is not a real directory",
            path.display()
        )));
    }
    validate_owner(&metadata, path)
}

fn validate_private_file(file: &File, path: &Path) -> CredentialResult<()> {
    let metadata = file.metadata().map_err(|error| {
        CredentialError::StoreUnavailable(format!(
            "could not inspect credential path {}: {error}",
            path.display()
        ))
    })?;
    if !metadata.is_file() {
        return Err(CredentialError::StoreUnavailable(format!(
            "credential path {} is not a regular file",
            path.display()
        )));
    }
    validate_owner(&metadata, path)
}

fn validate_owner(metadata: &fs::Metadata, path: &Path) -> CredentialResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err(CredentialError::StoreUnavailable(format!(
                "credential path {} is not owned by the current user",
                path.display()
            )));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(CredentialError::StoreUnavailable(format!(
                "credential path {} is a Windows reparse point",
                path.display()
            )));
        }
    }
    Ok(())
}

struct TempFileGuard {
    path: PathBuf,
    armed: bool,
}

impl TempFileGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn open_private_file(
    path: &Path,
    create: bool,
    truncate: bool,
    exclusive: bool,
) -> CredentialResult<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    if create {
        if exclusive {
            options.create_new(true);
        } else {
            options.create(true);
        }
    }
    if truncate {
        options.truncate(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path).map_err(|err| {
        CredentialError::StoreUnavailable(format!(
            "could not open credential path {}: {err}",
            path.display()
        ))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|err| {
                CredentialError::StoreUnavailable(format!(
                    "could not set permissions on {}: {err}",
                    path.display()
                ))
            })?;
    }
    validate_private_file(&file, path)?;
    Ok(file)
}

fn set_private_directory_permissions(path: &Path) -> CredentialResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|err| {
            CredentialError::StoreUnavailable(format!(
                "could not set permissions on {}: {err}",
                path.display()
            ))
        })?;
    }
    #[cfg(windows)]
    set_private_windows_acl(path, /*directory*/ true)?;
    Ok(())
}

fn set_private_file_permissions(path: &Path) -> CredentialResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|err| {
            CredentialError::StoreUnavailable(format!(
                "could not set permissions on {}: {err}",
                path.display()
            ))
        })?;
    }
    #[cfg(windows)]
    set_private_windows_acl(path, /*directory*/ false)?;
    Ok(())
}

fn random_u64() -> u64 {
    rand::random()
}

#[cfg(test)]
#[path = "file_tests.rs"]
mod tests;
