use tokio::sync::watch;

/// Cooperative cancellation shared by a run and all work nested beneath it.
///
/// Cancellation is idempotent. A clone observes cancellation requested through
/// any other clone, including clones held by provider requests, tool calls,
/// host-input requests, and compaction work. Dropping a token does not cancel
/// the operation because another owner may still be using it.
#[derive(Clone, Debug)]
pub struct CancellationToken {
    cancelled: watch::Receiver<bool>,
    cancel: watch::Sender<bool>,
}

impl Default for CancellationToken {
    fn default() -> Self {
        let (cancel, cancelled) = watch::channel(false);
        Self { cancelled, cancel }
    }
}

impl CancellationToken {
    /// Creates a token that has not been cancelled.
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cooperative cancellation.
    pub fn cancel(&self) {
        self.cancel.send_replace(true);
    }

    /// Returns whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        *self.cancelled.borrow()
    }

    /// Resolves after cancellation has been requested.
    pub async fn cancelled(&self) {
        let mut cancelled = self.cancelled.clone();
        if *cancelled.borrow_and_update() {
            return;
        }
        while cancelled.changed().await.is_ok() {
            if *cancelled.borrow_and_update() {
                return;
            }
        }
    }
}

#[cfg(test)]
#[path = "cancellation_tests.rs"]
mod tests;
