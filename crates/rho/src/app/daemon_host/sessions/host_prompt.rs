//! The system prompt an attach brings to a session the daemon has loaded.
//!
//! A request that opens a session gives it its client's system prompt as it
//! opens (see `setup.rs`). A loaded session takes another attach's from its
//! next model request: the change joins the runtime's queue under the turn
//! lock, as a settings change does (see `configure.rs`), as soon as the
//! attach has the session, so a turn under way finishes with the prompt it started with
//! and the client's next prompt's turn runs with the new one. An attach
//! that opened the session, or brings the prompt the session has, changes
//! nothing.

use std::sync::Arc;

use rho_sdk::CancellationToken;

use super::{Held, LoadedSession, Queued};
use crate::prompt::HostPrompt;

/// Gives `session` `prompt` once it holds the runtime, behind the turn under
/// way and the changes queued before, unless the daemon stops first, the
/// runtime is gone, or a delete holds the session. Counted as a queued
/// change meanwhile, as a settings change is, so a delete or an unload
/// waits for it.
pub(super) fn apply_when_free(
    session: &Arc<LoadedSession>,
    prompt: HostPrompt,
    stopping: CancellationToken,
) {
    let (held, queued) = {
        // `None` once the idle unload took the session after the attach.
        let Some(mut turn) = session.enter() else {
            return;
        };
        let Some(held) = Held::change(session, &mut turn) else {
            return;
        };
        (held, Queued::join(&session.runtime))
    };
    tokio::spawn(async move {
        let mut runtime = tokio::select! {
            biased;
            () = stopping.cancelled() => return,
            runtime = queued.acquire() => runtime,
        };
        if let Some(runtime) = runtime.as_mut() {
            if let Err(error) = runtime.set_host_prompt(prompt) {
                tracing::warn!(
                    session = %runtime.session_id(),
                    error = format!("{error:#}"),
                    "could not give the session the system prompt an attach brought"
                );
            }
        }
        // After the runtime, which a `Held`'s drop does not need.
        drop(runtime);
        drop(held);
    });
}
