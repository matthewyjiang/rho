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

use super::{worktree::Prune, LoadedSession, Sessions};
use crate::app::{daemon_host::jobs::SessionJob, session_runtime::SessionRuntime};

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
        let idle_since = {
            let mut turn = session.turn();
            let busy = matches!(self.jobs.session_job(id), SessionJob::Busy(_))
                || turn.hold.is_some()
                || session.feed.followed()
                || session.runtime_keeps_loaded();
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
            // A session the stop did not drain is torn down before the
            // stop's wait on `unloading`.
            if !self.evict(id, session, &mut turn) {
                return;
            }
            idle_since
        };
        // Free: no turn runs, and none starts on an unloaded session.
        let runtime = session.runtime.lock().await.take();
        // Evicted, so no turn can save it now. `None`: the stop took it.
        let saved = runtime.as_ref().is_none_or(SessionRuntime::is_saved);
        if let Some(runtime) = runtime {
            runtime.shutdown().await;
        }
        let idle_for = idle_since.elapsed();
        if saved {
            tracing::info!(
                session = %id,
                job = %session.job,
                ?idle_for,
                unload_idle_after = ?grace,
                "unloaded idle session"
            );
        } else {
            // Nothing on disk could reopen a session that saved nothing, so
            // its job goes with it, and its worktree when cleanup may
            // remove it: nothing would remember it after.
            let record = self.jobs.get(&session.job);
            self.jobs.remove(&session.job);
            if let Some(record) = record {
                self.prune_worktree(&record, Prune::Remove).await;
            }
            tracing::info!(
                session = %id,
                job = %session.job,
                ?idle_for,
                unload_idle_after = ?grace,
                "discarded unsaved idle session"
            );
        }
    }
}

impl Sessions {
    /// How many loaded sessions with no turn under way have work of their
    /// own a restart would cut short, which keeps an upgrade waiting
    /// (#1713): background work their prompts started, a queued settings
    /// change, or, for one a client follows, nothing saved yet, so a
    /// restarted daemon could not reopen it for that client. Turns under way
    /// are the job registry's to count.
    pub(in crate::app::daemon_host) fn upgrade_blockers(&self) -> usize {
        // Collected first: the turn lock comes before `loaded`, never after.
        let loaded: Vec<_> = self
            .loaded()
            .iter()
            .map(|(id, session)| (id.clone(), Arc::clone(session)))
            .collect();
        loaded
            .iter()
            .filter(|(id, session)| {
                !matches!(self.jobs.session_job(id), SessionJob::Busy(_))
                    && session.keeps_upgrade_waiting()
            })
            .count()
    }
}

impl LoadedSession {
    /// Whether restarting the daemon would cut short work of the session's
    /// own; see [`Sessions::upgrade_blockers`].
    fn keeps_upgrade_waiting(&self) -> bool {
        if self.turn().hold.is_some() {
            return true;
        }
        match self.runtime.try_lock() {
            Ok(runtime) => runtime.as_ref().is_some_and(|runtime| {
                runtime.has_background_work() || (!runtime.is_saved() && self.feed.followed())
            }),
            // A turn that just ended, or a change, still holds it.
            Err(_) => true,
        }
    }

    /// Whether the session's runtime keeps it loaded: background work its
    /// prompts started runs on or holds a result, which a teardown would
    /// stop or drop. So does a runtime a turn that just ended still holds.
    fn runtime_keeps_loaded(&self) -> bool {
        match self.runtime.try_lock() {
            Ok(runtime) => runtime
                .as_ref()
                .is_some_and(SessionRuntime::has_background_work),
            Err(_) => true,
        }
    }
}

#[cfg(test)]
#[path = "unload_tests.rs"]
mod tests;
