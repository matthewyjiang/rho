//! agy install and sign-in state, as `/login antigravity`, `/doctor`, and
//! `/info` report it.
//!
//! agy owns its Google sign-in, shared by the agy CLI and `agy acp`; Rho
//! never stores or reads the token. Two bounded probes read the state:
//! `agy --version`, and `agy models`, which lists models only when signed in
//! and otherwise exits 1 asking to sign in (1.5 s measured with agy 1.5.0).
//! `session/new` would also tell, but it records an empty conversation.

use std::time::Duration;

use thiserror::Error;

use crate::cli_runtime::{run_bounded_probe, CliExecutable, ProbeError};

use super::executable::{self, AntigravityExecutableError, AGY_PROGRAM};

/// Same budget as the Claude and Cursor probes: cold starts and keychain
/// access can take seconds.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// The first agy release with `agy acp`.
const MIN_VERSION: (u64, u64, u64) = (1, 5, 0);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AntigravityStatus {
    pub(crate) version: String,
    pub(crate) signed_in: bool,
}

#[derive(Debug, Error)]
pub(crate) enum AntigravityProbeError {
    #[error(transparent)]
    Missing(#[from] AntigravityExecutableError),
    #[error("antigravity: {0}")]
    Probe(#[from] ProbeError),
    #[error(
        "antigravity: agy {version} has no ACP server; run `agy update` to get 1.5.0 or later"
    )]
    TooOld { version: String },
    #[error("antigravity: `{AGY_PROGRAM} {command}` exited with {status}: {output}")]
    ExitStatus {
        command: &'static str,
        status: std::process::ExitStatus,
        output: String,
    },
}

impl AntigravityStatus {
    /// One-line summary for `/info`, `/doctor`, and login notices.
    pub(crate) fn description(&self) -> String {
        if self.signed_in {
            format!("agy {} signed in", self.version)
        } else {
            "not signed in - run /login antigravity".into()
        }
    }
}

/// What `agy models` says about sign-in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ModelsProbe {
    SignedIn,
    SignedOut,
    Failed,
}

/// agy 1.5.0 lists models and exits 0 when signed in; signed out it exits 1
/// with "Please sign in to view available models." Any other failure is not
/// evidence of being signed out.
fn classify_models(success: bool, output: &str) -> ModelsProbe {
    if success {
        ModelsProbe::SignedIn
    } else if output.contains("Please sign in") {
        ModelsProbe::SignedOut
    } else {
        ModelsProbe::Failed
    }
}

/// Probe the `agy` on `PATH`.
pub(crate) async fn query() -> Result<AntigravityStatus, AntigravityProbeError> {
    let agy = executable::resolve()?;
    let version = supported_version(&agy).await?;
    let models = run_bounded_probe(&agy, &["models"], PROBE_TIMEOUT).await?;
    let output = format!(
        "{}\n{}",
        String::from_utf8_lossy(&models.stdout).trim(),
        String::from_utf8_lossy(&models.stderr).trim()
    );
    let signed_in = match classify_models(models.status.success(), &output) {
        ModelsProbe::SignedIn => true,
        ModelsProbe::SignedOut => false,
        ModelsProbe::Failed => {
            return Err(AntigravityProbeError::ExitStatus {
                command: "models",
                status: models.status,
                output: output.trim().to_owned(),
            })
        }
    };
    Ok(AntigravityStatus { version, signed_in })
}

/// `agy --version`, refusing releases without `agy acp`.
async fn supported_version(agy: &CliExecutable) -> Result<String, AntigravityProbeError> {
    let output = run_bounded_probe(agy, &["--version"], PROBE_TIMEOUT).await?;
    if !output.status.success() {
        return Err(AntigravityProbeError::ExitStatus {
            command: "--version",
            status: output.status,
            output: output.stderr_lossy_trimmed(),
        });
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if parse_version(&version).is_some_and(|parsed| parsed < MIN_VERSION) {
        return Err(AntigravityProbeError::TooOld { version });
    }
    Ok(version)
}

/// Resolve `agy` and refuse releases without `agy acp`, before any login
/// hands it the terminal.
pub(crate) async fn login_preflight() -> Result<CliExecutable, AntigravityProbeError> {
    let agy = executable::resolve()?;
    supported_version(&agy).await?;
    Ok(agy)
}

/// `rho login antigravity`: hand this terminal to `agy acp --login`.
pub(crate) async fn login() -> anyhow::Result<()> {
    let agy = login_preflight().await?;
    let status = agy
        .try_command(executable::LOGIN_ARGS.iter().copied())?
        .status()
        .await?;
    anyhow::ensure!(
        status.success(),
        "could not sign in to antigravity: `{AGY_PROGRAM} acp --login` exited with {status}"
    );
    Ok(())
}

/// `major.minor.patch` from `agy --version`; `None` for anything else, which
/// is not treated as too old.
fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.trim().split('.').map(str::parse::<u64>);
    let parsed = (
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
    );
    parts.next().is_none().then_some(parsed)
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
