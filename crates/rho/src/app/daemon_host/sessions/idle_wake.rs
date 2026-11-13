//! Wakes an idle session whose client asked for it (`idle_wake: turn`, as
//! the TUI does) when its background work finishes: the daemon starts a
//! turn of its own, with no prompt, whose first boundary gives the model
//! the result.
//!
//! Such a session's prompts do not wait for the subagents, workflows and
//! background processes they start (`BackgroundWait::HostTurns`), so a turn
//! ends while they run on. One watcher per loaded session, started as the
//! session is added, waits for a result and for the session to be idle. A
//! result that arrives during a turn is delivered at that turn's
//! boundaries; one that turn left behind is delivered once it ended.
//!
//! A subagent's request that its parent act (`request_parent_action`) is a
//! result too. Its informational notices (`message_parent`) are not: they
//! start no turn, and go with the next one.
//!
//! The watcher holds the session weakly: the session stays loaded while
//! work runs or a result waits (see `unload`), and the watcher ends once
//! the session is dropped.

use std::sync::{Arc, Weak};

use rho_protocol::SessionSetup;
use rho_sdk::CancellationToken;
use tokio::sync::watch;

use super::{t3_wake::Background, Entered, LoadedSession, OpenWith, Sessions, TurnTask};
use crate::app::session_runtime::{BackgroundWait, SessionRuntime};

/// A session's side of its watcher.
pub(super) struct IdleWatch {
    background: Background,
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
                background: Background::of(runtime.tools(), runtime.session_id().to_string())
                    .with_subagents(runtime.tools()),
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
            wake.background.clone(),
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
    /// Returns whether it started one.
    fn deliver_idle_results(&self, session: &Arc<LoadedSession>, background: &Background) -> bool {
        if self.stopping.is_cancelled() {
            return false;
        }
        let Some(mut slot) = session.enter() else {
            return false;
        };
        // Under the turn lock, which every turn starts under: only a turn
        // takes a result, so one that waits now is this turn's to deliver.
        if !background.has_result() {
            return false;
        }
        let Some(grant) = self.jobs.grant_of(&session.job) else {
            return false;
        };
        let with = OpenWith {
            // The environment and grant the last prompt brought.
            environment: session.environment.current(),
            setup: SessionSetup::default(),
            grant,
        };
        // A busy session delivers at its turn's boundaries, and the watcher
        // looks again once that turn ended.
        let Ok(Entered::Started(turn)) =
            self.start_turn(session, &mut slot, &with, /*key*/ None)
        else {
            return false;
        };
        drop(slot);
        TurnTask {
            session: Arc::clone(session),
            turn,
            stopping: self.stopping.clone(),
            limit: None,
        }
        .spawn_delivery();
        true
    }
}

/// Starts a delivery turn on `session` each time a result of `background`
/// waits with the session idle, until the session or the daemon's
/// `sessions` are gone.
async fn watch_results(
    sessions: Weak<Sessions>,
    session: Weak<LoadedSession>,
    background: Background,
    mut turns_ended: watch::Receiver<u64>,
) {
    loop {
        tokio::select! {
            () = background.clone().result_ready() => {}
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
        if started {
            if turns_ended.changed().await.is_err() {
                return;
            }
            if !background.has_result() {
                continue;
            }
            // The turn ended without taking the result: it failed first or
            // was cancelled. Starting it again at once would loop.
        }
        // Nothing to start now: a turn is under way or just took the
        // result, or the start was refused. Look again once a turn ends or
        // another result arrives.
        tokio::select! {
            changed = turns_ended.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            () = another => {}
        }
    }
}
