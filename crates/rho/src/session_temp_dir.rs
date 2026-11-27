//! Each daemon session's own temp directory, which its tools' processes get
//! as their temp directory (`TMPDIR`; `TMP` and `TEMP` on Windows), so
//! sessions that write scratch files, with `mktemp` or by name, never write
//! each other's.
//!
//! It is `<temp root>/rho-<uid>/<session key>` (`rho\<session key>` on
//! Windows, whose temp dir is the user's own), the same on every opening,
//! so a session finds what it wrote in an earlier turn after the daemon
//! unloaded and reopened it. The OS clears it, as it does the rest of its
//! temp dir: rho removes none, since a session that sat idle is unloaded
//! within minutes and its next turn may still need what an earlier one
//! wrote. Each prompt makes it again if a cleaner removed it meanwhile.
//!
//! The path is kept short: a Unix socket made under it, such as one a test
//! harness or `ssh-agent` binds in a `mktemp -d` directory, must fit the
//! platform's limit of about 100 bytes.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use rho_sdk::SessionId;
use sha2::{Digest, Sha256};

/// A session's temp directory, private to this user, whose path is UTF-8 so
/// a client's environment can name it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionTempDir {
    dir: Arc<str>,
    /// The per-user directory `dir` is in, checked with it.
    parent: Arc<Path>,
}

impl SessionTempDir {
    /// Session `session`'s temp directory under `root`, made private to this
    /// user, or checked if an earlier opening made it.
    pub(crate) fn prepare(root: &Path, session: &SessionId) -> anyhow::Result<Self> {
        let parent = root.join(parent_name());
        let dir = parent
            .join(session_key(session))
            .into_os_string()
            .into_string()
            .map_err(|dir| anyhow::anyhow!("{} is not UTF-8", Path::new(&dir).display()))?;
        let temp_dir = Self {
            dir: dir.into(),
            parent: parent.into(),
        };
        temp_dir.ensure()?;
        Ok(temp_dir)
    }

    /// Makes the directory again if a temp cleaner removed it, and checks
    /// that it and its parent are still this user's alone.
    pub(crate) fn ensure(&self) -> anyhow::Result<()> {
        crate::daemon::prepare_private_dir(&self.parent)?;
        crate::daemon::prepare_private_dir(Path::new(&*self.dir))?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        Path::new(&*self.dir)
    }

    /// The variables that make this directory a process's temp directory.
    pub(crate) fn variables(&self) -> impl Iterator<Item = (String, String)> + '_ {
        TEMP_VARIABLES
            .iter()
            .map(|name| ((*name).to_owned(), self.dir.to_string()))
    }
}

/// Where session temp directories are made: the daemon's temp dir, except
/// on macOS, whose per-user `/var/folders/…/T` is long enough that a socket
/// path under a session's directory would pass the limit.
pub(crate) fn temp_root() -> PathBuf {
    if cfg!(target_os = "macos") {
        PathBuf::from("/tmp")
    } else {
        std::env::temp_dir()
    }
}

/// The name of `session`'s directory: the first 16 hex digits of the
/// SHA-256 of its id. Short, and a plain file name whatever the id holds.
fn session_key(session: &SessionId) -> String {
    hex::encode(&Sha256::digest(session.as_str().as_bytes())[..8])
}

/// The variables a process finds its temp directory by: `TMPDIR` for
/// `mktemp` and every Unix tool, `TMP` and `TEMP` for Windows'
/// `GetTempPath` and PowerShell.
#[cfg(unix)]
const TEMP_VARIABLES: &[&str] = &["TMPDIR"];
#[cfg(not(unix))]
const TEMP_VARIABLES: &[&str] = &["TMP", "TEMP"];

/// The directory under the temp root that holds this user's session temp
/// directories: one per uid, since every user shares `/tmp`.
#[cfg(unix)]
fn parent_name() -> String {
    // SAFETY: geteuid has no arguments or memory-safety preconditions.
    format!("rho-{}", unsafe { libc::geteuid() })
}

/// Windows gives each user a temp dir of their own.
#[cfg(not(unix))]
fn parent_name() -> String {
    "rho".to_owned()
}

#[cfg(test)]
#[path = "session_temp_dir_tests.rs"]
mod tests;
