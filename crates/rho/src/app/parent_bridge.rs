//! Rebindable child-to-parent channel shared by delegated-agent bridges.

use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

/// Sender port from delegated children to whichever parent session is listening.
///
/// Delegated questionnaires and notices need the same lifecycle: an interactive
/// parent binds a receiver, each child looks the sender up per message, and
/// unbinding makes later child sends fail closed instead of queueing for a
/// parent that will never read them. Owners keep only their delivery policy -
/// blocking request/response for questionnaires, fire-and-forget for notices -
/// and hold one of these for the mechanics.
pub(crate) struct ParentBridge<T> {
    inner: Arc<Inner<T>>,
}

struct Inner<T> {
    sender: Mutex<Option<mpsc::Sender<T>>>,
    capacity: usize,
}

// Derived `Clone` would demand `T: Clone`; the bridge only shares an `Arc`.
impl<T> Clone for ParentBridge<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T> ParentBridge<T> {
    /// Creates an unbound bridge whose queue holds `capacity` messages.
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Inner {
                sender: Mutex::new(None),
                capacity,
            }),
        }
    }

    /// Installs the parent receiver. Replaces any previous binding.
    pub(crate) fn bind_parent(&self) -> mpsc::Receiver<T> {
        let (sender, receiver) = mpsc::channel(self.inner.capacity);
        *self.sender_slot() = Some(sender);
        receiver
    }

    /// Drops the parent binding so later child sends fail closed.
    pub(crate) fn unbind_parent(&self) {
        *self.sender_slot() = None;
    }

    /// True while an interactive parent is listening.
    pub(crate) fn is_bound(&self) -> bool {
        self.sender_slot().is_some()
    }

    /// Clone of the live sender, or `None` when no parent is bound.
    pub(crate) fn sender(&self) -> Option<mpsc::Sender<T>> {
        self.sender_slot().clone()
    }

    fn sender_slot(&self) -> std::sync::MutexGuard<'_, Option<mpsc::Sender<T>>> {
        self.inner.sender.lock().expect("parent bridge sender lock")
    }
}
