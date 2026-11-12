//! Sanitized base environment for host-spawned children.
//!
//! Children that must not inherit provider credentials or ambient secrets start
//! from this fixed base set. Callers add their own allowlists and overlays on
//! top. Hooks and MCP stdio share this contract so the two cannot drift.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

/// Variables every sanitized child receives, on every platform.
#[cfg(unix)]
const BASE_NAMES: &[&str] = &["PATH", "HOME", "LANG", "LC_ALL", "LC_CTYPE", "TZ"];

/// Windows equivalents. `SystemRoot` and `ComSpec` are required by the loader
/// and by most interpreters; the rest keep path and locale behavior sane.
#[cfg(windows)]
const BASE_NAMES: &[&str] = &[
    "PATH",
    "SystemRoot",
    "SystemDrive",
    "ComSpec",
    "PATHEXT",
    "TEMP",
    "TMP",
    "USERPROFILE",
];

// Together they locate the daemon serving the session (see [`apply_base`]).
const RHO_HOME_NAME: &str = "RHO_HOME";
const RUNTIME_DIR_NAME: &str = "XDG_RUNTIME_DIR";

/// Names [`apply_base`] may give a child, part of the hook spawn contract
/// that diagnostics print before a project is trusted.
pub(crate) fn base_names() -> impl Iterator<Item = &'static str> {
    BASE_NAMES
        .iter()
        .copied()
        .chain([RHO_HOME_NAME, RUNTIME_DIR_NAME])
}

/// This process's rho home, as the daemon's spawner pins it: canonical, so
/// it names the same directory from a child's working directory, and created
/// first when it is missing. `None` when it does not resolve; only a nested
/// `rho` reads it, so that keeps no child from starting.
pub(crate) fn rho_home() -> Option<PathBuf> {
    crate::paths::rho_dir()
        .ok()
        .and_then(|home| crate::daemon::real_home(&home).ok())
}

/// Clears the child environment and installs only the base names that `read`
/// finds, such as a session's (see [`crate::session_environment::Ambient`]).
/// A base name `read` does not find is absent from the child, never invented.
///
/// Then sets `RHO_HOME` to `rho_home` (callers pass [`rho_home`]; `None`
/// leaves it unset) and passes on the `XDG_RUNTIME_DIR` `read` finds, which
/// together locate the daemon serving the session, so a nested `rho` reaches
/// it rather than the daemon of the default home or runtime directory. A
/// client whose `XDG_RUNTIME_DIR` differs from the daemon's could not have
/// reached it, so the session's is the daemon's.
pub(crate) fn apply_base<F>(
    command: &mut tokio::process::Command,
    rho_home: Option<&Path>,
    mut read: F,
) where
    F: FnMut(&str) -> Option<OsString>,
{
    command.env_clear();
    for name in BASE_NAMES.iter().copied().chain([RUNTIME_DIR_NAME]) {
        if let Some(value) = read(name) {
            command.env(name, value);
        }
    }
    if let Some(rho_home) = rho_home {
        command.env(RHO_HOME_NAME, rho_home);
    }
}
