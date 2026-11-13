//! A lock for state that belongs to the OS account, not to a rho home: a
//! 0600 file in the home the passwd database gives the effective uid.

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

/// Where the lock file `file_name` of the OS account this process runs as
/// is: in the account's own home, which no environment variable redirects.
/// `what` names what it guards, in errors.
pub(crate) fn account_lock_path(file_name: &str, what: &str) -> anyhow::Result<PathBuf> {
    // The OS account is per uid, so neither env overrides nor the presence
    // of a logind runtime directory may select a different mutation lock.
    // SAFETY: geteuid has no arguments or memory-safety preconditions.
    let user = unsafe { libc::geteuid() };
    lock_path_in(&account_home(user, what)?, user, file_name, what)
}

/// A passwd-derived home cannot be redirected with HOME/RHO_HOME/TMPDIR.
/// Its owner-only write access prevents another uid planting the lock file.
pub(super) fn lock_path_in(
    home: &Path,
    user: libc::uid_t,
    file_name: &str,
    what: &str,
) -> anyhow::Result<PathBuf> {
    let home = home
        .canonicalize()
        .context("could not resolve the account home")?;
    let metadata = home.metadata()?;
    if !metadata.is_dir() || metadata.uid() != user || metadata.mode() & 0o022 != 0 {
        anyhow::bail!(
            "{what} lock home {} must be a directory owned by uid {user} with no group or other write access",
            home.display()
        );
    }
    Ok(home.join(file_name))
}

/// Resolve the OS account, not the environment-dependent notion of home.
fn account_home(user: libc::uid_t, what: &str) -> anyhow::Result<PathBuf> {
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
            return Err(std::io::Error::from_raw_os_error(status)).with_context(|| {
                format!("could not look up the account home for the {what} lock")
            });
        }
        if found.is_null() {
            anyhow::bail!("no OS account for uid {user}; cannot locate the {what} lock");
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

/// Opens the account lock file `path`, creating it private, and takes its
/// exclusive lock, waiting for another holder.
pub(crate) fn lock_account_file(path: &Path, what: &str) -> anyhow::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .with_context(|| format!("could not open {what} mutation lock {}", path.display()))?;
    let metadata = file.metadata()?;
    // SAFETY: geteuid has no arguments or memory-safety preconditions.
    let user = unsafe { libc::geteuid() };
    if !metadata.is_file() || metadata.uid() != user || metadata.mode() & 0o077 != 0 {
        anyhow::bail!(
            "{what} mutation lock {} must be a regular file owned by uid {user} with no group or other access",
            path.display()
        );
    }
    fs2::FileExt::lock_exclusive(&file)
        .with_context(|| format!("could not lock {what} mutations at {}", path.display()))?;
    Ok(file)
}

#[cfg(test)]
#[path = "account_tests.rs"]
mod tests;
