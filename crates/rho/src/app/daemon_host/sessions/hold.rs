//! What holds a session between turns: its queued settings changes or its
//! delete, marked on its turn lock so no request holds the lock across its
//! awaits.

use std::{num::NonZeroUsize, sync::Arc};

use super::{LoadedSession, TurnSlot};

/// What holds a session: its queued settings changes or its delete, which
/// exclude each other. The turn lock is a std mutex no request holds across
/// its awaits, so a request marks its hold in [`TurnSlot::hold`] instead of
/// holding the lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Hold {
    /// This many changes to the session's settings are queued on its
    /// runtime or applying (see [`super::configure`]). A prompt still starts a
    /// turn, which queues on the runtime behind them.
    Configuring(NonZeroUsize),
    /// A delete is unloading the session to delete its files; no turn
    /// starts and no change queues meanwhile.
    Deleting,
}

/// A request's part of a session's [`Hold`], freed when dropped, whether
/// the request finished, failed, or was dropped.
pub(super) struct Held(Arc<LoadedSession>);

impl Held {
    /// Counts one more change queued on `session`, or `None` while a delete
    /// holds it. `turn` is `session`'s turn lock, held.
    pub(super) fn change(session: &Arc<LoadedSession>, turn: &mut TurnSlot) -> Option<Self> {
        let queued = match turn.hold {
            None => NonZeroUsize::MIN,
            Some(Hold::Configuring(queued)) => queued
                .checked_add(1)
                .expect("each queued change is a request in flight"),
            Some(Hold::Deleting) => return None,
        };
        turn.hold = Some(Hold::Configuring(queued));
        Some(Self(Arc::clone(session)))
    }

    /// Holds `session` for its delete. `turn` is `session`'s turn lock,
    /// held, with nothing holding the session.
    pub(super) fn delete(session: &Arc<LoadedSession>, turn: &mut TurnSlot) -> Self {
        debug_assert_eq!(turn.hold, None, "a delete holds only a free session");
        turn.hold = Some(Hold::Deleting);
        Self(Arc::clone(session))
    }
}

impl Drop for Held {
    /// Takes the session's turn lock, a std mutex: never drop a `Held`
    /// while holding that lock's guard, which would deadlock.
    fn drop(&mut self) {
        let mut turn = self.0.turn();
        // Changes and a delete exclude each other, so the hold in place is
        // the kind this one is part of.
        turn.hold = match turn.hold {
            Some(Hold::Configuring(queued)) => {
                NonZeroUsize::new(queued.get() - 1).map(Hold::Configuring)
            }
            Some(Hold::Deleting) | None => None,
        };
    }
}
