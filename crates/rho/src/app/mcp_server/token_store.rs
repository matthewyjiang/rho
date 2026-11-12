//! Cross-process transactions for the single MCP token credential account.
//!
//! The OS credential account is shared across rho homes. Serialize mutations
//! per OS user, including file-backed stores, rather than keying a lock by
//! RHO_HOME, TMPDIR, or the server's runtime directory. Where that user's
//! lock file is, and what makes it theirs alone, is the platform's (see
//! `daemon::account_lock_path`).

use std::path::Path;

use rho_providers::credentials::CredentialStore;

use super::tokens::TokenSet;
use crate::{
    credential_store::UntrackedCredentialStore,
    daemon::{account_lock_path, lock_account_file},
};

/// The lock file's name in the account's home.
const LOCK_FILE: &str = ".rho-mcp-tokens.lock";

/// What the lock guards, as its errors name it.
const GUARDED: &str = "MCP token";

/// Reloads, changes, and saves the token set under one cross-process lock.
/// CLI formatting and printing happen only after this transaction succeeds.
pub(crate) fn update<T>(
    change: impl FnOnce(&mut TokenSet) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    update_at(
        &UntrackedCredentialStore,
        &account_lock_path(LOCK_FILE, GUARDED)?,
        change,
    )
}

fn update_at<T>(
    store: &dyn CredentialStore,
    lock_path: &Path,
    change: impl FnOnce(&mut TokenSet) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let _lock = lock_account_file(lock_path, GUARDED)?;
    let mut tokens = TokenSet::load(store)?;
    let result = change(&mut tokens)?;
    tokens.save(store)?;
    Ok(result)
}

#[cfg(test)]
#[path = "token_store_tests.rs"]
mod tests;
