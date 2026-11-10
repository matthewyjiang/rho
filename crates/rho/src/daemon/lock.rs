//! Single-instance lock and the info file that names the running daemon.
//!
//! The daemon holds an exclusive `flock` on [`DaemonPaths::lock`] for its
//! whole lifetime; the kernel releases it when the process exits, however it
//! exits. Holding the lock therefore proves no other daemon is alive, which
//! is what makes removing a leftover socket safe.
//!
//! The lock and the info file live in the rho home's state directory, not the
//! runtime directory, so they guard the home's daemon state whichever runtime
//! directory a daemon resolved. A home's runtime directory is its own (see
//! [`DaemonPaths::resolve`]), so the lock also covers the socket.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::{
        fd::{AsFd, BorrowedFd},
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use fs2::FileExt;
use rho_protocol::ProtocolVersion;
use serde::{Deserialize, Serialize};

use super::paths::{DaemonPathError, DaemonPaths};

/// Proof that this process is the only daemon for its rho home.
/// Dropping it releases the lock.
#[derive(Debug)]
pub(crate) struct DaemonLock {
    file: File,
}

/// The contents of [`DaemonPaths::info_file`]: who holds the lock.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DaemonRecord {
    pub(crate) pid: u32,
    /// The daemon's rho version.
    pub(crate) version: String,
    pub(crate) protocol: ProtocolVersion,
    /// Unix time, in seconds, when the daemon started.
    pub(crate) started_at: u64,
    pub(crate) socket: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum LockError {
    /// Another daemon holds the lock. `record` is `None` while that daemon is
    /// between taking the lock and writing its info file.
    #[error("{}", already_running_message(.record.as_ref()))]
    AlreadyRunning { record: Option<DaemonRecord> },
    #[error("could not open rho daemon lock {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error(transparent)]
    StateDir(#[from] DaemonPathError),
}

fn already_running_message(record: Option<&DaemonRecord>) -> String {
    match record {
        Some(DaemonRecord {
            pid,
            version,
            socket,
            ..
        }) => format!(
            "rho daemon is already running (pid {pid}, version {version}, socket {})",
            socket.display()
        ),
        None => "rho daemon is already running (it is still starting)".to_owned(),
    }
}

/// Takes the daemon lock without blocking, after creating or verifying the
/// private state directory that holds it.
pub(crate) fn acquire(paths: &DaemonPaths) -> Result<DaemonLock, LockError> {
    paths.prepare_state_dir()?;
    let file = open_lock_file(&paths.lock)?;
    match FileExt::try_lock_exclusive(&file) {
        Ok(()) => Ok(DaemonLock { file }),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Err(LockError::AlreadyRunning {
            record: read_record(paths),
        }),
        Err(source) => Err(LockError::Io {
            path: paths.lock.clone(),
            source,
        }),
    }
}

/// Takes over the lock the daemon this process re-executed from handed it
/// as `file` (see `handoff`), once `file` proves to be this home's lock
/// file, still locked.
pub(super) fn adopt(paths: &DaemonPaths, file: File) -> io::Result<DaemonLock> {
    let inherited = file.metadata()?;
    let on_disk = fs::symlink_metadata(&paths.lock)?;
    if (inherited.dev(), inherited.ino()) != (on_disk.dev(), on_disk.ino()) {
        return Err(io::Error::other(format!(
            "the inherited lock is not the daemon lock {}",
            paths.lock.display()
        )));
    }
    // The lock belongs to the open file description, which the exec kept,
    // so locking it again succeeds at once; it would block only if another
    // daemon had taken the lock meanwhile.
    FileExt::try_lock_exclusive(&file)?;
    Ok(DaemonLock { file })
}

/// The lock's descriptor, which `handoff` keeps open across the exec.
impl AsFd for DaemonLock {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.file.as_fd()
    }
}

/// Whether a daemon currently holds the lock. Probes with a shared lock, so
/// it never blocks a daemon that is starting for longer than the probe.
pub(crate) fn is_held(paths: &DaemonPaths) -> Result<bool, LockError> {
    let file = match open_lock_file(&paths.lock) {
        Ok(file) => file,
        Err(LockError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            return Ok(false)
        }
        Err(error) => return Err(error),
    };
    match FileExt::try_lock_shared(&file) {
        Ok(()) => Ok(false),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(true),
        Err(source) => Err(LockError::Io {
            path: paths.lock.clone(),
            source,
        }),
    }
}

/// Polls until no daemon holds the lock. Returns the time waited, or `None`
/// if the lock was still held at `timeout`.
pub(crate) async fn wait_until_released(
    paths: &DaemonPaths,
    timeout: Duration,
) -> Result<Option<Duration>, LockError> {
    let started = Instant::now();
    loop {
        if !is_held(paths)? {
            return Ok(Some(started.elapsed()));
        }
        if started.elapsed() >= timeout {
            return Ok(None);
        }
        tokio::time::sleep(LOCK_POLL_INTERVAL).await;
    }
}

/// Interval between lock probes while waiting for a daemon to exit. Each probe
/// is an open and a `flock` on a local file; shutdown removes two files.
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Writes the info file atomically (0600) so readers never see a partial one.
pub(crate) fn write_record(paths: &DaemonPaths, record: &DaemonRecord) -> io::Result<()> {
    // `tempfile` creates the file with mode 0600.
    let mut file = tempfile::NamedTempFile::new_in(&paths.state_dir)?;
    serde_json::to_writer(&mut file, record)?;
    file.write_all(b"\n")?;
    file.persist(&paths.info_file)
        .map_err(|error| error.error)?;
    Ok(())
}

/// The info file, if it exists and parses.
pub(crate) fn read_record(paths: &DaemonPaths) -> Option<DaemonRecord> {
    let contents = fs::read(&paths.info_file).ok()?;
    serde_json::from_slice(&contents).ok()
}

/// Who holds the daemon lock now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LockHolder {
    /// No daemon runs.
    Nobody,
    /// A daemon between taking the lock and writing its record.
    Starting,
    Daemon(DaemonRecord),
}

impl LockHolder {
    /// The running daemon's record, once it wrote one.
    pub(crate) fn record(self) -> Option<DaemonRecord> {
        match self {
            Self::Daemon(record) => Some(record),
            Self::Nobody | Self::Starting => None,
        }
    }
}

/// Who holds the daemon lock now. The record is read only while the lock is
/// held: a record alone may be a crashed daemon's, whose pid may have been
/// reused. A lock that cannot be probed counts as held by nobody.
pub(crate) fn holder(paths: &DaemonPaths) -> LockHolder {
    if !is_held(paths).unwrap_or(false) {
        return LockHolder::Nobody;
    }
    read_record(paths).map_or(LockHolder::Starting, LockHolder::Daemon)
}

/// Sends SIGTERM to the recorded daemon, for a daemon that cannot take a
/// `rho/daemon/stop` request because it speaks an incompatible protocol.
/// SIGTERM triggers the same graceful shutdown. Returns the signalled pid, or
/// `None` when no daemon holds the lock.
pub(crate) fn terminate_recorded_daemon(paths: &DaemonPaths) -> anyhow::Result<Option<u32>> {
    // Only signal while the lock is held: the record then names a live
    // daemon rather than a crashed one whose pid may have been reused.
    if !is_held(paths)? {
        return Ok(None);
    }
    let record = read_record(paths)
        .ok_or_else(|| anyhow::anyhow!("rho daemon is still starting; retry in a moment"))?;
    let pid = libc::pid_t::try_from(record.pid)?;
    // SAFETY: `kill` takes no pointers and has no memory-safety preconditions.
    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(Some(record.pid))
}

fn open_lock_file(path: &Path) -> Result<File, LockError> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|source| LockError::Io {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(test)]
#[path = "lock_tests.rs"]
mod tests;
