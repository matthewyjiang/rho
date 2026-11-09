//! The rho binary the daemon runs, so it can tell once an upgrade replaced
//! it and re-execute the replacement in place (#1713).
//!
//! The daemon watches the path it was started from, not the file it runs:
//! installers replace a binary by writing a new file over its path, a
//! rename, which leaves the running process on the old file. A service
//! unit's `ExecStart` keeps the path it names, so a symlink an installer
//! switches is noticed there too; a daemon a client started runs the
//! client's binary with symlinks resolved, so it notices only a new file at
//! that resolved path. Once the path holds a file other than the one it held
//! at startup and other than the running one, that file is a replacement,
//! which the daemon runs with `--version` before trusting it, so a
//! half-installed or foreign binary is never executed in its place.

use std::{
    fs, io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

/// How long a replacement may take to print its version.
//
// Tripwire. Measured with the debug binary, 20 runs each: `rho --version`
// exits in 8 ms median (9 ms max) idle, and 32 ms median (45 ms max) with
// all 12 cores saturated. A binary that takes longer is hung or not rho;
// the daemon then keeps running as it is.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// The binary the daemon runs, by the path it was started from.
#[derive(Debug)]
pub(crate) struct Executable {
    path: PathBuf,
    /// The file `path` named as the daemon started, if it could be read.
    started: Option<FileIdentity>,
    /// The file the process runs. Differs from `started` when a launcher
    /// started it under another name, which must not read as a replacement
    /// at every check.
    running: Option<FileIdentity>,
}

/// Which file a path names: equal identities are the same file, unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    device: u64,
    inode: u64,
    size: u64,
    modified_secs: i64,
    modified_nanos: i64,
}

impl FileIdentity {
    fn of(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.size(),
            modified_secs: metadata.mtime(),
            modified_nanos: metadata.mtime_nsec(),
        }
    }
}

/// A file at the daemon's path other than the binary it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Replacement {
    pub(crate) identity: FileIdentity,
}

/// Why a replacement did not prove itself rho.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ProbeError {
    #[error("could not run it: {0}")]
    Spawn(io::Error),
    #[error("it did not print its version within PROBE_TIMEOUT ({PROBE_TIMEOUT:?})")]
    TimedOut,
    #[error("`--version` failed with {0}")]
    Failed(std::process::ExitStatus),
    #[error("`--version` printed {0:?}, not a rho version")]
    NotRho(String),
}

impl ProbeError {
    /// Whether probing the same file again may succeed: it could not run
    /// at that moment, as while an installer still holds it open, or the
    /// host was too loaded for it to answer in time.
    pub(crate) fn is_transient(&self) -> bool {
        match self {
            Self::Spawn(_) | Self::TimedOut => true,
            Self::Failed(_) | Self::NotRho(_) => false,
        }
    }
}

impl Executable {
    /// This process's binary. The path is `argv[0]` when it names one, as a
    /// service manager's `ExecStart` or a re-execution does, so a symlink an
    /// installer switches stays the path watched; otherwise the binary the
    /// OS reports.
    pub(crate) fn of_this_process() -> io::Result<Self> {
        let path = match std::env::args_os().next().map(PathBuf::from) {
            Some(path) if path.components().count() > 1 => std::path::absolute(path)?,
            _ => std::env::current_exe()?,
        };
        let started = identity(&path);
        Ok(Self {
            running: running_identity().or(started),
            started,
            path,
        })
    }

    /// The path the daemon re-executes.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// The file at the daemon's path, once it is neither the file the path
    /// named at startup nor the binary running. A path that names nothing,
    /// as midway through a reinstall, has no replacement yet, and one that
    /// could not be read at startup never has one.
    pub(crate) fn replacement(&self) -> Option<Replacement> {
        let started = self.started?;
        let identity = identity(&self.path)?;
        (identity != started && Some(identity) != self.running).then_some(Replacement { identity })
    }

    /// Runs the binary at the daemon's path with `--version` and returns
    /// the rho version it prints.
    pub(crate) async fn probe(&self) -> Result<String, ProbeError> {
        let child = tokio::process::Command::new(&self.path)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(ProbeError::Spawn)?;
        let output = tokio::time::timeout(PROBE_TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| ProbeError::TimedOut)?
            .map_err(ProbeError::Spawn)?;
        if !output.status.success() {
            return Err(ProbeError::Failed(output.status));
        }
        let printed = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        match printed.strip_prefix("rho ") {
            Some(version) if !version.is_empty() => Ok(version.to_owned()),
            _ => Err(ProbeError::NotRho(printed)),
        }
    }
}

/// The file `path` names now, following symlinks.
fn identity(path: &Path) -> Option<FileIdentity> {
    fs::metadata(path)
        .ok()
        .map(|metadata| FileIdentity::of(&metadata))
}

/// The binary this process runs, even once an installer replaced the path
/// it started from. Only Linux names it; elsewhere the daemon takes the
/// path's file at startup.
fn running_identity() -> Option<FileIdentity> {
    #[cfg(target_os = "linux")]
    return identity(Path::new("/proc/self/exe"));
    #[cfg(not(target_os = "linux"))]
    None
}
