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
//! holder to close releases it. Sharing the lock does not share the writer:
//! each open has its own append cursor, so a host retires the session it
//! replaces before the replacement writes.
//!
//! Locks go through fs2 rather than `std::fs::File::try_lock`, which std
//! does not implement on Android.

use std::{
    collections::{btree_map::Entry, BTreeMap},
    fmt,
    fs::{File, OpenOptions},
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
};

use super::{id_match::short_id, layout};

/// The session locks this process holds, by lock file path.
static HELD: Mutex<BTreeMap<PathBuf, HeldLock>> = Mutex::new(BTreeMap::new());

/// One session lock this process holds.
struct HeldLock {
    file: File,
    hold: Hold,
}

/// What this process holds a session's lock for.
#[derive(Clone, Copy)]
enum Hold {
    /// Open, shared by `holders` [`SessionLease`]s.
    Open { holders: NonZeroUsize },
    /// Being deleted, by the one lease that never shares it.
    Delete,
}

/// What a session lease is taken for.
#[derive(Clone, Copy, Debug)]
pub(super) enum LeasePurpose {
    /// Opening the session, to read and write its file.
    Open,
    /// Deleting the session's files.
    Delete,
}

/// A held session lease. Cloning one shares the lock. Dropping the last one
/// of a session unlocks explicitly before closing the file: a child process
/// spawned elsewhere in Rho shares the open file description from fork until
/// its exec closes it (Rust opens every file close-on-exec), so closing the
/// handle alone could keep the session looking open.
#[derive(Debug)]
pub(crate) struct SessionLease {
    path: PathBuf,
}

/// Refusal to open or delete a session something else holds. Two writers
/// would interleave the file's records, and a delete would pull the file
/// out from under an open.
#[derive(Debug)]
pub(crate) struct SessionInUse {
    id: String,
    by: InUseBy,
}

/// What holds a session a lease was refused for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InUseBy {
    /// Another Rho process has it open or is deleting it.
    OtherProcess,
    /// This process has it open; refuses a delete.
    OpenHere,
    /// This process is deleting it.
    DeletingHere,
}

impl SessionInUse {
    #[cfg(test)]
    pub(crate) fn by(&self) -> InUseBy {
        self.by
    }
}

impl fmt::Display for SessionInUse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let id = short_id(&self.id);
        match self.by {
            InUseBy::OtherProcess => write!(
                formatter,
                "session '{id}' is open in another Rho process; close it there first"
            ),
            InUseBy::OpenHere => write!(formatter, "session '{id}' is open; close it first"),
            InUseBy::DeletingHere => write!(
                formatter,
                "session '{id}' is being deleted; refresh the session list"
            ),
        }
    }
}

impl std::error::Error for SessionInUse {}

fn lock_file_name(id: &str) -> String {
    format!(".{id}.active.lock")
}

/// The lock file of session `id` saved under `cwd`'s workspace.
fn lock_path(session_root: &Path, cwd: &Path, id: &str) -> PathBuf {
    layout::session_dir_in_root(session_root, cwd).join(lock_file_name(id))
}

/// Takes the lease on session `id` for `purpose`, or refuses with
/// [`SessionInUse`] when another open or delete holds the session, in this
/// process or another.
pub(super) fn acquire(
    session_root: &Path,
    cwd: &Path,
    id: &str,
    purpose: LeasePurpose,
) -> anyhow::Result<SessionLease> {
    let path = lock_path(session_root, cwd, id);
    let in_use = |by| SessionInUse {
        id: id.to_owned(),
        by,
    };
    let mut held = HELD.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(lock) = held.get_mut(&path) {
        return match (lock.hold, purpose) {
            (Hold::Open { holders }, LeasePurpose::Open) => {
                lock.hold = Hold::Open {
                    holders: holders.saturating_add(1),
                };
                Ok(SessionLease { path })
            }
            (Hold::Open { .. }, LeasePurpose::Delete) => Err(in_use(InUseBy::OpenHere).into()),
            (Hold::Delete, _) => Err(in_use(InUseBy::DeletingHere).into()),
        };
    }
    let dir = layout::ensure_session_dir(session_root, cwd)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join(lock_file_name(id)))?;
    layout::set_private_file_permissions(&file)?;
    if let Err(error) = fs2::FileExt::try_lock_exclusive(&file) {
        // fs2 reports contention as `WouldBlock` on Unix and as the raw
        // lock-violation error on Windows.
        let contended = error.kind() == std::io::ErrorKind::WouldBlock
            || error.raw_os_error() == fs2::lock_contended_error().raw_os_error();
        return Err(if contended {
            in_use(InUseBy::OtherProcess).into()
        } else {
            error.into()
        });
    }
    let hold = match purpose {
        LeasePurpose::Open => Hold::Open {
            holders: NonZeroUsize::MIN,
        },
        LeasePurpose::Delete => Hold::Delete,
    };
    held.insert(path.clone(), HeldLock { file, hold });
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

impl Clone for SessionLease {
    fn clone(&self) -> Self {
        let mut held = HELD.lock().unwrap_or_else(PoisonError::into_inner);
        let lock = held
            .get_mut(&self.path)
            .expect("a live lease's lock is held");
        match lock.hold {
            Hold::Open { holders } => {
                lock.hold = Hold::Open {
                    holders: holders.saturating_add(1),
                };
            }
            Hold::Delete => unreachable!("a delete lease is never cloned"),
        }
        Self {
            path: self.path.clone(),
        }
    }
}

impl Drop for SessionLease {
    fn drop(&mut self) {
        let mut held = HELD.lock().unwrap_or_else(PoisonError::into_inner);
        let Entry::Occupied(mut lock) = held.entry(std::mem::take(&mut self.path)) else {
            debug_assert!(false, "a live lease's lock is held");
            return;
        };
        let remaining = match lock.get().hold {
            Hold::Open { holders } => NonZeroUsize::new(holders.get() - 1),
            Hold::Delete => None,
        };
        match remaining {
            Some(holders) => lock.get_mut().hold = Hold::Open { holders },
            None => {
                let _ = fs2::FileExt::unlock(&lock.remove().file);
            }
        }
    }
}

#[cfg(test)]
#[path = "lease_tests.rs"]
mod tests;
