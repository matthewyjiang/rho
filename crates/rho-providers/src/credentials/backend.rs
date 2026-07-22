//! Explicit credential-store backend selection and availability probes.
//!
//! Security rules:
//! - [`CredentialStoreBackend::Auto`] only uses the OS keyring.
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
/// `Auto` preserves the historical OS-only behavior: if the OS store is
/// unavailable, Rho fails closed instead of writing secrets to disk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CredentialStoreBackend {
    /// Probe and open the OS credential store only. Never uses file storage.
    #[default]
    Auto,
    /// Always use the operating system credential store.
    Os,
    /// Use private files under the Rho home directory. Must be selected explicitly.
    File,
}

impl CredentialStoreBackend {
    /// Parses a backend selector from configuration or CLI text.
    ///
    /// Accepted values are `auto`, `os`, and `file` (case-insensitive).
    pub fn parse(value: &str) -> CredentialResult<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "os" => Ok(Self::Os),
            "file" => Ok(Self::File),
            other => Err(CredentialError::InvalidData(format!(
                "unknown credential store backend '{other}'; expected auto, os, or file"
            ))),
        }
    }

    /// Returns the canonical lowercase name for this backend.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Os => "os",
            Self::File => "file",
        }
    }

    /// Backend used for availability probes and store construction.
    ///
    /// `Auto` resolves to the OS backend and never to file storage.
    pub const fn resolved(self) -> Self {
        match self {
            Self::Auto => Self::Os,
            Self::Os | Self::File => self,
        }
    }
}

/// Result of a non-destructive credential-store availability probe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialStoreProbe {
    /// Backend that was requested.
    pub requested: CredentialStoreBackend,
    /// Backend that was actually probed (`auto` resolves to `os`).
    pub backend: CredentialStoreBackend,
    /// Whether create/read/delete of a temporary secret succeeded.
    pub available: bool,
    /// Human-readable detail for diagnostics. Never contains the probe secret.
    pub detail: String,
}

/// Opens the selected credential store backend.
///
/// `auto` opens the OS store only. It never falls back to file storage.
pub fn open_credential_store(
    backend: CredentialStoreBackend,
) -> CredentialResult<Arc<dyn CredentialStore>> {
    match backend.resolved() {
        CredentialStoreBackend::Auto => unreachable!("auto resolves to os"),
        CredentialStoreBackend::Os => Ok(Arc::new(OsCredentialStore)),
        CredentialStoreBackend::File => Ok(Arc::new(FileCredentialStore::open()?)),
    }
}

/// Probes whether the selected backend can store a temporary secret.
///
/// The probe creates a random account, writes a random secret, reads it back,
/// deletes it, and confirms deletion. Existing credentials are left untouched.
///
/// `auto` probes the OS backend only and never tries file storage.
pub fn probe_credential_store(backend: CredentialStoreBackend) -> CredentialStoreProbe {
    let resolved = backend.resolved();
    match probe_resolved_backend(resolved) {
        Ok(()) => CredentialStoreProbe {
            requested: backend,
            backend: resolved,
            available: true,
            detail: format!(
                "{} credential store accepted a temporary secret",
                resolved.as_str()
            ),
        },
        Err(err) => CredentialStoreProbe {
            requested: backend,
            backend: resolved,
            available: false,
            detail: err.to_string(),
        },
    }
}

fn probe_resolved_backend(backend: CredentialStoreBackend) -> CredentialResult<()> {
    let store: Arc<dyn CredentialStore> = match backend {
        CredentialStoreBackend::Auto => unreachable!("auto resolves before probe"),
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
