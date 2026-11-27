use std::{future::Future, task::Poll};

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

    /// Resolves after cancellation has been requested: on the first poll
    /// after [`Self::cancel`], including a wait created before it.
    ///
    /// Every poll reads the flag before it waits on the channel. A wait
    /// parked on the channel would otherwise stay pending after the cancel
    /// while the task's cooperative budget is spent, and in the moment after
    /// the flag is set and before the channel wakes its waiters.
    pub async fn cancelled(&self) {
        let mut changes = self.cancelled.clone();
        let changed = changes.wait_for(|cancelled| *cancelled);
        tokio::pin!(changed);
        std::future::poll_fn(|cx| {
            if self.is_cancelled() {
                return Poll::Ready(());
            }
            // `self` holds a sender, so the channel cannot close under it.
            changed.as_mut().poll(cx).map(drop)
        })
        .await;
    }
}

#[cfg(test)]
#[path = "cancellation_tests.rs"]
mod tests;
