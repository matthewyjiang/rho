//! Locate `agy` and build the `agy acp` command line.

use std::{ffi::OsString, path::Path};

use crate::cli_runtime::CliExecutable;

pub(crate) const AGY_PROGRAM: &str = "agy";

/// agy settings directory Rho runs use, under agy's app data directory
/// (`~/.gemini/antigravity-cli/rho`). Sign-in lives one level up, so runs
/// stay signed in, but the user's own `settings.json` (which may set
/// `toolPermission: always-proceed`) is never read, so agy keeps asking Rho.
const RHO_CLI_DIR: &str = "rho";

/// agy's own terminal Google sign-in. It prints a link and accepts the
/// pasted code, so it also works over SSH.
pub(crate) const LOGIN_ARGS: &[&str] = &["acp", "--login"];

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum AntigravityExecutableError {
    #[error("antigravity: {AGY_PROGRAM} is not installed; install agy 1.5.0 or later")]
    BinaryMissing,
}

pub(crate) fn resolve() -> Result<CliExecutable, AntigravityExecutableError> {
    CliExecutable::resolve(AGY_PROGRAM).ok_or(AntigravityExecutableError::BinaryMissing)
}

/// `agy acp` with Rho's settings directory; every run adds its agent.
pub(crate) const SERVER_ARGS: &[&str] = &["acp", "--cli_dir_name", RHO_CLI_DIR];

/// `agy acp` argv for one delegated run: [`SERVER_ARGS`], then the run's
/// agent workspace root and agent (see `agent_file`).
pub(crate) fn server_args(agent_root: &Path) -> Vec<OsString> {
    let mut argv: Vec<OsString> = SERVER_ARGS.iter().map(OsString::from).collect();
    argv.extend([
        "--add-dir".into(),
        agent_root.into(),
        "--agent".into(),
        super::agent_file::AGENT_NAME.into(),
    ]);
    argv
}
