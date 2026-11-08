//! Unloading idle sessions (#1661). A loaded session holds its file's lease,
//! which keeps every other rho process from opening it, and its MCP
//! servers. Once it sits idle for the daemon's grace, the daemon tears it
//! down; the next prompt or attach reopens it from its file. A session that
//! has saved nothing yet has no file to reopen from, so it is discarded
//! instead: torn down the same way, and its job forgotten, so the daemon
//! lists it no more and a request for it finds nothing saved.
//!
//! A session is busy, never idle, while a turn or a subagent writes it, a
//! settings change is queued on it or applying, a delete holds it, a client
//! follows it, or background work its prompts started runs on or holds a
//! result.
//!
//! A sweep every grace marks each idle session idle from then, and unloads
//! or discards one a sweep already marked a grace ago, so a session leaves
//! between one and two graces after it went idle. Sweeps mark a busy
//! session in use, and so do a prompt and an attach. Each happens under the
//! session's turn lock, which the sweep decides under, so a request either
//! uses the session before the sweep sees it, keeping it, or finds it gone
//! and looks it up again: that reopens an unloaded session, and finds
//! nothing saved for a discarded one.

use std::{sync::Arc, time::Duration};

use rho_sdk::SessionId;
use tokio::time::Instant;

use super::{LoadedSession, Sessions};
use crate::app::daemon_host::jobs::SessionJob;

/// How long a session sits idle before the daemon unloads or discards it,
/// unless `rho daemon run --unload-idle-after` says otherwise.
//
// Reopening costs little, so this need only span a client's reconnect, such
// as a TUI restart or a frontend switching back to a session. Measured on a
// debug build, 15 sessions each, as cold minus warm attach: 7 ms median
// (26 ms max) with the fixture config, 10 ms (28 ms) with a real config,
// 21 ms (41 ms) with a stdio MCP server added. A tripwire past that, not a
// measured reconnect gap. The price of a longer one: another process, such
// as the TUI, is refused an idle session as in use for up to twice this.
// `rho daemon run --help` and docs/daemon.md state it; keep them in step.
pub(in crate::app) const UNLOAD_IDLE_AFTER: Duration = Duration::from_secs(60);

/// Whether a session is in use, idle, or unloaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Residency {
    /// Used since the last sweep, or a turn runs or a client follows it.
    InUse,
    /// Idle at every sweep since this one.
    IdleSince(Instant),
    /// Unloaded or discarded: a request that found the session before then
    /// looks it up again.
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

/// Whether anything keeps a session loaded, as a sweep finds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Idleness {
    /// A turn, a hold, a follower, or background work keeps it.
    Busy,
    /// Nothing keeps it; once idle for the grace it leaves this way.
    Idle(Departure),
}

/// How an idle session leaves the daemon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Departure {
    /// Torn down; the next request reopens it from its file.
    Unload,
    /// Torn down and its job forgotten: it saved nothing, so nothing could
    /// reopen it.
    Discard,
}

impl Sessions {
    /// Sweeps the loaded sessions every `grace`, unloading or discarding
    /// those idle for it, until the daemon starts stopping.
    pub(in crate::app::daemon_host) async fn unload_idle(&self, grace: Duration) {
        loop {
            tokio::select! {
                () = self.stopping.cancelled() => return,
                () = tokio::time::sleep(grace) => {}
            }
            self.sweep(grace).await;
        }
    }

    /// Marks each loaded session idle, or unloads or discards it once it was
    /// idle for `grace`.
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

    /// Unloads `session`, loaded as `id`, or discards it when it saved
    /// nothing, if a sweep marked it idle `grace` ago and it is idle still;
    /// marks it idle or in use otherwise.
    async fn unload_if_idle(&self, id: &SessionId, session: &Arc<LoadedSession>, grace: Duration) {
        // Both held from the decision until the session is torn down and a
        // discarded one's job forgotten. The stop waits for the first. A
        // reopen waits for the second, since the old runtime holds the
        // session's file until it is torn down.
        let _unloading = self.unloading.read().await;
        let _reopening = self.reopening.lock(id).await;
        let (idle_since, departure) = {
            let mut turn = session.turn();
            let idleness = if matches!(self.jobs.session_job(id), SessionJob::Busy(_))
                || turn.hold.is_some()
                || session.feed.followed()
            {
                Idleness::Busy
            } else {
                session.runtime_idleness()
            };
            let now = Instant::now();
            let left = match (turn.residency, idleness) {
                (Residency::Unloaded, _) => return,
                (Residency::InUse | Residency::IdleSince(_), Idleness::Busy) => {
                    turn.residency = Residency::InUse;
                    return;
                }
                (Residency::InUse, Idleness::Idle(_)) => {
                    turn.residency = Residency::IdleSince(now);
                    return;
                }
                (Residency::IdleSince(since), Idleness::Idle(_))
                    if now.duration_since(since) < grace =>
                {
                    return
                }
                (Residency::IdleSince(since), Idleness::Idle(departure)) => (since, departure),
            };
            // A session the stop did not drain is torn down before the
            // stop's wait on `unloading`.
            if !self.evict(id, session, &mut turn) {
                return;
            }
            left
        };
        // Free: no turn runs, and none starts on an unloaded session.
        let runtime = session.runtime.lock().await.take();
        if let Some(runtime) = runtime {
            runtime.shutdown().await;
        }
        match departure {
            Departure::Unload => tracing::info!(
                session = %id,
                job = %session.job,
                idle_for = ?idle_since.elapsed(),
                unload_idle_after = ?grace,
                "unloaded idle session"
            ),
            Departure::Discard => {
                self.jobs.remove(&session.job);
                tracing::info!(
                    session = %id,
                    job = %session.job,
                    idle_for = ?idle_since.elapsed(),
                    unload_idle_after = ?grace,
                    "discarded unsaved idle session"
                );
            }
        }
    }
}

impl LoadedSession {
    /// Whether the session's runtime keeps it loaded: background work its
    /// prompts started runs on or holds a result, which a teardown would
    /// stop or drop, or a turn that just ended still holds the runtime.
    /// Else how it leaves: discarded when it has saved nothing yet.
    fn runtime_idleness(&self) -> Idleness {
        let Ok(runtime) = self.runtime.try_lock() else {
            return Idleness::Busy;
        };
        match runtime.as_ref() {
            Some(runtime) if runtime.has_background_work() => Idleness::Busy,
            Some(runtime) if !runtime.is_saved() => Idleness::Idle(Departure::Discard),
            // `None`: the daemon's stop took the runtime, and the eviction
            // yields to the stop.
            Some(_) | None => Idleness::Idle(Departure::Unload),
        }
    }
}

#[cfg(test)]
#[path = "unload_tests.rs"]
mod tests;
