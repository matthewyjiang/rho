//! Single-writer leases on session files.
//!
//! One Rho process writes a session file at a time. Opening a session takes
//! an exclusive lock (`flock` on Unix) on `.{id}.active.lock` in its
//! workspace's session directory, and deleting one takes the same lock. So
//! another process can neither open nor delete a session this one has open,
//! nor open one this one is deleting. The OS drops the lock when its holder
//! exits, so a crashed process leaves only an unlocked file, and nothing
//! reads the file's existence as a hold.
//!
//! A lock taken twice conflicts even within one process, because each open
//! file handle holds its own. So a process locks each session once and
//! counts its holders in [`HELD`]: another open of a session this process
//! already has open (an ACP `session/load` replacing its live session, the
//! TUI's `/resume` of its current session) shares that lock, and the last
//! holder to close releases it.

use std::{
    collections::{btree_map::Entry, BTreeMap},
    fmt,
    fs::{File, OpenOptions, TryLockError},
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
};

use super::{delete::short_id, layout};

/// The session locks this process holds, by lock file path.
static HELD: Mutex<BTreeMap<PathBuf, HeldLock>> = Mutex::new(BTreeMap::new());

/// One session lock this process holds.
struct HeldLock {
    file: File,
    purpose: LeasePurpose,
    /// The [`SessionLease`]s sharing the lock. A delete never shares it.
    holders: usize,
}

/// What a session lease is taken for.
#[derive(Clone, Copy, Debug)]
pub(super) enum LeasePurpose {
    /// Opening the session, to read and write its file.
    Open,
    /// Deleting the session's files.
    Delete,
}

/// A held session lease. Dropping the last one of a session unlocks
/// explicitly before closing the file: a child process spawned elsewhere in
/// Rho shares the open file description from fork until its exec closes it
/// (Rust opens every file close-on-exec), so closing the handle alone could
/// keep the session looking open.
#[derive(Debug)]
pub(crate) struct SessionLease {
    path: PathBuf,
}

/// Refusal to open a session another Rho process has open: two processes
/// writing one session file would interleave its records. A process
/// deleting the session holds the same lock, for as long as that takes.
#[derive(Debug)]
pub(crate) struct SessionInUse {
    id: String,
}

impl fmt::Display for SessionInUse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "session '{}' is open in another Rho process; close it there first",
            short_id(&self.id)
        )
    }
}

impl std::error::Error for SessionInUse {}

/// The lock file of session `id` saved under `cwd`'s workspace.
fn lock_path(session_root: &Path, cwd: &Path, id: &str) -> PathBuf {
    layout::session_dir_in_root(session_root, cwd).join(format!(".{id}.active.lock"))
}

/// Takes the lease on session `id` for `purpose`, or refuses when another
/// open or delete holds the session, in this process or another.
pub(super) fn acquire(
    session_root: &Path,
    cwd: &Path,
    id: &str,
    purpose: LeasePurpose,
) -> anyhow::Result<SessionLease> {
    let path = lock_path(session_root, cwd, id);
    let mut held = HELD.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(lock) = held.get_mut(&path) {
        match (lock.purpose, purpose) {
            (LeasePurpose::Open, LeasePurpose::Open) => {
                lock.holders += 1;
                return Ok(SessionLease { path });
            }
            (LeasePurpose::Open, LeasePurpose::Delete) => anyhow::bail!(
                "refusing to delete open session '{}'; close it first",
                short_id(id)
            ),
            (LeasePurpose::Delete, _) => anyhow::bail!(
                "session '{}' is being deleted; refresh the session list",
                short_id(id)
            ),
        }
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)?;
    layout::set_private_file_permissions(&file)?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            return Err(match purpose {
                LeasePurpose::Open => SessionInUse { id: id.to_owned() }.into(),
                LeasePurpose::Delete => anyhow::anyhow!(
                    "refusing to delete active session '{}'; close it in the other Rho process first",
                    short_id(id)
                ),
            });
        }
        Err(TryLockError::Error(error)) => return Err(error.into()),
    }
    held.insert(
        path.clone(),
        HeldLock {
            file,
            purpose,
            holders: 1,
        },
    );
    Ok(SessionLease { path })
}

impl SessionLease {
    /// A duplicate of the leased handle, standing in for one a child process
    /// inherits between fork and exec.
    #[cfg(test)]
    pub(crate) fn inherited_handle_for_tests(&self) -> File {
        let held = HELD.lock().unwrap_or_else(PoisonError::into_inner);
        held[&self.path].file.try_clone().unwrap()
    }
}

impl Drop for SessionLease {
    fn drop(&mut self) {
        let mut held = HELD.lock().unwrap_or_else(PoisonError::into_inner);
        let Entry::Occupied(mut lock) = held.entry(std::mem::take(&mut self.path)) else {
            return;
        };
        lock.get_mut().holders -= 1;
        if lock.get().holders == 0 {
            let _ = lock.remove().file.unlock();
        }
    }
}

#[cfg(test)]
#[path = "lease_tests.rs"]
mod tests;
