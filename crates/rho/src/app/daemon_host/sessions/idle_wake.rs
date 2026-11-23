//! Wakes an idle session whose client asked for it (`idle_wake: turn`, as
//! the TUI does) when its background work finishes: the daemon starts a
//! turn of its own, with no prompt, whose first boundary gives the model
//! the result.
//!
//! Such a session's prompts do not wait for the subagents, workflows and
//! background tasks they start (`BackgroundWait::HostTurns`), so a turn
//! ends while they run on. One watcher per loaded session, started as the
//! session is added, waits for a result and for the session to be idle. A
//! result that arrives during a turn is delivered at that turn's
//! boundaries; one that turn left behind is delivered once it ended. A
//! background service's exit is no result: it wakes nothing.
//!
//! A subagent's request that its parent act (`request_parent_action`) is a
//! result too. Its informational notices (`message_parent`) are not: they
//! start no turn, and go with the next one.
//!
//! A delivery turn that ends without taking its result, as one that failed
//! first or was cancelled, is not started again at once, which would loop:
//! the watcher waits for another result or turn. Once nothing it waits on
//! still runs, no result can come to start one, so it gives up, the
//! session settles (see `settle`), and only a later turn's end wakes the
//! watcher again.
//!
//! The watcher holds the session weakly: the session stays loaded while
//! work runs or a result waits (see `unload`), and the watcher ends once
//! the session is dropped.

use std::sync::{Arc, Weak};

use rho_protocol::{SessionSetup, TurnId};
use rho_sdk::CancellationToken;
use tokio::sync::watch;

use super::{
    awaited::AwaitedWork, DaemonTurn, Entered, LoadedSession, OpenWith, Sessions, TurnTask,
};
use crate::app::session_runtime::{BackgroundWait, SessionRuntime};

/// A session's side of its watcher.
pub(super) struct IdleWatch {
    /// How many of the session's turns ended.
    turns_ended: watch::Sender<u64>,
    /// Cancelled when the session is dropped.
    dropped: CancellationToken,
}

impl IdleWatch {
    /// The wake state of the session `runtime` runs, when the daemon wakes
    /// it.
    pub(super) fn of(runtime: &SessionRuntime) -> Option<Self> {
        match runtime.background_wait() {
            BackgroundWait::HoldPrompt | BackgroundWait::HostWakes => None,
            BackgroundWait::HostTurns => Some(Self {
                turns_ended: watch::Sender::new(0),
                dropped: CancellationToken::new(),
            }),
        }
    }

    /// Call once a turn's end is recorded, so the session can start
    /// another.
    pub(super) fn turn_ended(&self) {
        self.turns_ended.send_modify(|ended| *ended += 1);
    }
}

impl Drop for IdleWatch {
    fn drop(&mut self) {
        self.dropped.cancel();
    }
}

impl Sessions {
    /// Starts `session`'s watcher, if the daemon wakes it. Call once, as
    /// the session is added.
    pub(super) fn watch_idle_results(&self, session: &Arc<LoadedSession>) {
        let Some(wake) = &session.idle_wake else {
            return;
        };
        let watcher = watch_results(
            self.myself.clone(),
            Arc::downgrade(session),
            session.background.replacements(),
            wake.turns_ended.subscribe(),
        );
        let dropped = wake.dropped.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = dropped.cancelled() => {}
                () = watcher => {}
            }
        });
    }

    /// Starts a turn that delivers `session`'s waiting results, unless a
    /// turn is under way, the session is leaving, or the daemon stops.
    /// Returns the turn it started, if it started one.
    fn deliver_idle_results(
        &self,
        session: &Arc<LoadedSession>,
        background: &AwaitedWork,
    ) -> Option<TurnId> {
        if self.stopping.is_cancelled() {
            return None;
        }
        let mut slot = session.enter()?;
        // Under the turn lock, which every turn starts under: only a turn
        // takes a result, so one that waits now is this turn's to deliver.
        if !background.has_result() {
            return None;
        }
        let grant = self.jobs.grant_of(&session.job)?;
        let with = OpenWith {
            // The environment and grant the last prompt brought.
            environment: session.environment.current(),
            setup: SessionSetup::default(),
            grant,
        };
        // A busy session delivers at its turn's boundaries, and the watcher
        // looks again once that turn ended.
        let Ok(Entered::Started(turn)) = self.start_turn(
            session, &mut slot, &with, /*key*/ None, /*limit*/ None,
        ) else {
            return None;
        };
        drop(slot);
        let id = turn.id.clone();
        TurnTask {
            session: Arc::clone(session),
            turn,
            stopping: self.stopping.clone(),
        }
        .spawn_daemon(DaemonTurn::delivery());
        Some(id)
    }
}

/// Starts a delivery turn on `session` each time a result of the work it
/// waits on waits with the session idle, until the session or the daemon's
/// `sessions` are gone. Reads that work from the session's tools as they are
/// now, and again after `replacements` says a configure replaced them, so it
/// waits on the same managers that decide the session's activity.
async fn watch_results(
    sessions: Weak<Sessions>,
    session: Weak<LoadedSession>,
    mut replacements: watch::Receiver<u64>,
    mut turns_ended: watch::Receiver<u64>,
) {
    loop {
        // Marked seen before the read, so a replacement after it is not.
        replacements.borrow_and_update();
        let Some(mut background) = awaited(&session) else {
            return;
        };
        tokio::select! {
            () = background.clone().result_ready() => {}
            changed = replacements.changed() => {
                if changed.is_err() {
                    return;
                }
                continue;
            }
            // A turn that ended may have put back a result it took and
            // never delivered, which no result's arrival announces.
            changed = turns_ended.changed() => {
                if changed.is_err() {
                    return;
                }
                continue;
            }
        }
        // Only a turn that ends from here on frees the session again.
        turns_ended.borrow_and_update();
        // Subscribed before the attempt, so no result that follows it is
        // missed.
        let another = background.next_result();
        let started = {
            let (Some(sessions), Some(session)) = (sessions.upgrade(), session.upgrade()) else {
                return;
            };
            sessions.deliver_idle_results(&session, &background)
        };
        // The delivery turn that ended without taking the result, if any:
        // it failed first or was cancelled. Starting it again at once would
        // loop.
        let undelivered = match started {
            Some(turn) => {
                if turns_ended.changed().await.is_err() {
                    return;
                }
                // Every turn refreshes the session's sources as it ends (see
                // `LoadedSession::refresh_options`), which is no replacement
                // to look again for: that would start this delivery again at
                // once, and one that ends without taking the result would
                // loop. The sources are read again here instead.
                replacements.borrow_and_update();
                let Some(now) = awaited(&session) else {
                    return;
                };
                background = now;
                if !background.has_result() {
                    continue;
                }
                Some(turn)
            }
            None => None,
        };
        // Nothing to start now: a turn is under way or just took the
        // result, the start was refused, or the delivery turn ended without
        // taking it. Look again once a turn ends or another result arrives.
        let mut another = std::pin::pin!(another);
        // Whatever is ready already comes before giving up: a turn that
        // ended, another result, or a replacement. Giving up first could
        // publish a spurious `settled` while the next pass starts a wake.
        let ready = tokio::select! {
            biased;
            changed = turns_ended.changed() => Some(changed.is_ok()),
            () = &mut another => Some(true),
            changed = replacements.changed() => Some(changed.is_ok()),
            () = std::future::ready(()) => None,
        };
        let alive = match ready {
            Some(alive) => alive,
            None => {
                // Subscribed before giving up, so news a tick queues once
                // `settled` is published still wakes this loop.
                let news = background.outside_news();
                let gave_up = match (&undelivered, session.upgrade()) {
                    (Some(turn), Some(session)) => try_give_up(&session, &background, turn),
                    _ => false,
                };
                if gave_up {
                    // Nothing awaited runs and no turn is under way, so a
                    // new result can only come from work a later turn
                    // starts, and that turn's end wakes this loop, or from
                    // a watched pull request, which needs no turn. What
                    // `another` hears meanwhile, a service's exit or a
                    // child's notice, starts no turn.
                    tokio::select! {
                        changed = turns_ended.changed() => changed.is_ok(),
                        changed = replacements.changed() => changed.is_ok(),
                        () = news => true,
                    }
                } else {
                    tokio::select! {
                        changed = turns_ended.changed() => changed.is_ok(),
                        () = another => true,
                        changed = replacements.changed() => changed.is_ok(),
                    }
                }
            }
        };
        if !alive {
            return;
        }
    }
}

/// The work `session` waits on, from its tools now; `None` once it is gone
/// or nothing wakes it.
fn awaited(session: &Weak<LoadedSession>) -> Option<AwaitedWork> {
    session.upgrade()?.background.awaited()
}

/// Gives up waking `session` once its delivery turn `turn` ended without
/// taking its result and nothing `background` waits on still runs, so no
/// result can arrive to start another: the session settles. Returns whether
/// it gave up; it does not when a turn started since, or the result was
/// taken or more may come.
pub(super) fn try_give_up(
    session: &LoadedSession,
    background: &AwaitedWork,
    turn: &TurnId,
) -> bool {
    // Under the turn lock, which every turn starts under, so no turn
    // started since: a later one rearms the wake as it starts, and one that
    // ended since frees the watcher, which then looks again.
    // The turn must also have ended: the watcher hears a turn end that may
    // be the one before `turn`, sent after that turn let the lock go.
    let _slot = session.turn();
    let ended = session.jobs.get(&session.job).is_some_and(|job| {
        !job.state.in_progress() && job.turn.is_some_and(|last| &last.id == turn)
    });
    let gives_up = ended && background.has_result() && !background.running();
    if gives_up {
        session.give_up_waking();
    }
    gives_up
}
