//! Sanitized base environment for host-spawned children.
//!
//! Children that must not inherit provider credentials or ambient secrets start
//! from this fixed base set. Callers add their own allowlists and overlays on
//! top. Hooks and MCP stdio share this contract so the two cannot drift.

use std::collections::BTreeMap;

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

/// Names in the documented base set.
pub(crate) fn base_names() -> &'static [&'static str] {
    BASE_NAMES
}

/// Builds a map of base names that are set in the parent via `read`.
pub(crate) fn collect_base<F>(mut read: F) -> BTreeMap<String, String>
where
    F: FnMut(&str) -> Option<String>,
{
    BASE_NAMES
        .iter()
        .copied()
        .filter_map(|name| read(name).map(|value| (name.to_owned(), value)))
        .collect()
}

/// Clears the child environment and installs only the base set from the parent.
pub(crate) fn apply_base(command: &mut tokio::process::Command) {
    command.env_clear();
    for name in BASE_NAMES {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
}
