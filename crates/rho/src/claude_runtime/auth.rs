//! Read Claude Code authentication and binary state.
//!
//! Source of truth is `claude auth status` JSON. Rho never stores these
//! credentials; it only reports what the `claude` binary reports.
//!
//! All probes are bounded: short timeout, capped stdout/stderr, and the child
//! is killed and awaited on timeout when the host allows it.

use std::{io, process::Stdio, time::Duration};

use serde::Deserialize;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use super::executable::{self, ClaudeExecutable};

/// Program name resolved on `PATH` for Claude Code.
pub(crate) const CLAUDE_PROGRAM: &str = "claude";

/// Default wall-clock budget for status/version/logout probes.
///
/// Cold Claude starts and keychain access can exceed a few seconds. Ten seconds
/// stays bounded for UI probes while avoiding false timeouts on first launch.
/// Tests inject a shorter timeout via [`run_bounded_command_with_timeout`].
pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Hard cap for each of stdout and stderr on a probe.
pub(crate) const PROBE_OUTPUT_CAP_BYTES: usize = 64 * 1024;

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
    #[error("claude code: failed to run `{program}`: {source}")]
    Spawn {
        program: String,
        #[source]
        source: io::Error,
    },
    #[error("claude code: `{program}` exited with {status}")]
    ExitStatus {
        program: String,
        status: std::process::ExitStatus,
        stderr: String,
    },
    #[error("claude code: `{program}` timed out after {timeout:?}")]
    Timeout { program: String, timeout: Duration },
    #[error("claude code: `{program}` produced more than {cap} bytes of {stream}")]
    OutputTooLarge {
        program: String,
        stream: &'static str,
        cap: usize,
    },
    #[error("claude code: auth status output was not valid UTF-8")]
    InvalidUtf8,
    #[error("claude code: could not parse auth status JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
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
    /// One-line summary for `/info`, `/doctor`, and login/logout notices.
    pub(crate) fn describe(&self) -> String {
        if !self.logged_in {
            return "claude code: not signed in - run /login claude-code".into();
        }
        let mut summary = String::from("claude code: signed in");
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
        summary.push_str(" - managed by the claude binary");
        summary
    }

    /// Post-login success copy that keeps ownership with Claude Code.
    pub(crate) fn describe_login_success(&self) -> String {
        let mut lines = Vec::with_capacity(2);
        let mut summary = String::from("claude code: signed in");
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
        lines.push(summary);
        lines.push(
            "Managed by the claude binary. Rho reads this state with `claude auth status`.".into(),
        );
        lines.join("\n")
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

    pub(crate) fn auth_description(&self) -> String {
        match &self.auth {
            Ok(status) => status.describe(),
            Err(error) => error.clone(),
        }
    }

    pub(crate) fn version_description(&self) -> String {
        match &self.version {
            Ok(version) => version.clone(),
            Err(error) => error.clone(),
        }
    }

    pub(crate) fn auth_healthy(&self) -> bool {
        matches!(&self.auth, Ok(status) if status.logged_in)
    }

    pub(crate) fn binary_healthy(&self) -> bool {
        self.version.is_ok()
    }
}

/// Typed doctor inputs so health is never inferred from display strings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClaudeDoctorHealth {
    pub(crate) auth_detail: String,
    pub(crate) auth_healthy: bool,
    pub(crate) version_detail: String,
    pub(crate) binary_healthy: bool,
}

impl ClaudeDoctorHealth {
    pub(crate) fn from_snapshot(snapshot: &ClaudeProbeSnapshot) -> Self {
        Self {
            auth_detail: snapshot.auth_description(),
            auth_healthy: snapshot.auth_healthy(),
            version_detail: snapshot.version_description(),
            binary_healthy: snapshot.binary_healthy(),
        }
    }

    pub(crate) fn unavailable_during_turn() -> Self {
        Self {
            auth_detail: "claude code: status not refreshed during a model turn".into(),
            auth_healthy: false,
            version_detail: "claude code: version not refreshed during a model turn".into(),
            binary_healthy: false,
        }
    }
}

/// Run `claude auth status` and parse its JSON.
pub(crate) async fn query() -> Result<ClaudeAuthStatus, ClaudeAuthError> {
    let executable = executable::resolve()?;
    query_executable(&executable).await
}

pub(crate) async fn query_executable(
    executable: &ClaudeExecutable,
) -> Result<ClaudeAuthStatus, ClaudeAuthError> {
    let output = run_bounded_probe(executable, &["auth", "status"]).await?;
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

pub(crate) async fn logout_executable(
    executable: &ClaudeExecutable,
) -> Result<(), ClaudeAuthError> {
    let output = run_bounded_probe(executable, &["auth", "logout"]).await?;
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
    executable: &ClaudeExecutable,
) -> Result<String, ClaudeAuthError> {
    let output = run_bounded_probe(executable, &["--version"]).await?;
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
pub(crate) fn describe_probe_result(result: &Result<ClaudeAuthStatus, ClaudeAuthError>) -> String {
    match result {
        Ok(status) => status.describe(),
        Err(error) => error.to_string(),
    }
}

#[cfg(test)]
impl ClaudeAuthError {
    pub(crate) fn is_binary_missing(&self) -> bool {
        matches!(self, Self::BinaryMissing)
    }
}

/// Run status and version probes concurrently for idle `/info` and `/doctor`.
pub(crate) async fn probe_snapshot() -> ClaudeProbeSnapshot {
    let auth = query();
    let version = version();
    let (auth, version) = tokio::join!(auth, version);
    ClaudeProbeSnapshot::from_results(auth, version)
}

/// Bounded child run used by status/version/logout helpers.
#[derive(Debug)]
struct BoundedOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl BoundedOutput {
    fn stderr_lossy_trimmed(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim().to_string()
    }
}

async fn run_bounded_probe(
    executable: &ClaudeExecutable,
    args: &[&str],
) -> Result<BoundedOutput, ClaudeAuthError> {
    run_bounded_probe_with_timeout(executable, args, PROBE_TIMEOUT).await
}

async fn run_bounded_probe_with_timeout(
    executable: &ClaudeExecutable,
    args: &[&str],
    timeout: Duration,
) -> Result<BoundedOutput, ClaudeAuthError> {
    let command = executable.command(args.iter().copied());
    run_bounded_command_with_timeout(command, executable.display(), timeout).await
}

/// Run an already-built probe command with the standard bounds.
///
/// Production helpers build the command from a resolved [`ClaudeExecutable`].
/// Tests inject a stable system shell (`/bin/sh -c …`) so they never exec a
/// freshly written file (which can race with `ETXTBSY` under parallel load).
async fn run_bounded_command_with_timeout(
    mut command: Command,
    program: String,
    timeout: Duration,
) -> Result<BoundedOutput, ClaudeAuthError> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = command
        .spawn()
        .map_err(|source| map_spawn_error(&program, source))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let collect = async {
        let stdout_task = async {
            match stdout {
                Some(pipe) => read_capped(pipe, PROBE_OUTPUT_CAP_BYTES).await,
                None => Ok(Vec::new()),
            }
        };
        let stderr_task = async {
            match stderr {
                Some(pipe) => read_capped(pipe, PROBE_OUTPUT_CAP_BYTES).await,
                None => Ok(Vec::new()),
            }
        };
        let (stdout, stderr) = tokio::join!(stdout_task, stderr_task);
        let stdout = stdout.map_err(|_| ClaudeAuthError::OutputTooLarge {
            program: program.clone(),
            stream: "stdout",
            cap: PROBE_OUTPUT_CAP_BYTES,
        })?;
        let stderr = stderr.map_err(|_| ClaudeAuthError::OutputTooLarge {
            program: program.clone(),
            stream: "stderr",
            cap: PROBE_OUTPUT_CAP_BYTES,
        })?;
        let status = child
            .wait()
            .await
            .map_err(|source| ClaudeAuthError::Spawn {
                program: program.clone(),
                source,
            })?;
        Ok::<BoundedOutput, ClaudeAuthError>(BoundedOutput {
            status,
            stdout,
            stderr,
        })
    };

    match tokio::time::timeout(timeout, collect).await {
        Ok(result) => result,
        Err(_) => {
            // Kill and reap the direct child. Probes use short budgets and
            // `kill_on_drop`; this path covers the timeout case explicitly so
            // the direct child is not left running. Full process-tree ownership
            // (descendants of a misbehaving binary) belongs to the Claude
            // execution lifecycle worker, not these short status/version probes.
            let _ = child.start_kill();
            let _ = child.wait().await;
            Err(ClaudeAuthError::Timeout { program, timeout })
        }
    }
}

async fn read_capped<R>(mut reader: R, cap: usize) -> Result<Vec<u8>, ()>
where
    R: AsyncRead + Unpin,
{
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 2048];
    loop {
        let read = reader.read(&mut chunk).await.map_err(|_| ())?;
        if read == 0 {
            return Ok(buffer);
        }
        if buffer.len().saturating_add(read) > cap {
            return Err(());
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
}

fn map_spawn_error(program: &str, source: io::Error) -> ClaudeAuthError {
    if source.kind() == io::ErrorKind::NotFound {
        ClaudeAuthError::BinaryMissing
    } else {
        ClaudeAuthError::Spawn {
            program: program.into(),
            source,
        }
    }
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
    Err(ClaudeAuthError::InvalidJson(
        serde_json::from_str::<ClaudeAuthStatus>("").expect_err("empty json"),
    ))
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
The terminal comes back to Rho when it finishes."
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
