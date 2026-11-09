//! Cross-process transactions for the single MCP token credential account.
//!
//! The OS credential account is shared across rho homes. Serialize mutations
//! per OS user, including file-backed stores, rather than keying a lock by
//! RHO_HOME, TMPDIR, or the server's runtime directory.

use std::{
    ffi::{CStr, OsStr},
    fs::{File, OpenOptions},
    mem::MaybeUninit,
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

use anyhow::Context;
use rho_providers::credentials::CredentialStore;

use super::tokens::TokenSet;
use crate::credential_store::UntrackedCredentialStore;

/// Reloads, changes, and saves the token set under one cross-process lock.
/// CLI formatting and printing happen only after this transaction succeeds.
pub(crate) fn update<T>(
    change: impl FnOnce(&mut TokenSet) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    // The OS account is per uid, so neither env overrides nor the presence
    // of a logind runtime directory may select a different mutation lock.
    // SAFETY: geteuid has no arguments or memory-safety preconditions.
    let user = unsafe { libc::geteuid() };
    let lock_path = lock_path_in(&account_home(user)?, user)?;
    update_at(&UntrackedCredentialStore, &lock_path, change)
}

fn update_at<T>(
    store: &dyn CredentialStore,
    lock_path: &Path,
    change: impl FnOnce(&mut TokenSet) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let _lock = lock_tokens(lock_path)?;
    let mut tokens = TokenSet::load(store)?;
    let result = change(&mut tokens)?;
    tokens.save(store)?;
    Ok(result)
}

/// A passwd-derived home cannot be redirected with HOME/RHO_HOME/TMPDIR.
/// Its owner-only write access prevents another uid planting the lock file.
fn lock_path_in(home: &Path, user: libc::uid_t) -> anyhow::Result<PathBuf> {
    let home = home
        .canonicalize()
        .context("could not resolve the account home")?;
    let metadata = home.metadata()?;
    if !metadata.is_dir() || metadata.uid() != user || metadata.mode() & 0o022 != 0 {
        anyhow::bail!(
            "MCP token lock home {} must be a directory owned by uid {user} with no group or other write access",
            home.display()
        );
    }
    Ok(home.join(".rho-mcp-tokens.lock"))
}

/// Resolve the OS account, not the environment-dependent notion of home.
fn account_home(user: libc::uid_t) -> anyhow::Result<PathBuf> {
    // SAFETY: sysconf has no memory-safety preconditions.
    let suggested = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    // NSS supplies a recommended size where supported; otherwise begin with
    // the record's size and grow on ERANGE, without an invented size limit.
    let mut buffer = vec![
        0_u8;
        usize::try_from(suggested)
            .unwrap_or_default()
            .max(std::mem::size_of::<libc::passwd>())
    ];
    loop {
        let mut record = MaybeUninit::<libc::passwd>::uninit();
        let mut found = std::ptr::null_mut();
        // SAFETY: record, buffer, and found are writable for their declared
        // sizes and remain live throughout this reentrant lookup.
        let status = unsafe {
            libc::getpwuid_r(
                user,
                record.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut found,
            )
        };
        if status == libc::ERANGE {
            buffer.try_reserve(buffer.len())?;
            buffer.resize(buffer.capacity(), 0);
            continue;
        }
        if status != 0 {
            return Err(std::io::Error::from_raw_os_error(status))
                .context("could not look up the account home for the MCP token lock");
        }
        if found.is_null() {
            anyhow::bail!("no OS account for uid {user}; cannot locate the MCP token lock");
        }
        // SAFETY: a successful non-null result initialized record; pw_dir
        // is a nul-terminated field in the still-live lookup buffer.
        let record = unsafe { record.assume_init() };
        if record.pw_dir.is_null() {
            anyhow::bail!("OS account for uid {user} has no home directory");
        }
        // SAFETY: pw_dir's non-null field is valid until buffer is dropped.
        let home = unsafe { CStr::from_ptr(record.pw_dir) };
        let home = PathBuf::from(OsStr::from_bytes(home.to_bytes()));
        if !home.is_absolute() {
            anyhow::bail!("OS account for uid {user} has a non-absolute home directory");
        }
        return Ok(home);
    }
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
