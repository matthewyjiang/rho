//! Native compact fixtures for PTY scenarios.
//!
//! Compact stays in flight until the scenario cancels it or writes a release
//! marker. Do not finish compact on a wall-clock timer; that races typing.

use std::{path::PathBuf, time::Duration};

use rho_sdk::{
    model::ModelRequest,
    provider::{NativeCompactionFuture, NativeCompactionResponse},
    CancellationToken, ProviderError, ProviderErrorKind, Retryability,
};

use super::last_user_text;

/// Workspace-relative marker that releases a hanging compact fixture.
///
/// Mirrored as a literal in `crates/rho-tui-pty/src/scenarios.rs`
/// (`release_compact_fixture`). The crates cannot share a const; keep both
/// strings identical or `submit_during_compact` hangs until the STREAM timeout.
const RELEASE_MARKER: &str = ".rho-fixture-release-compact";

/// How often to look for [`RELEASE_MARKER`]. The signal is the file; this
/// interval only bounds how soon we notice it.
const RELEASE_POLL: Duration = Duration::from_millis(20);

pub(super) fn native_compact(request: ModelRequest<'_>) -> Option<NativeCompactionFuture<'_>> {
    let prompt = last_user_text(&request)?;
    if prompt.contains("fixture compact until cancel") {
        return Some(Box::pin(async move {
            request.cancellation.cancelled().await;
            NativeCompactionResponse::failure(ProviderError::interrupted(
                "fixture provider cancelled",
            ))
        }));
    }
    if prompt.contains("fixture compact until release") {
        return Some(Box::pin(async move {
            match wait_for_release_or_cancel(&request.cancellation).await {
                Ok(()) => NativeCompactionResponse::failure(ProviderError::new(
                    ProviderErrorKind::Unavailable,
                    "fixture compact released",
                    Retryability::Permanent,
                )),
                Err(error) => NativeCompactionResponse::failure(error),
            }
        }));
    }
    None
}

async fn wait_for_release_or_cancel(cancellation: &CancellationToken) -> Result<(), ProviderError> {
    let marker = release_marker_path();
    loop {
        if marker.exists() {
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

fn release_marker_path() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(RELEASE_MARKER)
}
