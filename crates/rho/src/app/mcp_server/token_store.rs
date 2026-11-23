//! Cross-process transactions for the MCP server's credential accounts:
//! its tokens (`tokens.rs`), its webhook targets (`webhooks.rs`), and the
//! remotes a hub reaches (`remotes/store.rs`), each one JSON entry.
//!
//! The OS credential account is shared across rho homes. Serialize mutations
//! per OS user, including file-backed stores, rather than keying a lock by
//! RHO_HOME, TMPDIR, or the server's runtime directory. Where that user's
//! lock file is, and what makes it theirs alone, is the platform's (see
//! `daemon::account_lock_path`). One lock guards every account: they change
//! only at a person's command, so they never wait on each other long.

use std::path::Path;

use anyhow::Context;
use rho_providers::credentials::CredentialStore;
use serde::{de::DeserializeOwned, Serialize};

use crate::{
    credential_store::UntrackedCredentialStore,
    daemon::{account_lock_path, lock_account_file},
};

/// The lock file's name in the account's home. It is named for the tokens,
/// which it guarded first; renaming it would let builds on either side of
/// the rename change the accounts at once.
const LOCK_FILE: &str = ".rho-mcp-tokens.lock";

/// What the lock guards, as its errors name it: the tokens, the webhook
/// targets, and the remotes.
const GUARDED: &str = "MCP server credentials";

/// A set kept as one JSON entry in the credential store, changed only
/// through [`update`]. Implementors name the account and how errors call
/// the set; an absent entry is the empty set.
pub(crate) trait StoredSet: Default + Serialize + DeserializeOwned {
    /// The credential-store account that holds the set.
    const ACCOUNT: &'static str;
    /// What the set is, as errors name it, such as "MCP server tokens".
    const WHAT: &'static str;
    /// Cross-process lock file in the account's home. Defaults to the MCP
    /// server's lock file so existing MCP credential sets keep sharing it.
    const LOCK_FILE: &'static str = LOCK_FILE;
    /// Human-readable description of what `LOCK_FILE` guards.
    const GUARDED: &'static str = GUARDED;
}

/// The set saved in `store`, read past the store's per-process cache so a
/// change another process saved applies; empty when none is.
pub(crate) fn load<S: StoredSet>(store: &dyn CredentialStore) -> anyhow::Result<S> {
    let Some(saved) = store
        .get_secret_fresh(S::ACCOUNT)
        .with_context(|| format!("could not read the {} from the credential store", S::WHAT))?
    else {
        return Ok(S::default());
    };
    serde_json::from_str(&saved).with_context(|| format!("the saved {} do not parse", S::WHAT))
}

/// [`load`] from the store rho opened, off the async workers, as a server
/// reads a set on every request so a change another process saved applies.
pub(crate) async fn load_fresh<S: StoredSet + Send + 'static>() -> anyhow::Result<S> {
    tokio::task::spawn_blocking(|| load::<S>(&UntrackedCredentialStore)).await?
}

pub(crate) fn save<S: StoredSet>(set: &S, store: &dyn CredentialStore) -> anyhow::Result<()> {
    let encoded = serde_json::to_string(set)?;
    store
        .set_secret(S::ACCOUNT, &encoded)
        .with_context(|| format!("could not save the {} in the credential store", S::WHAT))
}

/// Reloads, changes, and saves a set under one cross-process lock. CLI
/// formatting and printing happen only after this transaction succeeds.
pub(crate) fn update<S: StoredSet, T>(
    change: impl FnOnce(&mut S) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    update_at(
        &UntrackedCredentialStore,
        &account_lock_path(S::LOCK_FILE, S::GUARDED)?,
        change,
    )
}

fn update_at<S: StoredSet, T>(
    store: &dyn CredentialStore,
    lock_path: &Path,
    change: impl FnOnce(&mut S) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let _lock = lock_account_file(lock_path, S::GUARDED)?;
    let mut set = load::<S>(store)?;
    let result = change(&mut set)?;
    save(&set, store)?;
    Ok(result)
}

#[cfg(test)]
#[path = "token_store_tests.rs"]
mod tests;
