//! A connection's forwarders: one task per attached session that writes its
//! subscription's events as notifications (#1657).
//!
//! Ordering rests on one rule: a forwarder queues a notification only while
//! it is still the session's current forwarder, checked under the same lock
//! that answers attaches and detaches. So every event a forwarder queued
//! precedes the response that replaced or stopped it, and none follows it;
//! an attach's response precedes its first event.
//!
//! A detach or a new attach stops a forwarder at once. The connection's end
//! instead lets each forwarder flush: it sends what its publisher has
//! already published, and the detach reason of a publisher that already
//! closed, without waiting for more. So a service that closes its
//! publishers before a connection ends, as the daemon does when it stops,
//! reaches every attached client.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use tokio::{
    sync::{watch, Semaphore},
    task::{AbortHandle, JoinSet},
};

use crate::{
    jsonrpc,
    methods::{
        Notification, SessionDetachedNotification, SessionDetachedParams, SessionEventNotification,
        SessionEventParams, SessionId,
    },
    server::{Outbox, Outgoing},
    subscription::{Next, Subscription},
};

/// Notifications a connection may have queued for its writer but not yet
/// written, across all its forwarders.
///
/// Any capacity is correct: a forwarder that finds the queue full waits,
/// its subscription fills, and the publisher reports the lag. Each
/// subscription's backlog, not this, bounds how far a client falls behind.
/// 256 events (about 26 KiB at the 104 B per streamed delta measured for
/// #1657) lets a burst from several sessions queue without parking their
/// forwarders.
pub(crate) const NOTIFICATION_QUEUE: usize = 256;

/// One connection's forwarders.
pub(crate) struct Forwarders {
    /// [`NOTIFICATION_QUEUE`] permits; each queued notification holds one
    /// until it is written.
    queue: Arc<Semaphore>,
    /// Set once the connection ends; forwarders then flush and finish.
    ending: watch::Sender<bool>,
    table: Mutex<Table>,
}

struct Table {
    /// False once the connection ended: a late attach starts no forwarder.
    open: bool,
    /// The id the next forwarder gets, so a replaced forwarder can tell it
    /// is no longer current.
    next_id: u64,
    current: HashMap<SessionId, Current>,
    tasks: JoinSet<()>,
}

/// A session's current forwarder.
struct Current {
    id: u64,
    task: AbortHandle,
}

impl Forwarders {
    pub(crate) fn new() -> Self {
        Self {
            queue: Arc::new(Semaphore::new(NOTIFICATION_QUEUE)),
            ending: watch::Sender::new(false),
            table: Mutex::new(Table {
                open: true,
                next_id: 0,
                current: HashMap::new(),
                tasks: JoinSet::new(),
            }),
        }
    }

    /// Answers an attach with `answer`, then forwards `subscription`,
    /// replacing the session's earlier forwarder. Once the connection has
    /// ended, it only answers and drops the subscription.
    ///
    /// # Panics
    ///
    /// Outside a Tokio runtime.
    pub(crate) fn attach(
        self: &Arc<Self>,
        session: SessionId,
        subscription: Subscription,
        outbox: Outbox,
        answer: impl FnOnce(),
    ) {
        let mut table = self.lock();
        answer();
        if !table.open {
            return;
        }
        while table.tasks.try_join_next().is_some() {}
        let id = table.next_id;
        table.next_id += 1;
        let task = table.tasks.spawn(forward(
            Arc::clone(self),
            outbox,
            session.clone(),
            id,
            subscription,
            self.ending.subscribe(),
        ));
        if let Some(replaced) = table.current.insert(session, Current { id, task }) {
            replaced.task.abort();
        }
    }

    /// Stops the session's forwarder, if it has one. Nothing it has not
    /// queued yet is sent.
    pub(crate) fn detach(&self, session: &SessionId) {
        if let Some(current) = self.lock().current.remove(session) {
            current.task.abort();
        }
    }

    /// Ends every forwarder as the connection ends: each sends what its
    /// publisher already published, and the reason of a publisher already
    /// closed, then drops its subscription, so its publisher sees
    /// [`crate::Published::Gone`] and none keeps the writer open. Waits
    /// until all have.
    pub(crate) async fn end_all(&self) {
        let mut tasks = {
            let mut table = self.lock();
            table.open = false;
            std::mem::take(&mut table.tasks)
        };
        self.ending.send_replace(true);
        while tasks.join_next().await.is_some() {}
    }

    /// Queues `message` while forwarder `id` is the session's current one;
    /// false once it is not, or the writer is gone.
    async fn send(
        &self,
        session: &SessionId,
        id: u64,
        message: jsonrpc::Notification,
        outbox: &Outbox,
    ) -> bool {
        // Never closed, so acquiring fails only if that changes.
        let Ok(permit) = Arc::clone(&self.queue).acquire_owned().await else {
            return false;
        };
        let table = self.lock();
        table.is_current(session, id)
            && outbox.send(Outgoing::Notification(message, permit)).is_ok()
    }

    /// Forgets forwarder `id` once it has finished, unless it was replaced.
    fn finish(&self, session: &SessionId, id: u64) {
        let mut table = self.lock();
        if table.is_current(session, id) {
            table.current.remove(session);
        }
    }

    /// Nothing panics while holding the lock, so a poisoned one is still
    /// consistent.
    fn lock(&self) -> MutexGuard<'_, Table> {
        self.table.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Table {
    fn is_current(&self, session: &SessionId, id: u64) -> bool {
        self.current
            .get(session)
            .is_some_and(|current| current.id == id)
    }
}

/// Forwards one subscription's events, then its detach reason, while it is
/// the session's current forwarder. Once `ending` is set, it forwards only
/// what is already published, then finishes.
///
/// An event JSON cannot hold, such as one with a non-UTF-8 path, is skipped;
/// the client sees the gap in `seq`.
async fn forward(
    forwarders: Arc<Forwarders>,
    outbox: Outbox,
    session: SessionId,
    id: u64,
    mut subscription: Subscription,
    mut ending: watch::Receiver<bool>,
) {
    let send_event = async |seq, event| {
        let params = SessionEventParams {
            session: session.clone(),
            seq,
            event,
        };
        match notification::<SessionEventNotification>(&params) {
            Ok(message) => forwarders.send(&session, id, message, &outbox).await,
            Err(_) => true,
        }
    };
    let published_all = loop {
        let next = tokio::select! {
            next = subscription.next() => next,
            // Never fails: the sender lives as long as `forwarders`.
            _ = ending.wait_for(|ending| *ending) => break false,
        };
        let Some((seq, event)) = next else {
            break true;
        };
        if !send_event(seq, event).await {
            return;
        }
    };
    if !published_all {
        loop {
            match subscription.try_next() {
                Next::Event(seq, event) => {
                    if !send_event(seq, event).await {
                        return;
                    }
                }
                // The publisher may still publish; it sees the subscription
                // gone once this returns.
                Next::Empty => return,
                Next::Ended => break,
            }
        }
    }
    if let Some(reason) = subscription.reason() {
        let params = SessionDetachedParams {
            session: session.clone(),
            reason,
        };
        if let Ok(message) = notification::<SessionDetachedNotification>(&params) {
            forwarders.send(&session, id, message, &outbox).await;
        }
    }
    forwarders.finish(&session, id);
}

/// Encodes `N` with its params, the type [`crate::Client`] decodes.
fn notification<N: Notification>(
    params: &N::Params,
) -> Result<jsonrpc::Notification, serde_json::Error> {
    Ok(jsonrpc::Notification {
        method: N::NAME.to_owned(),
        params: Some(serde_json::to_value(params)?),
    })
}
