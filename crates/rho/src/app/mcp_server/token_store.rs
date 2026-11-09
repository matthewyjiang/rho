//! Cross-process transactions for the single MCP token credential account.
//!
//! The OS credential account is shared across rho homes. Serialize mutations
//! per OS user, including file-backed stores, rather than keying a lock by
//! RHO_HOME, TMPDIR, or the server's runtime directory.

use std::{
    fs::{File, OpenOptions},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

use anyhow::Context;
use rho_providers::credentials::CredentialStore;

use super::tokens::TokenSet;
use crate::credential_store::{AppCredentialStore, UntrackedCredentialStore};

/// Reloads, changes, and saves the token set under one cross-process lock.
/// CLI formatting and printing happen only after this transaction succeeds.
pub(crate) fn update<T>(
    change: impl FnOnce(&mut TokenSet) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    // SAFETY: geteuid has no arguments or memory-safety preconditions.
    let user = unsafe { libc::geteuid() };
    // A fixed per-user path is intentional: the OS store's `rho` account
    // does not change when a caller overrides RHO_HOME or TMPDIR.
    let lock_path = format!("/tmp/rho-mcp-tokens-{user}.lock");
    update_at(&UntrackedCredentialStore, Path::new(&lock_path), change)
}

fn update_at<T>(
    store: &dyn CredentialStore,
    lock_path: &Path,
    change: impl FnOnce(&mut TokenSet) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let _lock = lock_tokens(lock_path)?;
    AppCredentialStore::reread_external_logins();
    let mut tokens = TokenSet::load(store)?;
    let result = change(&mut tokens)?;
    tokens.save(store)?;
    Ok(result)
}

fn lock_tokens(path: &Path) -> anyhow::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .with_context(|| format!("could not open MCP token mutation lock {}", path.display()))?;
    let metadata = file.metadata()?;
    // SAFETY: geteuid has no arguments or memory-safety preconditions.
    let user = unsafe { libc::geteuid() };
    if !metadata.is_file() || metadata.uid() != user || metadata.mode() & 0o077 != 0 {
        anyhow::bail!(
            "MCP token mutation lock {} must be a regular file owned by uid {user} with no group or other access",
            path.display()
        );
    }
    fs2::FileExt::lock_exclusive(&file)
        .with_context(|| format!("could not lock MCP token mutations at {}", path.display()))?;
    Ok(file)
}

#[cfg(test)]
#[path = "token_store_tests.rs"]
mod tests;
