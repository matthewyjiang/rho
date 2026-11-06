//! The channel between one session's live events and one attached client
//! (#1657).
//!
//! The daemon keeps a [`Publisher`] per attached client and hands the
//! matching [`Subscription`] to [`crate::serve_connection`] in its
//! [`crate::Service::attach_session`] answer. The connection forwards the
//! subscription's events as [`crate::methods::SESSION_EVENT`] notifications.

use std::{
    collections::VecDeque,
    sync::{Arc, OnceLock},
};

use tokio::sync::mpsc::{
    self,
    error::{TryRecvError, TrySendError},
};

use crate::methods::{DetachReason, Seq, SessionEvent};

/// Opens a subscription that first sends `replay`, the events a client
/// attaching or reattaching has not seen, then holds up to `backlog` live
/// events its connection has not yet taken. The replay is already in memory
/// and does not count against the backlog, so a long one never drops the
/// client. A subscriber further than `backlog` live events behind is dropped
/// with [`DetachReason::Lagged`] rather than buffered without end.
///
/// # Panics
///
/// When `backlog` is 0.
pub fn subscription(backlog: usize, replay: Vec<(Seq, SessionEvent)>) -> (Publisher, Subscription) {
    let (events, receiver) = mpsc::channel(backlog);
    let reason = Arc::new(OnceLock::new());
    let publisher = Publisher {
        events,
        reason: Arc::clone(&reason),
    };
    let subscription = Subscription {
        replay: replay.into(),
        events: receiver,
        reason,
    };
    (publisher, subscription)
}

/// The daemon's end of one subscription: publishes one session's live events
/// to one attached client.
///
/// Dropping it ends the subscription once the client has the events already
/// published, without a [`crate::methods::SESSION_DETACHED`] notification;
/// [`Publisher::close`] ends it with one.
#[derive(Debug)]
pub struct Publisher {
    events: mpsc::Sender<(Seq, SessionEvent)>,
    /// Set once: by the first lag, or by [`Publisher::close`].
    reason: Arc<OnceLock<DetachReason>>,
}

/// Why [`Publisher::try_publish`] did not queue an event. Either way, drop
/// the publisher: the subscription takes no more events.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Published {
    /// The subscription's backlog is full. The publisher has closed it with
    /// [`DetachReason::Lagged`], so the client gets the events it already
    /// has queued, then a detached notification, once the publisher drops.
    #[error("the subscriber fell a whole backlog behind")]
    Lagged,
    /// The subscription is gone: the client detached, attached again, or
    /// left.
    #[error("the subscriber is gone")]
    Gone,
}

impl Publisher {
    /// Queues one event. Never waits, so a turn never blocks on a client.
    ///
    /// Once it has reported [`Published::Lagged`], it keeps doing so even
    /// after the backlog drains, so the client never sees a gap in `seq`.
    pub fn try_publish(&self, seq: Seq, event: SessionEvent) -> Result<(), Published> {
        if self.reason.get().is_some() {
            return Err(Published::Lagged);
        }
        match self.events.try_send((seq, event)) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                // Only this publisher sets the reason, and only once.
                let _ = self.reason.set(DetachReason::Lagged);
                Err(Published::Lagged)
            }
            Err(TrySendError::Closed(_)) => Err(Published::Gone),
        }
    }

    /// Resolves once the subscription is gone, as [`Published::Gone`]
    /// reports, so a publisher whose client left can be dropped without
    /// waiting for the next event.
    pub async fn closed(&self) {
        self.events.closed().await;
    }

    /// Ends the subscription with `reason`, such as
    /// [`DetachReason::DaemonStopping`]: the client gets the events already
    /// published, then a detached notification. A subscription that already
    /// lagged keeps [`DetachReason::Lagged`].
    pub fn close(self, reason: DetachReason) {
        let _ = self.reason.set(reason);
    }
}

/// The connection's end of one subscription. [`crate::serve_connection`]
/// forwards its events, then its detach reason, if any.
#[derive(Debug)]
pub struct Subscription {
    /// Sent before any live event.
    replay: VecDeque<(Seq, SessionEvent)>,
    events: mpsc::Receiver<(Seq, SessionEvent)>,
    reason: Arc<OnceLock<DetachReason>>,
}

impl Subscription {
    /// The next event, replayed ones first, or `None` once the publisher is
    /// gone and every published event was taken.
    pub(crate) async fn next(&mut self) -> Option<(Seq, SessionEvent)> {
        match self.replay.pop_front() {
            Some(event) => Some(event),
            None => self.events.recv().await,
        }
    }

    /// The next event already published, without waiting.
    pub(crate) fn try_next(&mut self) -> Next {
        if let Some((seq, event)) = self.replay.pop_front() {
            return Next::Event(seq, event);
        }
        match self.events.try_recv() {
            Ok((seq, event)) => Next::Event(seq, event),
            Err(TryRecvError::Empty) => Next::Empty,
            Err(TryRecvError::Disconnected) => Next::Ended,
        }
    }

    /// Why the publisher ended the subscription, once [`Self::next`] has
    /// returned `None` or [`Self::try_next`] [`Next::Ended`]; `None` when it
    /// dropped without a reason.
    pub(crate) fn reason(&self) -> Option<DetachReason> {
        self.reason.get().copied()
    }
}

/// What [`Subscription::try_next`] found.
pub(crate) enum Next {
    Event(Seq, SessionEvent),
    /// Nothing published yet; the publisher may still publish.
    Empty,
    /// The publisher is gone and every event it published was taken.
    Ended,
}

#[cfg(test)]
#[path = "subscription_tests.rs"]
mod tests;
