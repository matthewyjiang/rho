//! The channel between one session's live events and one attached client
//! (#1657).
//!
//! The daemon keeps a [`Publisher`] per attached client and hands the
//! matching [`Subscription`] to [`crate::serve_connection`] in its
//! [`crate::Service::attach_session`] answer. The connection forwards the
//! subscription's events as [`crate::methods::SESSION_EVENT`] notifications.

use std::{
    fmt,
    sync::{Arc, OnceLock},
};

use tokio::sync::mpsc::{
    self,
    error::{TryRecvError, TrySendError},
};

use crate::methods::{DetachReason, Seq, SessionEvent};

/// Opens a subscription that first sends `replay`, the events a client
/// attaching or reattaching has not seen, then holds up to `backlog` live
/// events its connection has not yet taken. The replay is taken as the
/// connection sends it, so a long one, such as a whole transcript, can be
/// produced an event at a time rather than held in memory at once. It does
/// not count against the backlog, so a long one never drops the client. A
/// subscriber further than `backlog` live events behind is dropped with
/// [`DetachReason::Lagged`] rather than buffered without end.
///
/// # Panics
///
/// When `backlog` is 0.
pub fn subscription<R>(backlog: usize, replay: R) -> (Publisher, Subscription)
where
    R: IntoIterator<Item = (Seq, SessionEvent)>,
    R::IntoIter: Send + Sync + 'static,
{
    let (events, receiver) = mpsc::channel(backlog);
    let reason = Arc::new(OnceLock::new());
    let publisher = Publisher {
        events,
        reason: Arc::clone(&reason),
    };
    let subscription = Subscription {
        replay: Box::new(replay.into_iter()),
        events: receiver,
        reason,
    };
    (publisher, subscription)
}

/// The daemon's end of one subscription: publishes one session's live events
/// to one attached client. Publish, and replay through [`subscription`],
/// only events that encode as JSON; one with a non-UTF-8 path does not, and
/// its connection would fail on it.
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
    /// reports.
    pub async fn closed(&self) {
        self.events.closed().await;
    }

    /// Whether the subscription is gone, as [`Published::Gone`] reports, so
    /// a publisher whose client left can be dropped before the next event.
    pub fn is_gone(&self) -> bool {
        self.events.is_closed()
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
pub struct Subscription {
    /// Sent before any live event.
    replay: Box<dyn Iterator<Item = (Seq, SessionEvent)> + Send + Sync>,
    events: mpsc::Receiver<(Seq, SessionEvent)>,
    reason: Arc<OnceLock<DetachReason>>,
}

impl fmt::Debug for Subscription {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Subscription")
            .field("events", &self.events)
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

impl Subscription {
    /// The next event, replayed ones first, or `None` once the publisher is
    /// gone and every published event was taken.
    pub(crate) async fn next(&mut self) -> Option<(Seq, SessionEvent)> {
        match self.replay.next() {
            Some(event) => Some(event),
            None => self.events.recv().await,
        }
    }

    /// The next event already published, without waiting.
    pub(crate) fn try_next(&mut self) -> Next {
        if let Some((seq, event)) = self.replay.next() {
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
