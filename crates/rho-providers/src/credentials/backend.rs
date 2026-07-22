//! Explicit credential-store backend selection and availability probes.
//!
//! Security rules:
//! - [`CredentialStoreBackend::Os`] is the default and uses the OS keyring only.
//! - Local file storage is never chosen as a silent fallback.
//! - Callers must select [`CredentialStoreBackend::File`] explicitly.

use std::sync::Arc;

use rand::{distributions::Alphanumeric, Rng};

use super::{
    file::FileCredentialStore,
    os::{OsCredentialStore, UncachedOsCredentialStore},
    CredentialError, CredentialResult, CredentialStore,
};

/// Selects which credential storage backend to construct or probe.
///
/// The default is the OS keyring. If the OS store is unavailable, Rho fails
/// closed instead of writing secrets to disk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CredentialStoreBackend {
    /// Use the operating system credential store.
    #[default]
    Os,
    /// Use private files under the Rho home directory. Must be selected explicitly.
    File,
}

impl CredentialStoreBackend {
    /// Parses a backend selector from configuration or CLI text.
    ///
    /// Accepted values are `os` and `file` (case-insensitive). `auto` is
    /// accepted as an alias for `os` for backwards compatibility.
    pub fn parse(value: &str) -> CredentialResult<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" | "os" => Ok(Self::Os),
            "file" => Ok(Self::File),
            other => Err(CredentialError::InvalidData(format!(
                "unknown credential store backend '{other}'; expected os or file"
            ))),
        }
    }

    /// Returns the canonical lowercase name for this backend.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Os => "os",
            Self::File => "file",
        }
    }
}

/// Result of a non-destructive credential-store availability probe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialStoreProbe {
    /// Backend that was requested and probed.
    pub backend: CredentialStoreBackend,
    /// Whether create/read/delete of a temporary secret succeeded.
    pub available: bool,
    /// Human-readable detail for diagnostics. Never contains the probe secret.
    pub detail: String,
}

/// Opens the selected credential store backend.
///
/// File storage is never selected implicitly.
pub fn open_credential_store(
    backend: CredentialStoreBackend,
) -> CredentialResult<Arc<dyn CredentialStore>> {
    match backend {
        CredentialStoreBackend::Os => Ok(Arc::new(OsCredentialStore)),
        CredentialStoreBackend::File => Ok(Arc::new(FileCredentialStore::open()?)),
    }
}

/// Probes whether the selected backend can store a temporary secret.
///
/// The probe creates a random account, writes a random secret, reads it back,
/// deletes it, and confirms deletion. Existing credentials are left untouched.
pub fn probe_credential_store(backend: CredentialStoreBackend) -> CredentialStoreProbe {
    match probe_backend(backend) {
        Ok(()) => CredentialStoreProbe {
            backend,
            available: true,
            detail: format!(
                "{} credential store accepted a temporary secret",
                backend.as_str()
            ),
        },
        Err(err) => CredentialStoreProbe {
            backend,
            available: false,
            detail: err.to_string(),
        },
    }
}

fn probe_backend(backend: CredentialStoreBackend) -> CredentialResult<()> {
    let store: Arc<dyn CredentialStore> = match backend {
        CredentialStoreBackend::Os => Arc::new(UncachedOsCredentialStore),
        CredentialStoreBackend::File => Arc::new(FileCredentialStore::open()?),
    };
    run_availability_probe(store.as_ref())
}

fn run_availability_probe(store: &dyn CredentialStore) -> CredentialResult<()> {
    let account = probe_account_name();
    let secret = probe_secret_value();

    // Best-effort cleanup if a previous interrupted probe left this account.
    let _ = store.delete_secret(&account);

    if let Err(error) = store.set_secret(&account, &secret) {
        let _ = cleanup_probe_secret(store, &account);
        return Err(error);
    }
    let round_trip = match store.get_secret(&account) {
        Ok(Some(loaded)) if loaded == secret => Ok(()),
        Ok(Some(_)) => Err(CredentialError::InvalidData(
            "credential store probe did not round trip".into(),
        )),
        Ok(None) => Err(CredentialError::InvalidData(
            "credential store probe could not read back the temporary secret".into(),
        )),
        Err(error) => Err(error),
    };
    let cleanup = cleanup_probe_secret(store, &account);
    round_trip?;
    cleanup
}

fn cleanup_probe_secret(store: &dyn CredentialStore, account: &str) -> CredentialResult<()> {
    let mut last_error = None;
    for _ in 0..2 {
        match store.delete_secret(account) {
            Ok(_) => match store.get_secret(account) {
                Ok(None) => return Ok(()),
                Ok(Some(_)) => {
                    last_error = Some(CredentialError::InvalidData(
                        "credential store probe still reported the temporary secret after delete"
                            .into(),
                    ));
                }
                Err(error) => last_error = Some(error),
            },
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        CredentialError::InvalidData(
            "credential store probe could not delete the temporary secret".into(),
        )
    }))
}

fn probe_account_name() -> String {
    let token = random_token(16);
    format!("rho-probe:{token}")
}

fn probe_secret_value() -> String {
    format!("rho-probe-secret:{}", random_token(24))
}

fn random_token(len: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

#[cfg(test)]
#[path = "backend_tests.rs"]
mod tests;
