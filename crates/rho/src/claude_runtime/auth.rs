//! Read Claude Code authentication and binary state.
//!
//! Source of truth is `claude auth status` JSON. Rho never stores these
//! credentials; it only reports what the `claude` binary reports.
//!
//! All probes are bounded: short timeout, capped stdout/stderr, and the child
//! is killed and awaited on timeout when the host allows it.

use std::time::Duration;

use serde::Deserialize;
use thiserror::Error;

use crate::cli_runtime::{run_bounded_probe, BoundedOutput, CliExecutable, ProbeError};

#[cfg(test)]
pub(crate) use crate::cli_runtime::{run_bounded_command_with_timeout, PROBE_OUTPUT_CAP_BYTES};

use super::executable;

/// Program name resolved on `PATH` for Claude Code.
pub(crate) const CLAUDE_PROGRAM: &str = "claude";

/// Default wall-clock budget for status/version/logout probes.
///
/// Cold Claude starts and keychain access can exceed a few seconds. Ten seconds
/// stays bounded for UI probes while avoiding false timeouts on first launch.
/// Tests inject a shorter timeout via
/// [`crate::cli_runtime::run_bounded_command_with_timeout`].
pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Parsed `claude auth status` payload.
///
/// Every field except `logged_in` is optional. Claude Code's schema is not a
/// stable contract; only the signed-in bit is load-bearing for Rho.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClaudeAuthStatus {
    pub(crate) logged_in: bool,
    #[serde(default)]
    pub(crate) auth_method: Option<String>,
    #[serde(default)]
    pub(crate) api_provider: Option<String>,
    #[serde(default)]
    pub(crate) email: Option<String>,
    #[serde(default)]
    pub(crate) org_id: Option<String>,
    #[serde(default)]
    pub(crate) org_name: Option<String>,
    #[serde(default)]
    pub(crate) subscription_type: Option<String>,
}

/// Failures when probing the Claude Code binary or its auth state.
#[derive(Debug, Error)]
pub(crate) enum ClaudeAuthError {
    #[error("claude code: binary not found on PATH")]
    BinaryMissing,
    #[error("claude code: {0}")]
    Probe(ProbeError),
    #[error("claude code: `{program}` exited with {status}")]
    ExitStatus {
        program: String,
        status: std::process::ExitStatus,
        stderr: String,
    },
    #[error("claude code: auth status output was not valid UTF-8")]
    InvalidUtf8,
    #[error("claude code: `{program}` produced no auth status output")]
    EmptyOutput { program: String },
    #[error("claude code: could not parse auth status JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
}

impl From<ProbeError> for ClaudeAuthError {
    fn from(error: ProbeError) -> Self {
        match error {
            ProbeError::BinaryMissing => Self::BinaryMissing,
            other => Self::Probe(other),
        }
    }
}

impl ClaudeAuthError {
    /// Bounded stderr excerpt for UI notices. Empty when none is available.
    pub(crate) fn stderr_excerpt(&self) -> Option<&str> {
        match self {
            Self::ExitStatus { stderr, .. } if !stderr.is_empty() => Some(stderr.as_str()),
            _ => None,
        }
    }

    /// Short sanitized detail suitable for notices (already capped at read time).
    pub(crate) fn sanitized_detail(&self) -> String {
        match self {
            Self::ExitStatus { stderr, status, .. } => {
                if stderr.is_empty() {
                    format!("exit status {status}")
                } else {
                    stderr.chars().take(240).collect()
                }
            }
            other => other.to_string(),
        }
    }
}

impl ClaudeAuthStatus {
    /// `claude code: signed in[ as EMAIL][ (PLAN)]`, the shared prefix of every
    /// signed-in summary.
    fn signed_in_summary(&self) -> String {
        format!("claude code: {}", self.account_summary())
    }

    /// `signed in[ as EMAIL][ (PLAN)]` for surfaces that already name the
    /// runtime, such as a doctor row.
    pub(crate) fn account_summary(&self) -> String {
        let mut summary = String::from("signed in");
        if let Some(email) = self.email.as_deref().filter(|value| !value.is_empty()) {
            summary.push_str(" as ");
            summary.push_str(email);
        }
        if let Some(subscription) = self
            .subscription_type
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            summary.push_str(" (");
            summary.push_str(subscription);
            summary.push(')');
        }
        summary
    }

    /// One-line summary for `/info`, `/doctor`, and login/logout notices.
    pub(crate) fn describe(&self) -> String {
        if !self.logged_in {
            return "claude code: not signed in - run /login claude-code".into();
        }
        let mut summary = self.signed_in_summary();
        summary.push_str(" - managed by the claude binary");
        summary
    }

    /// Post-login success copy that keeps ownership with Claude Code.
    pub(crate) fn describe_login_success(&self) -> String {
        format!(
            "{}\nManaged by the claude binary. Rho reads this state with `claude auth status`.",
            self.signed_in_summary()
        )
    }
}

/// Snapshot used by `/info` and `/doctor` so turns never block on a probe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClaudeProbeSnapshot {
    pub(crate) auth: Result<ClaudeAuthStatus, String>,
    pub(crate) version: Result<String, String>,
}

impl ClaudeProbeSnapshot {
    pub(crate) fn from_results(
        auth: Result<ClaudeAuthStatus, ClaudeAuthError>,
        version: Result<String, ClaudeAuthError>,
    ) -> Self {
        Self {
            auth: auth.map_err(|error| error.to_string()),
            version: version.map_err(|error| error.to_string()),
        }
    }

    /// Snapshot for surfaces that skip live probes during a model turn, so a
    /// child process never blocks stream draining.
    pub(crate) fn not_refreshed_during_turn() -> Self {
        Self {
            auth: Err("claude code: status not refreshed during a model turn".into()),
            version: Err("claude code: version not refreshed during a model turn".into()),
        }
    }

    pub(crate) fn auth_description(&self) -> String {
        match &self.auth {
            Ok(status) => status.describe(),
            Err(error) => error.clone(),
        }
    }
}

/// Run `claude auth status` and parse its JSON.
pub(crate) async fn query() -> Result<ClaudeAuthStatus, ClaudeAuthError> {
    let executable = executable::resolve()?;
    query_executable(&executable).await
}

pub(crate) async fn query_executable(
    executable: &CliExecutable,
) -> Result<ClaudeAuthStatus, ClaudeAuthError> {
    let output = run_bounded_probe(executable, &["auth", "status"], PROBE_TIMEOUT).await?;
    parse_auth_status_output(&executable.display(), &output)
}

/// Run `claude auth logout`. Signs the user out of Claude Code globally.
///
/// Callers must treat a follow-up [`query`] as the source of truth for
/// signed-out state. A non-zero exit here is extra detail only.
pub(crate) async fn logout() -> Result<(), ClaudeAuthError> {
    let executable = executable::resolve()?;
    logout_executable(&executable).await
}

pub(crate) async fn logout_executable(executable: &CliExecutable) -> Result<(), ClaudeAuthError> {
    let output = run_bounded_probe(executable, &["auth", "logout"], PROBE_TIMEOUT).await?;
    if output.status.success() {
        Ok(())
    } else {
        Err(ClaudeAuthError::ExitStatus {
            program: executable.display(),
            status: output.status,
            stderr: output.stderr_lossy_trimmed(),
        })
    }
}

/// Probe `claude --version` for doctor diagnostics.
pub(crate) async fn version() -> Result<String, ClaudeAuthError> {
    let executable = executable::resolve()?;
    version_executable(&executable).await
}

pub(crate) async fn version_executable(
    executable: &CliExecutable,
) -> Result<String, ClaudeAuthError> {
    let output = run_bounded_probe(executable, &["--version"], PROBE_TIMEOUT).await?;
    if !output.status.success() {
        return Err(ClaudeAuthError::ExitStatus {
            program: executable.display(),
            status: output.status,
            stderr: output.stderr_lossy_trimmed(),
        });
    }
    let stdout = String::from_utf8(output.stdout).map_err(|_| ClaudeAuthError::InvalidUtf8)?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let version = first_nonempty_line(&stdout)
        .or_else(|| first_nonempty_line(&stderr))
        .unwrap_or("unknown version")
        .to_string();
    Ok(version)
}

#[cfg(test)]
impl ClaudeAuthError {
    pub(crate) fn is_binary_missing(&self) -> bool {
        matches!(
            self,
            Self::BinaryMissing | Self::Probe(ProbeError::BinaryMissing)
        )
    }
}

/// Run status and version probes concurrently for idle `/info` and `/doctor`.
pub(crate) async fn probe_snapshot() -> ClaudeProbeSnapshot {
    let auth = query();
    let version = version();
    let (auth, version) = tokio::join!(auth, version);
    ClaudeProbeSnapshot::from_results(auth, version)
}

fn parse_auth_status_output(
    program: &str,
    output: &BoundedOutput,
) -> Result<ClaudeAuthStatus, ClaudeAuthError> {
    // Claude Code returns exit 1 with valid JSON when signed out:
    // `{"loggedIn":false,"authMethod":"none","apiProvider":"firstParty"}`.
    // Prefer structurally valid status JSON over exit status so signed-out is
    // a normal `ClaudeAuthStatus`, not a probe failure.
    let stdout =
        String::from_utf8(output.stdout.clone()).map_err(|_| ClaudeAuthError::InvalidUtf8)?;
    let trimmed = stdout.trim();
    if !trimmed.is_empty() {
        return match serde_json::from_str::<ClaudeAuthStatus>(trimmed) {
            Ok(status) => Ok(status),
            // Non-empty stdout that is not auth status JSON is always a parse
            // error, even when the exit status is non-zero.
            Err(error) => Err(ClaudeAuthError::InvalidJson(error)),
        };
    }
    if !output.status.success() {
        return Err(ClaudeAuthError::ExitStatus {
            program: program.into(),
            status: output.status,
            stderr: output.stderr_lossy_trimmed(),
        });
    }
    Err(ClaudeAuthError::EmptyOutput {
        program: program.into(),
    })
}

fn first_nonempty_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
}

/// Notice shown before Rho hands the terminal to `claude auth login`.
pub(crate) fn login_handoff_notice() -> &'static str {
    "Rho is handing the terminal to the claude binary to sign in.\n\n\
Claude Code runs the sign-in and stores the credential. Rho never sees or \
stores your token. To sign out later, run `/logout claude-code` or \
`claude auth logout` yourself.\n\n\
If you did not mean to sign in, stop the claude process from another \
terminal or close this prompt. Rho resumes when that process exits. \
There is no cancel key inside the Claude sign-in prompt."
}

/// Status printed on the main screen after Rho leaves the alternate buffer.
pub(crate) fn login_handoff_status() -> String {
    format!("Signing in to Claude Code…\n\n{}", login_handoff_notice())
}

/// Notice shown in the logout confirmation choice.
pub(crate) fn logout_confirm_description() -> &'static str {
    "This signs you out of Claude Code everywhere the claude binary is used, \
not only inside Rho. Rho does not store this credential and cannot delete a \
Rho token for it."
}

/// Login argv for suspended interactive handoff (fixed tokens only).
pub(crate) fn login_args() -> &'static [&'static str] {
    &["auth", "login", "--claudeai"]
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
