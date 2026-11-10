//! Workspace marker releases shared by fixtures, and where fixtures find the
//! workspace's files. A release is consumed once.

use std::{io::ErrorKind, path::PathBuf, time::Duration};

use rho_sdk::{CancellationToken, ProviderError};

// The file synchronizes the fixture; this existing compact observation interval
// only bounds how soon we notice a release.
const RELEASE_POLL: Duration = Duration::from_millis(20);

/// Names the workspace the PTY harness reads and writes fixture files in. A
/// daemon-hosted fixture runs in the daemon, whose working directory is `/`,
/// not the workspace, so it finds them through this.
const WORKSPACE_ENV: &str = "RHO_TUI_FIXTURE_WORKSPACE";

/// Where the workspace's `file` is: in the harness's workspace, or relative
/// to this process's working directory, which is the workspace in process.
pub(super) fn workspace_path(file: &str) -> PathBuf {
    match std::env::var_os(WORKSPACE_ENV) {
        Some(workspace) => PathBuf::from(workspace).join(file),
        None => PathBuf::from(file),
    }
}

/// Consume a release, or clear an owned stale marker before publishing output.
pub(super) fn consume_release(marker: &str) -> Result<bool, ProviderError> {
    match std::fs::remove_file(workspace_path(marker)) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(ProviderError::interrupted(format!(
            "consume fixture release marker {marker}: {error}"
        ))),
    }
}

pub(super) async fn wait_for_release_or_cancel(
    marker: &str,
    cancellation: &CancellationToken,
) -> Result<(), ProviderError> {
    loop {
        if consume_release(marker)? {
            return Ok(());
        }
        tokio::select! {
            () = cancellation.cancelled() => {
                return Err(ProviderError::interrupted("fixture provider cancelled"));
            }
            () = tokio::time::sleep(RELEASE_POLL) => {}
        }
    }
}
