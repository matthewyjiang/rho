//! Locate and launch `agy_acp_server`, Antigravity's ACP server.
//!
//! It is a separate download from the `agy` CLI (ACP registry entry
//! `antigravity-acp`), shipped with a sibling `localharness_external`. The
//! server looks for the harness next to `abspath(argv[0])` without resolving
//! symlinks, which fails for a PATH symlink and for frozen workflow launches
//! (exec through `/proc/self/fd/N`). Its `ANTIGRAVITY_HARNESS_PATH` override
//! wins over that search, so Rho sets it from the canonical server path.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use crate::cli_runtime::CliExecutable;

#[cfg(windows)]
pub(crate) const ANTIGRAVITY_PROGRAM: &str = "agy_acp_server.exe";
#[cfg(not(windows))]
pub(crate) const ANTIGRAVITY_PROGRAM: &str = "agy_acp_server.par";

/// `_configure_localharness_path` in agy_acp_server 1.3.0.
pub(crate) const HARNESS_PATH_ENV: &str = "ANTIGRAVITY_HARNESS_PATH";
#[cfg(windows)]
const HARNESS_FILE: &str = "localharness_external.exe";
#[cfg(not(windows))]
const HARNESS_FILE: &str = "localharness_external";

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum AntigravityExecutableError {
    #[error(
        "antigravity: {ANTIGRAVITY_PROGRAM} not found on PATH; install the Antigravity ACP server (see docs/subagents/antigravity.md)"
    )]
    BinaryMissing,
}

pub(crate) fn resolve() -> Result<CliExecutable, AntigravityExecutableError> {
    CliExecutable::resolve(ANTIGRAVITY_PROGRAM).ok_or(AntigravityExecutableError::BinaryMissing)
}

/// Server argv. On Linux the registry launches it with an empty `--uid=`;
/// without it 1.3.0 aborts at startup trying to switch to group `nobody`.
pub(crate) fn server_args() -> Vec<OsString> {
    if cfg!(target_os = "linux") {
        vec![OsString::from("--uid=")]
    } else {
        Vec::new()
    }
}

/// Where the server will find `localharness_external`. Both missing
/// variants fail runs; the server logs "Localharness not found." to the run
/// log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HarnessLocation {
    /// The user's `ANTIGRAVITY_HARNESS_PATH` names a file; the server uses it
    /// as-is.
    Override(PathBuf),
    /// The user's `ANTIGRAVITY_HARNESS_PATH` names no file. The server does
    /// not fall back to searching beside itself.
    OverrideMissing(PathBuf),
    /// Next to the canonical server; runs pin it.
    Beside(PathBuf),
    /// No override and nothing next to the server.
    Missing { expected: PathBuf },
}

/// Locate the harness: the user's `existing` override when set, else next to
/// `server` (canonicalized, so a PATH symlink still finds the real install
/// directory).
pub(crate) fn harness_location(server: &Path, existing: Option<OsString>) -> HarnessLocation {
    if let Some(path) = existing
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        return if path.is_file() {
            HarnessLocation::Override(path)
        } else {
            HarnessLocation::OverrideMissing(path)
        };
    }
    let server = std::fs::canonicalize(server).unwrap_or_else(|_| server.to_path_buf());
    let expected = server
        .parent()
        .map_or_else(|| PathBuf::from(HARNESS_FILE), |dir| dir.join(HARNESS_FILE));
    if expected.is_file() {
        HarnessLocation::Beside(expected)
    } else {
        HarnessLocation::Missing { expected }
    }
}

/// Spawn env pinning the harness next to the canonical server, or nothing
/// when the user already set the override or the harness is absent.
pub(crate) fn harness_env(
    server: Option<&Path>,
    existing: Option<OsString>,
) -> Vec<(OsString, OsString)> {
    match server.map(|server| harness_location(server, existing)) {
        Some(HarnessLocation::Beside(harness)) => {
            vec![(HARNESS_PATH_ENV.into(), harness.into_os_string())]
        }
        // Never replace the user's override, even a broken one.
        Some(
            HarnessLocation::Override(_)
            | HarnessLocation::OverrideMissing(_)
            | HarnessLocation::Missing { .. },
        )
        | None => Vec::new(),
    }
}

/// The live `ANTIGRAVITY_HARNESS_PATH` override, for [`harness_location`].
pub(crate) fn harness_override_from_env() -> Option<OsString> {
    std::env::var_os(HARNESS_PATH_ENV)
}

/// [`harness_env`] for the server this launch spawns: the frozen workflow
/// executable when given (a `/proc/self/fd/N` handle canonicalizes to the
/// verified file), else the PATH-resolved one. Reads the live
/// `ANTIGRAVITY_HARNESS_PATH` through [`harness_override_from_env`].
pub(crate) fn harness_env_for_launch(frozen: Option<&CliExecutable>) -> Vec<(OsString, OsString)> {
    let server: Option<PathBuf> = match frozen {
        Some(frozen) => Some(frozen.path().to_path_buf()),
        None => resolve().ok().map(|found| found.path().to_path_buf()),
    };
    harness_env(server.as_deref(), harness_override_from_env())
}

#[cfg(test)]
#[path = "executable_tests.rs"]
mod tests;
