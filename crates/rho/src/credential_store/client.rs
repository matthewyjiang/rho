//! The credentials of one daemon client (#1779).
//!
//! A daemon serves clients whose environments differ, so nothing a session
//! reads may come from the daemon's own: not an API key in a variable, and
//! not `RHO_CREDENTIAL_STORE`. A [`ClientCredentialStore`] answers both from
//! the variables its client sent, and is what a session, and a sign-in made
//! for that client, read and write credentials through.

use std::{collections::BTreeMap, sync::Arc};

use rho_providers::credentials::{open_credential_store, CredentialResult, CredentialStore};

use super::{selected_store, selection, tracked_delete, tracked_set, ENV_BACKEND};

/// Whether a write through a store moves the credential generation (see
/// [`super::AppCredentialStore`]), which makes every session rebuild its
/// provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Writes {
    /// Provider credentials: sessions rebuild their providers.
    Tracked,
    /// Secrets no provider reads, as [`super::UntrackedCredentialStore`]
    /// keeps.
    Untracked,
}

/// One daemon client's credentials: the environment that overrides saved
/// ones, and the store that keeps them. The store is the one
/// `RHO_CREDENTIAL_STORE` in the client's environment names, else this
/// process's own (see [`super::AppCredentialStore`]); an unknown value is
/// refused on every access, never replaced by a default. Variables this
/// process's environment sets are never read.
#[derive(Clone)]
pub(crate) struct ClientCredentialStore {
    environment: Arc<BTreeMap<String, String>>,
    writes: Writes,
}

impl ClientCredentialStore {
    /// The credentials of the client that sent `environment`.
    pub(crate) fn new(environment: Arc<BTreeMap<String, String>>) -> Self {
        Self {
            environment,
            writes: Writes::Tracked,
        }
    }

    /// The same credentials for secrets no provider reads, whose writes
    /// rebuild no session's provider. The caller decides that what it
    /// writes feeds no provider.
    pub(crate) fn untracked(&self) -> Self {
        Self {
            environment: Arc::clone(&self.environment),
            writes: Writes::Untracked,
        }
    }

    fn store(&self) -> CredentialResult<Arc<dyn CredentialStore>> {
        let chosen = self.environment.get(ENV_BACKEND).map(String::as_str);
        match selection::parse_environment(chosen)? {
            Some(backend) => open_credential_store(backend),
            None => selected_store(),
        }
    }
}

impl CredentialStore for ClientCredentialStore {
    fn get_secret(&self, account: &str) -> CredentialResult<Option<String>> {
        self.store()?.get_secret(account)
    }

    fn get_secret_fresh(&self, account: &str) -> CredentialResult<Option<String>> {
        self.store()?.get_secret_fresh(account)
    }

    fn set_secret(&self, account: &str, secret: &str) -> CredentialResult<()> {
        let store = self.store()?;
        match self.writes {
            Writes::Tracked => tracked_set(store.as_ref(), account, secret),
            Writes::Untracked => store.set_secret(account, secret),
        }
    }

    fn delete_secret(&self, account: &str) -> CredentialResult<bool> {
        let store = self.store()?;
        match self.writes {
            Writes::Tracked => tracked_delete(store.as_ref(), account),
            Writes::Untracked => store.delete_secret(account),
        }
    }

    fn environment_variable(&self, name: &str) -> Option<String> {
        self.environment.get(name).cloned()
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
