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
const HARNESS_PATH_ENV: &str = "ANTIGRAVITY_HARNESS_PATH";
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

/// Spawn env pinning the harness next to the canonical server, or nothing
/// when the user already set the override or the harness is absent (the
/// server then logs "Localharness not found." to the run log).
pub(crate) fn harness_env(
    server: Option<&Path>,
    existing: Option<OsString>,
) -> Vec<(OsString, OsString)> {
    if existing.is_some_and(|value| !value.is_empty()) {
        return Vec::new();
    }
    let harness = server
        .and_then(|server| std::fs::canonicalize(server).ok())
        .and_then(|server| server.parent().map(|dir| dir.join(HARNESS_FILE)))
        .filter(|harness| harness.is_file());
    match harness {
        Some(harness) => vec![(HARNESS_PATH_ENV.into(), harness.into_os_string())],
        None => Vec::new(),
    }
}

/// [`harness_env`] for the server this launch spawns: the frozen workflow
/// executable when given (a `/proc/self/fd/N` handle canonicalizes to the
/// verified file), else the PATH-resolved one. Reads the live
/// `ANTIGRAVITY_HARNESS_PATH`; the only environment-reading seam here.
pub(crate) fn harness_env_for_launch(frozen: Option<&CliExecutable>) -> Vec<(OsString, OsString)> {
    let server: Option<PathBuf> = match frozen {
        Some(frozen) => Some(frozen.path().to_path_buf()),
        None => resolve().ok().map(|found| found.path().to_path_buf()),
    };
    harness_env(server.as_deref(), std::env::var_os(HARNESS_PATH_ENV))
}

#[cfg(test)]
#[path = "executable_tests.rs"]
mod tests;
