//! Driver update discovery delegates release and channel policy to the driver's
//! own `check-update --json`. Checks run the trusted executable but never
//! connect it, so they are separate from desktop authority. They are part of
//! the session lifecycle: refused while an installer replaces the executable,
//! and aborted when one starts.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::{anyhow, bail};
use futures_util::{
    future::{BoxFuture, Shared},
    FutureExt,
};
use serde::Deserialize;
use tokio::{process::Command, task::AbortHandle};

use super::{ComputerUseSession, State};

/// Upstream bounds its GitHub request at 4s and caches for 20h; this only
/// catches a hung executable, so it is generous.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
/// `--version` is local and prints immediately; this only catches a hang.
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
/// Enough stderr to explain a failure without flooding the dashboard.
const STDERR_EXCERPT_BYTES: usize = 400;

/// Subset of the driver's `check_for_update` payload Rho relies on.
#[derive(Deserialize)]
struct CheckPayload {
    current_version: String,
    latest_version: Option<String>,
    update_available: bool,
    /// The driver reports offline, rate-limited, and package-managed states here.
    error: Option<String>,
    release_notes_url: Option<String>,
}

/// What the driver's own check concluded, validated once at the boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum UpdateOutcome {
    Available {
        current: String,
        latest: String,
        notes: Option<String>,
    },
    UpToDate {
        current: String,
    },
    /// The driver answered but could not determine availability.
    Unavailable {
        current: String,
        reason: String,
    },
}

/// Latest update check for this session's driver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum UpdateCheckStatus {
    NotChecked,
    Checking,
    Checked(UpdateOutcome),
    Failed(String),
}

type CheckTask = Shared<BoxFuture<'static, Result<UpdateOutcome, String>>>;

/// Aborts the check (killing its driver process) when replaced or dropped.
pub(super) struct RunningCheck {
    task: CheckTask,
    abort: AbortHandle,
}

impl Drop for RunningCheck {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

#[derive(Default)]
pub(super) enum UpdateCheck {
    #[default]
    NotChecked,
    Checking(RunningCheck),
    Done(Result<UpdateOutcome, String>),
}

impl ComputerUseSession {
    /// Run the driver's read-only update check in the background. Callers must
    /// only do this with desktop consent or an explicit user request, because
    /// it executes the detected driver. Refused while an installer is running.
    pub(crate) fn start_update_check(&self) -> anyhow::Result<()> {
        // Lock order: state, then update_check (as in `start_installer`).
        let state = self.state();
        match &*state {
            State::Installing(_) => {
                bail!("Cua Driver installation is pending; check again after it finishes")
            }
            State::Off { .. }
            | State::Connecting { .. }
            | State::Connected { .. }
            | State::Closing { .. } => {}
        }
        let driver = self
            .driver_path()
            .ok_or_else(|| anyhow!("Cua Driver was not detected; /computer setup installs it"))?;
        let home = absolute_home().ok_or_else(|| {
            anyhow!("an absolute home directory is required to check for updates")
        })?;
        let mut check = self.update_check();
        if matches!(&*check, UpdateCheck::Checking(_)) {
            return Ok(());
        }
        let handle = tokio::spawn(async move {
            check_driver(&driver, &home)
                .await
                .map_err(|error| error.to_string())
        });
        let abort = handle.abort_handle();
        let task = async move { handle.await.map_err(|error| error.to_string())? }
            .boxed()
            .shared();
        *check = UpdateCheck::Checking(RunningCheck { task, abort });
        Ok(())
    }

    /// Settle a finished check. Returns whether the visible status changed.
    pub(crate) fn poll_update_check(&self) -> bool {
        let mut check = self.update_check();
        let UpdateCheck::Checking(running) = &*check else {
            return false;
        };
        let Some(result) = running.task.clone().now_or_never() else {
            return false;
        };
        *check = UpdateCheck::Done(result);
        true
    }

    pub(crate) fn update_check_status(&self) -> UpdateCheckStatus {
        match &*self.update_check() {
            UpdateCheck::NotChecked => UpdateCheckStatus::NotChecked,
            UpdateCheck::Checking(_) => UpdateCheckStatus::Checking,
            UpdateCheck::Done(Ok(outcome)) => UpdateCheckStatus::Checked(outcome.clone()),
            UpdateCheck::Done(Err(error)) => UpdateCheckStatus::Failed(error.clone()),
        }
    }

    /// Discard (and abort) any check of an executable about to be replaced.
    pub(super) fn reset_update_check(&self) {
        *self.update_check() = UpdateCheck::NotChecked;
    }

    fn update_check(&self) -> std::sync::MutexGuard<'_, UpdateCheck> {
        self.inner
            .update_check
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
}

/// Only release-shaped versions (`0.31.0`, `0.30.5-nightly.20260929.1`) reach
/// the installer environment or comparisons.
pub(super) fn validate_version(version: &str) -> anyhow::Result<()> {
    let shaped = version.starts_with(|c: char| c.is_ascii_digit())
        && !version.contains("..")
        && !version.ends_with(['.', '-'])
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    if !shaped {
        bail!("unexpected Cua Driver version {version:?}");
    }
    Ok(())
}

fn absolute_home() -> Option<PathBuf> {
    crate::paths::home_dir().filter(|home| home.is_absolute())
}

async fn check_driver(driver: &Path, home: &Path) -> anyhow::Result<UpdateOutcome> {
    let output = run_driver(driver, home, &["check-update", "--json"], CHECK_TIMEOUT)
        .await
        .map_err(|error| {
            anyhow!("{error}; older drivers do not support update checks, so update manually")
        })?;
    let payload: CheckPayload = serde_json::from_slice(&output)
        .map_err(|error| anyhow!("cua-driver check-update returned unexpected output: {error}"))?;
    outcome(payload)
}

fn outcome(payload: CheckPayload) -> anyhow::Result<UpdateOutcome> {
    let CheckPayload {
        current_version: current,
        latest_version,
        update_available,
        error,
        release_notes_url: notes,
    } = payload;
    validate_version(&current)?;
    Ok(match (error, update_available, latest_version) {
        (Some(reason), _, _) => UpdateOutcome::Unavailable { current, reason },
        (None, true, Some(latest)) => {
            validate_version(&latest)?;
            UpdateOutcome::Available {
                current,
                latest,
                notes,
            }
        }
        (None, true, None) => UpdateOutcome::Unavailable {
            current,
            reason: "the driver reported an update without a version".into(),
        },
        (None, false, _) => UpdateOutcome::UpToDate { current },
    })
}

/// The version reported by the executable Rho would launch.
pub(super) async fn driver_version(driver: &Path) -> anyhow::Result<String> {
    let home = absolute_home().ok_or_else(|| anyhow!("an absolute home directory is required"))?;
    let output = run_driver(driver, &home, &["--version"], VERSION_TIMEOUT).await?;
    let output = String::from_utf8_lossy(&output);
    let version = output
        .split_whitespace()
        .last()
        .map(|token| token.trim_start_matches('v'))
        .unwrap_or_default();
    validate_version(version)
        .map_err(|_| anyhow!("unexpected cua-driver --version output: {}", output.trim()))?;
    Ok(version.to_owned())
}

async fn run_driver(
    driver: &Path,
    home: &Path,
    args: &[&str],
    timeout: Duration,
) -> anyhow::Result<Vec<u8>> {
    let mut command = Command::new(driver);
    command.args(args);
    super::setup::restrict_environment(&mut command, home);
    command
        .envs(super::policy::telemetry_environment())
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let invocation = format!("cua-driver {}", args.join(" "));
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| anyhow!("{invocation} exceeded the {}s limit", timeout.as_secs()))??;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        let start = stderr.floor_char_boundary(stderr.len().saturating_sub(STDERR_EXCERPT_BYTES));
        bail!(
            "{invocation} exited with {}: {}",
            output.status,
            &stderr[start..]
        );
    }
    Ok(output.stdout)
}

#[cfg(test)]
#[path = "update_tests.rs"]
mod tests;
