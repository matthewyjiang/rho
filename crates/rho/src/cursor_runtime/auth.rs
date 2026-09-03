//! Read Cursor Agent authentication and binary state.
//!
//! Source of truth is `cursor-agent status --format json`. Rho never stores
//! these credentials; it only reports what the binary reports.
//!
//! All probes are bounded: short timeout, capped stdout/stderr, and the child
//! is killed and awaited on timeout when the host allows it.

use std::time::Duration;

use serde::Deserialize;
use thiserror::Error;

use crate::cli_runtime::{run_bounded_probe, BoundedOutput, CliExecutable, ProbeError};

use super::{executable, models::CURSOR_PROGRAM_LABEL};

/// Default wall-clock budget for status/version probes.
///
/// Cold starts and keychain access can exceed a few seconds. Ten seconds
/// stays bounded for UI probes while avoiding false timeouts on first launch
/// (same budget as the Claude probe).
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Parsed `cursor-agent status --format json` payload.
///
/// Cursor's schema is not a stable contract. Every field defaults so extra
/// keys (`hasAccessToken`, …) are ignored and a missing signed-in bit is
/// treated as signed out.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CursorAuthStatus {
    #[serde(default)]
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) is_authenticated: bool,
    #[serde(default)]
    pub(crate) message: Option<String>,
    #[serde(default)]
    pub(crate) user_info: Option<CursorUserInfo>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CursorUserInfo {
    #[serde(default)]
    pub(crate) email: Option<String>,
}

/// Failures when probing the Cursor Agent binary or its auth state.
#[derive(Debug, Error)]
pub(crate) enum CursorAuthError {
    #[error("cursor: binary not found on PATH")]
    BinaryMissing,
    #[error("cursor: {0}")]
    Probe(ProbeError),
    #[error("cursor: `{program}` exited with {status}")]
    ExitStatus {
        program: String,
        status: std::process::ExitStatus,
        stderr: String,
    },
    #[error("cursor: auth status output was not valid UTF-8")]
    InvalidUtf8,
    #[error("cursor: `{program}` produced no auth status output")]
    EmptyOutput { program: String },
    #[error("cursor: could not parse auth status JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
}

impl From<ProbeError> for CursorAuthError {
    fn from(error: ProbeError) -> Self {
        match error {
            ProbeError::BinaryMissing => Self::BinaryMissing,
            other => Self::Probe(other),
        }
    }
}

impl CursorAuthStatus {
    /// One-line summary for `/info`, `/doctor`, and login notices.
    pub(crate) fn auth_description(&self) -> String {
        if !self.is_authenticated {
            return format!("{CURSOR_PROGRAM_LABEL}: not signed in - run /login cursor");
        }
        match self
            .user_info
            .as_ref()
            .and_then(|info| info.email.as_deref())
            .filter(|email| !email.is_empty())
        {
            Some(email) => format!("{CURSOR_PROGRAM_LABEL}: signed in as {email}"),
            None => format!("{CURSOR_PROGRAM_LABEL}: signed in"),
        }
    }
}

/// Run `cursor-agent status --format json` and parse its JSON.
pub(crate) async fn query() -> Result<CursorAuthStatus, CursorAuthError> {
    let executable = executable::resolve()?;
    query_executable(&executable).await
}

async fn query_executable(executable: &CliExecutable) -> Result<CursorAuthStatus, CursorAuthError> {
    let output =
        run_bounded_probe(executable, &["status", "--format", "json"], PROBE_TIMEOUT).await?;
    parse_auth_status_output(&executable.display(), &output)
}

/// Probe `cursor-agent --version` for doctor diagnostics.
pub(crate) async fn version() -> Result<String, CursorAuthError> {
    let executable = executable::resolve()?;
    let output = run_bounded_probe(&executable, &["--version"], PROBE_TIMEOUT).await?;
    if !output.status.success() {
        return Err(CursorAuthError::ExitStatus {
            program: executable.display(),
            status: output.status,
            stderr: output.stderr_lossy_trimmed(),
        });
    }
    let stdout = String::from_utf8(output.stdout).map_err(|_| CursorAuthError::InvalidUtf8)?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let version = first_nonempty_line(&stdout)
        .or_else(|| first_nonempty_line(&stderr))
        .unwrap_or("unknown version")
        .to_string();
    Ok(version)
}

/// Login argv for suspended interactive handoff (fixed tokens only).
pub(crate) fn login_args() -> &'static [&'static str] {
    &["login"]
}

fn parse_auth_status_output(
    program: &str,
    output: &BoundedOutput,
) -> Result<CursorAuthStatus, CursorAuthError> {
    // Prefer structurally valid status JSON over exit status so signed-out is
    // a normal `CursorAuthStatus`, not a probe failure.
    let stdout =
        String::from_utf8(output.stdout.clone()).map_err(|_| CursorAuthError::InvalidUtf8)?;
    let trimmed = stdout.trim();
    if !trimmed.is_empty() {
        return match serde_json::from_str::<CursorAuthStatus>(trimmed) {
            Ok(status) => Ok(status),
            Err(error) => Err(CursorAuthError::InvalidJson(error)),
        };
    }
    if !output.status.success() {
        return Err(CursorAuthError::ExitStatus {
            program: program.into(),
            status: output.status,
            stderr: output.stderr_lossy_trimmed(),
        });
    }
    Err(CursorAuthError::EmptyOutput {
        program: program.into(),
    })
}

fn first_nonempty_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
