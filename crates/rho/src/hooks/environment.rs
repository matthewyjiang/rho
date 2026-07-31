//! The exact environment a hook program receives.
//!
//! A hook child never inherits the ambient environment. It gets a fixed base set
//! plus the names the hook allowlisted, and nothing else, so provider
//! credentials and unrelated secrets cannot reach a hook program by accident.

use std::collections::BTreeMap;

/// Variables every hook child receives, on every platform.
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

/// Names in the documented base set, part of the spawn contract.
///
/// [`super::IN_HOOK_ENV`] is always set as well; it is Rho's own marker rather
/// than an inherited value, so it is listed separately in the contract.
pub fn base_environment_names() -> &'static [&'static str] {
    BASE_NAMES
}

/// Builds the child environment from `read`, which supplies ambient values.
///
/// Variables that are unset in the parent are simply absent from the child. A
/// hook that needs one must handle its absence; the runtime does not invent a
/// value or fail the spawn, because a missing optional token is a hook concern.
pub fn child_environment<F>(allowlist: &[String], mut read: F) -> BTreeMap<String, String>
where
    F: FnMut(&str) -> Option<String>,
{
    let mut environment: BTreeMap<String, String> = BASE_NAMES
        .iter()
        .copied()
        .map(str::to_owned)
        .chain(allowlist.iter().cloned())
        .filter_map(|name| read(&name).map(|value| (name, value)))
        .collect();
    // Stops a hook that runs `rho` from re-entering the hooks that invoked it.
    environment.insert(super::IN_HOOK_ENV.to_owned(), "1".into());
    environment
}

/// Reads the ambient environment for [`child_environment`].
pub fn ambient(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

#[cfg(test)]
#[path = "environment_tests.rs"]
mod tests;
