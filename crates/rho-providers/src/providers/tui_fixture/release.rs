//! Workspace marker releases shared by fixtures. A release is consumed once.

use std::{io::ErrorKind, time::Duration};

use rho_sdk::{CancellationToken, ProviderError};

// The file synchronizes the fixture; this existing compact observation interval
// only bounds how soon we notice a release.
const RELEASE_POLL: Duration = Duration::from_millis(20);

/// Consume a release, or clear an owned stale marker before publishing output.
pub(super) fn consume_release(marker: &str) -> Result<bool, ProviderError> {
    match std::fs::remove_file(marker) {
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
