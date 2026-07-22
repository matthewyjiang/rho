//! Secrets document serialization and atomic publish helpers.

use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::file_permissions::{
    ensure_private_directory, open_private_file, set_private_file_permissions, PrivateFileOpen,
};
use super::{CredentialError, CredentialResult};

pub(super) const STORE_VERSION: u32 = 1;
const SECRETS_FILE_NAME: &str = "secrets.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct SecretDocument {
    pub(super) version: u32,
    pub(super) secrets: BTreeMap<String, String>,
}

impl Default for SecretDocument {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            secrets: BTreeMap::new(),
        }
    }
}

pub(super) fn read_document(secrets_path: &Path) -> CredentialResult<SecretDocument> {
    if !secrets_path.exists() {
        return Ok(SecretDocument::default());
    }
    let mut file = open_private_file(secrets_path, PrivateFileOpen::Existing)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents).map_err(|err| {
        CredentialError::StoreUnavailable(format!(
            "could not read credential file {}: {err}",
            secrets_path.display()
        ))
    })?;
    if contents.trim().is_empty() {
        return Ok(SecretDocument::default());
    }
    let document: SecretDocument = serde_json::from_str(&contents).map_err(|err| {
        CredentialError::InvalidData(format!(
            "credential file {} is invalid: {err}",
            secrets_path.display()
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

pub(super) fn write_document(
    directory: &Path,
    secrets_path: &Path,
    document: &SecretDocument,
) -> CredentialResult<()> {
    ensure_private_directory(directory)?;
    let payload = serde_json::to_vec_pretty(document).map_err(|err| {
        CredentialError::InvalidData(format!("could not encode credentials: {err}"))
    })?;
    let temp_name = format!(
        "{SECRETS_FILE_NAME}.tmp.{}-{:x}",
        std::process::id(),
        random_u64()
    );
    let temp_path = directory.join(temp_name);
    let mut temp_guard = TempFileGuard::new(temp_path.clone());
    {
        let mut file = open_private_file(&temp_path, PrivateFileOpen::CreateNew)?;
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
    publish_temp_file(&temp_path, secrets_path).map_err(|err| {
        CredentialError::StoreUnavailable(format!(
            "could not publish credential file {}: {err}",
            secrets_path.display()
        ))
    })?;
    temp_guard.disarm();
    set_private_file_permissions(secrets_path)?;
    Ok(())
}

fn publish_temp_file(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(not(windows))]
    {
        fs::rename(source, destination)
    }
    #[cfg(windows)]
    {
        super::file_windows::replace_file(source, destination)
    }
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

fn random_u64() -> u64 {
    rand::random()
}
