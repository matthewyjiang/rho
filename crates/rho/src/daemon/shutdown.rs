//! How the daemon's serving ends: a stop, after which the process exits, or
//! a restart, after which it re-executes its executable as the same process
//! (#1713).

use std::sync::{Arc, Mutex, PoisonError};

use rho_sdk::CancellationToken;

/// What the daemon does once it stopped serving.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ending {
    /// Release the lock, remove the socket, and exit.
    Stop,
    /// Keep the lock and the listening socket, and re-execute in place.
    Restart,
}

/// Asks the daemon's transport to stop serving, and records how the daemon
/// ends. Clones share one request.
///
/// A stop wins over a restart: a restart asked for once a stop was never
/// replaces it, while a stop asked for during a restart turns it into a
/// stop, which leaves clients told the daemon is restarting to start a new
/// one.
#[derive(Clone, Debug, Default)]
pub(crate) struct Shutdown {
    requested: CancellationToken,
    ending: Arc<Mutex<Option<Ending>>>,
}

impl Shutdown {
    /// Ends serving as `ending`, as far as an earlier request lets it, and
    /// calls `prepare` with the ending decided before the transport starts
    /// closing connections, so it can tell each client why first.
    pub(crate) fn request(&self, ending: Ending, prepare: impl FnOnce(Ending)) {
        let decided = {
            let mut current = self.ending.lock().unwrap_or_else(PoisonError::into_inner);
            let decided = match (*current, ending) {
                (None | Some(Ending::Restart), ending) => ending,
                (Some(Ending::Stop), _) => Ending::Stop,
            };
            *current = Some(decided);
            decided
        };
        prepare(decided);
        self.requested.cancel();
    }

    /// Whether serving was asked to end.
    pub(crate) fn is_requested(&self) -> bool {
        self.requested.is_cancelled()
    }

    /// Resolves once serving was asked to end.
    pub(crate) async fn requested(&self) {
        self.requested.cancelled().await;
    }

    /// How the daemon ends, as requested so far; a stop until a request
    /// said otherwise.
    pub(crate) fn ending(&self) -> Ending {
        self.ending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .unwrap_or(Ending::Stop)
    }
}
