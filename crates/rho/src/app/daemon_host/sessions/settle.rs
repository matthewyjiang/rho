//! Whether a loaded session is settled: no turn is under way, and neither
//! the daemon nor the session's host will start one for work the session
//! waits on. Its clients follow it as [`rho_protocol::SessionEvent::Activity`]
//! (see `feed`), which `wait` of `rho mcp serve` returns on.
//!
//! Settled is not "nothing runs": a session whose prompts hold until their
//! work finishes (`BackgroundWait::HoldPrompt`) is never woken, so it
//! settles once its turn ends, even a cancelled one that left a subagent
//! running. A session the daemon or its host wakes is active while work it
//! waits on (see `awaited`) runs or holds a result no turn took, unless the
//! waker gave up: a delivery turn that ended without taking its result,
//! with nothing left running, is not started again (see `idle_wake`), and a
//! host whose wake watcher could not be armed never wakes the session (see
//! `t3_wake`). A turn starting clears that.
//!
//! The level is reported as the session is added, as each turn starts (by
//! the feed, ahead of the turn's state) and ends, as its background work
//! starts or ends, and as its waker gives up. Each report reads the work
//! under the feed's lock, taking only the work's own locks, never the
//! turn's or the runtime's.

use std::sync::atomic::Ordering;

use super::{LoadedSession, TurnSlot};

impl LoadedSession {
    /// Tells the session's clients whether it is settled, if that changed
    /// since they were last told.
    pub(super) fn report_activity(&self) {
        self.feed.update_activity(|| self.wake_due());
    }

    /// The daemon will not wake the session for the result its work holds
    /// (see `idle_wake`): it settles, and its clients hear so at once.
    pub(super) fn give_up_waking(&self) {
        self.mark_wake_given_up();
        self.report_activity();
    }

    /// As [`Self::give_up_waking`], for a caller whose turn's end reports
    /// the session's activity next, as a T3 watcher that could not be
    /// armed does (see `turn_wake`). Call before that end is reported.
    pub(super) fn mark_wake_given_up(&self) {
        self.wake_gave_up.store(true, Ordering::Relaxed);
    }

    /// A client asked that the session's results wait for its next turn
    /// ([`rho_protocol::SESSION_BACKGROUND_HOLD`]): the daemon starts no
    /// turn to deliver them meanwhile, and its background work, running or
    /// ended, keeps it neither loaded nor an upgrade waiting (see `unload`)
    /// nor from settling, which its clients hear at once.
    pub(super) fn hold_wake(&self) {
        // Under the turn lock, which a delivery turn reads the hold under
        // and every turn clears it under as it starts: a delivery turn
        // either never starts, or started already and leaves the hold set
        // for its end, so cancelling it delivers nothing either.
        let turn = self.turn();
        self.hold_wake_under(&turn);
        drop(turn);
        self.give_up_waking();
    }

    /// As [`Self::hold_wake`], for a caller that holds the turn lock, its
    /// guard's slot passed as proof, and gives up waking the session itself
    /// once it lets go.
    pub(super) fn hold_wake_under(&self, _turn: &TurnSlot) {
        self.wake_held.store(true, Ordering::Relaxed);
    }

    /// Whether the session's results wait for its next turn (see
    /// [`Self::hold_wake`]).
    pub(super) fn wake_held(&self) -> bool {
        self.wake_held.load(Ordering::Relaxed)
    }

    /// A turn starts and takes the session's results, so whatever gave up
    /// waking the session for them, or held them, looks again once it ends.
    /// Call under the turn lock as the turn starts, before its activity is
    /// reported.
    pub(super) fn rearm_wake(&self) {
        self.wake_gave_up.store(false, Ordering::Relaxed);
        self.wake_held.store(false, Ordering::Relaxed);
    }

    /// Whether the daemon or the session's host will start a turn for work
    /// the session waits on.
    fn wake_due(&self) -> bool {
        !self.wake_gave_up.load(Ordering::Relaxed)
            && self
                .background
                .awaited()
                .is_some_and(|work| work.outstanding())
    }
}

#[cfg(test)]
#[path = "settle_tests.rs"]
mod tests;
