//! Unloading idle sessions (#1661). A loaded session holds its file's lease,
//! which keeps every other rho process from opening it, and its MCP
//! servers. Once it sits idle, with no turn under way and no client
//! attached, for the daemon's grace, the daemon tears it down; the next
//! prompt or attach reopens it from its file.
//!
//! A sweep every grace marks each idle session idle from then, and unloads
//! one a sweep already marked a grace ago, so a session unloads between one
//! and two graces after it went idle. A prompt, an attach, and a turn's end
//! mark it in use again. Each happens under the session's turn lock, which
//! the unload decides under, so a request either uses the session before
//! the unload sees it, keeping it, or finds it unloaded and reopens it.

use std::{sync::Arc, time::Duration};

use rho_sdk::SessionId;
use tokio::time::Instant;

use super::{LoadedSession, Sessions};
use crate::app::daemon_host::jobs::SessionJob;

/// How long a session sits idle before the daemon unloads it, unless
/// `rho daemon run --unload-idle-after` says otherwise.
//
// Reopening costs little, so this need only span a client's reconnect, such
// as a TUI restart or a frontend switching back to a session. Measured on a
// debug build, 15 sessions each, as cold minus warm attach: 7 ms median
// (26 ms max) with the fixture config, 10 ms (28 ms) with a real config,
// 21 ms (41 ms) with a stdio MCP server added. A tripwire past that, not a
// measured reconnect gap.
pub(in crate::app) const UNLOAD_IDLE_AFTER: Duration = Duration::from_secs(60);

/// Whether a session is in use, idle, or unloaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Residency {
    /// Used since the last sweep, or a turn runs or a client follows it.
    InUse,
    /// Idle at every sweep since this one.
    IdleSince(Instant),
    /// Unloaded: a request that found the session before then reopens it.
    Unloaded,
}

impl Residency {
    /// Marks the session in use unless it was unloaded; whether it is still
    /// loaded.
    pub(super) fn mark_in_use(&mut self) -> bool {
        match self {
            Self::Unloaded => false,
            Self::InUse | Self::IdleSince(_) => {
                *self = Self::InUse;
                true
            }
        }
    }
}

impl Sessions {
    /// Sweeps the loaded sessions every `grace`, unloading those idle for
    /// it, until the daemon starts stopping.
    pub(in crate::app::daemon_host) async fn unload_idle(&self, grace: Duration) {
        loop {
            tokio::select! {
                () = self.stopping.cancelled() => return,
                () = tokio::time::sleep(grace) => {}
            }
            self.sweep(grace).await;
        }
    }

    /// Marks each loaded session idle, or unloads it once it was idle for
    /// `grace`.
    async fn sweep(&self, grace: Duration) {
        let sessions: Vec<_> = self
            .loaded()
            .iter()
            .map(|(id, session)| (id.clone(), Arc::clone(session)))
            .collect();
        for (id, session) in sessions {
            self.unload_if_idle(&id, &session, grace).await;
        }
    }

    /// Unloads `session`, loaded as `id`, if a sweep marked it idle `grace`
    /// ago and it is idle still; marks it idle or in use otherwise.
    async fn unload_if_idle(&self, id: &SessionId, session: &Arc<LoadedSession>, grace: Duration) {
        // Both held until the session is torn down: the stop waits for the
        // first, and a request reopens the session only once it gets the
        // second.
        let _unloading = self.unloading.read().await;
        let _reopening = self.reopening.lock(id).await;
        let idle_since = {
            let mut turn = session.turn();
            // A turn or a subagent writes the session, or a client follows it.
            let busy =
                matches!(self.jobs.session_job(id), SessionJob::Busy(_)) || session.feed.followed();
            let now = Instant::now();
            let idle_since = match turn.residency {
                Residency::Unloaded => return,
                Residency::InUse | Residency::IdleSince(_) if busy => {
                    turn.residency = Residency::InUse;
                    return;
                }
                Residency::InUse => {
                    turn.residency = Residency::IdleSince(now);
                    return;
                }
                Residency::IdleSince(since) if now.duration_since(since) < grace => return,
                Residency::IdleSince(since) => since,
            };
            // Under the lock `begin_stop` takes, so a session the stop did
            // not drain is torn down before the stop's wait on `unloading`.
            let mut loaded = self.loaded();
            let current = loaded
                .get(id)
                .is_some_and(|loaded| Arc::ptr_eq(loaded, session));
            if self.stopping.is_cancelled() || !current {
                return;
            }
            loaded.remove(id);
            turn.residency = Residency::Unloaded;
            idle_since
        };
        // Free: no turn runs, and none starts on an unloaded session.
        let runtime = session.runtime.lock().await.take();
        if let Some(runtime) = runtime {
            runtime.shutdown().await;
        }
        tracing::info!(
            session = %id,
            job = %session.job,
            idle_for = ?idle_since.elapsed(),
            unload_idle_after = ?grace,
            "unloaded idle session"
        );
    }
}

#[cfg(test)]
#[path = "unload_tests.rs"]
mod tests;
