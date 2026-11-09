//! Sanitized base environment for host-spawned children.
//!
//! Children that must not inherit provider credentials or ambient secrets start
//! from this fixed base set. Callers add their own allowlists and overlays on
//! top. Hooks and MCP stdio share this contract so the two cannot drift.

use std::ffi::OsString;

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

/// Names in the documented base set, part of the hook spawn contract that
/// diagnostics print before a project is trusted.
pub(crate) fn base_names() -> &'static [&'static str] {
    BASE_NAMES
}

/// Clears the child environment and installs only the base names that `read`
/// finds, such as a session's (see [`crate::session_environment::Ambient`]).
/// A base name `read` does not find is absent from the child, never invented.
pub(crate) fn apply_base<F>(command: &mut tokio::process::Command, mut read: F)
where
    F: FnMut(&str) -> Option<OsString>,
{
    command.env_clear();
    for name in BASE_NAMES {
        if let Some(value) = read(name) {
            command.env(name, value);
        }
    }
}
