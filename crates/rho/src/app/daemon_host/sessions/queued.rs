//! A place in the queue on a fair tokio mutex, joined without waiting, so a
//! request can join under a std lock and wait once it let that lock go. A
//! prompt's turn and a settings change join the queue on their session's
//! runtime this way under the session's turn lock, so the order they take
//! the runtime in is the order they started under that lock (see
//! `Sessions::prompt` and `configure`).

use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, Waker},
};

use tokio::sync::{Mutex, OwnedMutexGuard};

/// A place in a [`Mutex`]'s queue, joined when created. The mutex is fair:
/// it goes to whoever joined first.
pub(super) enum Queued<T> {
    /// The mutex was free, so this holds it already.
    Held(OwnedMutexGuard<T>),
    /// Waits behind the holder and everyone who joined before.
    Waiting(Pin<Box<dyn Future<Output = OwnedMutexGuard<T>> + Send>>),
}

impl<T: Send + 'static> Queued<T> {
    /// Joins `lock`'s queue at once, without awaiting, so a caller may hold
    /// a std lock meanwhile.
    pub(super) fn join(lock: &Arc<Mutex<T>>) -> Self {
        // Unconstrained, so a task out of its coop budget still takes a
        // free mutex on this poll rather than leaving the queue unjoined.
        let mut waiting: Pin<Box<dyn Future<Output = OwnedMutexGuard<T>> + Send>> = Box::pin(
            tokio::task::coop::unconstrained(Arc::clone(lock).lock_owned()),
        );
        // One poll joins the queue: tokio's semaphore links a waiter into
        // its wait list on the first poll that finds the mutex held (tokio
        // 1.54 `batch_semaphore::Acquire::poll`), which `queued_tests` pins.
        // The waker is a no-op: whoever awaits the place polls it again with
        // its own, and finds the mutex if it was handed over meanwhile.
        match waiting
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(held) => Self::Held(held),
            Poll::Pending => Self::Waiting(waiting),
        }
    }

    /// Waits for the mutex, if this does not hold it yet.
    pub(super) async fn acquire(self) -> OwnedMutexGuard<T> {
        match self {
            Self::Held(held) => held,
            Self::Waiting(waiting) => waiting.await,
        }
    }
}

#[cfg(test)]
#[path = "queued_tests.rs"]
mod tests;
