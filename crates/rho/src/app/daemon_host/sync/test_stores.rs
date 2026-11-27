//! Credential stores and settings the sync tests share.

use std::{ops::Deref, path::Path, sync::Arc};

use rho_providers::credentials::{
    CredentialError, CredentialResult, CredentialStore, MemoryCredentialStore,
};

use super::{SecretSync, StoreSource};
use crate::app::{agent_concurrency::AgentConcurrency, daemon_host::settings::ConfigStore};

/// `inner`, except that it refuses every write to `account`.
pub(super) struct Refusing<S> {
    pub(super) inner: S,
    pub(super) account: &'static str,
}

impl<S: Deref<Target = MemoryCredentialStore> + Send + Sync> CredentialStore for Refusing<S> {
    fn get_secret(&self, account: &str) -> CredentialResult<Option<String>> {
        self.inner.get_secret(account)
    }

    fn set_secret(&self, account: &str, secret: &str) -> CredentialResult<()> {
        if account == self.account {
            return Err(CredentialError::StoreUnavailable(format!(
                "refused {secret}"
            )));
        }
        self.inner.set_secret(account, secret)
    }

    fn delete_secret(&self, account: &str) -> CredentialResult<bool> {
        self.inner.delete_secret(account)
    }
}

impl SecretSync {
    /// Sync state that keeps keys in `keys` and its journal in `journal`.
    pub(super) fn with_stores(
        keys: Arc<dyn CredentialStore>,
        journal: Arc<dyn CredentialStore>,
    ) -> Self {
        Self {
            stores: StoreSource::Fixed { keys, journal },
            ..Self::default()
        }
    }
}

/// The settings of the home `home`, whose `config.toml` holds `file`.
pub(super) fn settings(home: &Path, file: &str) -> ConfigStore {
    std::fs::write(home.join("config.toml"), file).unwrap();
    ConfigStore::new(
        home,
        AgentConcurrency::from_config(crate::config::DEFAULT_AGENT_CONCURRENCY),
    )
}
