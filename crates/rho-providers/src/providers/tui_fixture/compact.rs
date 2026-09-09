//! Native compact fixtures for PTY scenarios.
//!
//! Compact stays in flight until the scenario cancels it or writes a release
//! marker. Do not finish compact on a wall-clock timer; that races typing.

use rho_sdk::{
    model::{ContentBlock, ModelRequest, SemanticMessage},
    provider::{NativeCompactionFuture, NativeCompactionResponse},
    ProviderError, ProviderErrorKind, Retryability,
};

use super::release::wait_for_release_or_cancel;

/// Workspace-relative marker that releases a hanging compact fixture.
///
/// Mirrored as a literal in `crates/rho-tui-pty/src/scenarios.rs`
/// (`release_compact_fixture`). The crates cannot share a const; keep both
/// strings identical or `submit_during_compact` hangs until the STREAM timeout.
const RELEASE_MARKER: &str = ".rho-fixture-release-compact";

fn history_contains_user_text(request: &ModelRequest<'_>, needle: &str) -> bool {
    request.messages.iter().any(|message| {
        let SemanticMessage::User(content) = message.semantic() else {
            return false;
        };
        content.iter().any(|block| match block {
            ContentBlock::Text(text) => text.contains(needle),
            ContentBlock::Image(_) | ContentBlock::ToolCall(_) => false,
        })
    })
}

pub(super) fn native_compact(request: ModelRequest<'_>) -> Option<NativeCompactionFuture<'_>> {
    // Match the seed prompt anywhere in history. Compact requests can grow a
    // later empty or non-fixture user entry; last-only matching then falls
    // through to the fast summary path and races PTY waits.
    if history_contains_user_text(&request, "fixture compact until cancel") {
        return Some(Box::pin(async move {
            request.cancellation.cancelled().await;
            NativeCompactionResponse::failure(ProviderError::interrupted(
                "fixture provider cancelled",
            ))
        }));
    }
    if history_contains_user_text(&request, "fixture compact until release") {
        return Some(Box::pin(async move {
            match wait_for_release_or_cancel(RELEASE_MARKER, &request.cancellation).await {
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
