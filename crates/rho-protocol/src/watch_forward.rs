//! A connection's watch forwarders: one per state the connection read and
//! so subscribed to, which writes a notification whenever that state
//! changes. A settings namespace read with `rho/config/get` sends
//! `rho/config/changed`; the models read with `rho/model/list` send
//! `rho/model/changed`.
//!
//! The daemon publishes each state whole into a watch. A forwarder compares
//! each state it sees with the last one it sent and sends what differs, so
//! coalescing two quick writes never loses a change, and there is no
//! backlog to size: whatever changed since the last notification is in the
//! next one.
//!
//! Ordering follows the session forwarders' rule (see the parent module): a
//! forwarder queues a notification only while it is its state's current
//! one, so the response to a repeated read follows every notification of
//! the forwarder it replaced.

use std::{collections::BTreeMap, sync::Arc};

use tokio::{sync::watch, task::AbortHandle};

use super::{notification, Forwarders};
use crate::{
    jsonrpc,
    methods::{
        ConfigChangedNotification, ConfigChangedParams, ConfigGetResponse, ConfigNamespace,
        ModelChangedNotification, ModelList,
    },
    server::{Outbox, Outgoing},
};

/// The daemon's state of one settings namespace, published whole each time
/// it changes. [`crate::SettingsService::get_config`] returns one; the
/// connection answers with its current state, then sends each later state's
/// changed entries as a `rho/config/changed` notification.
pub struct ConfigWatch(watch::Receiver<Arc<ConfigGetResponse>>);

impl ConfigWatch {
    pub fn new(receiver: watch::Receiver<Arc<ConfigGetResponse>>) -> Self {
        Self(receiver)
    }
}

/// The models one client may pick, published whole each time they change.
/// [`crate::SettingsService::list_models`] returns one; the connection
/// answers with its current list, then sends each later list that differs
/// as a `rho/model/changed` notification.
pub struct ModelWatch(watch::Receiver<Arc<ModelList>>);

impl ModelWatch {
    pub fn new(receiver: watch::Receiver<Arc<ModelList>>) -> Self {
        Self(receiver)
    }
}

/// A state a connection watches; reading it again replaces its forwarder.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Watched {
    Config(ConfigNamespace),
    Models,
}

/// A watched state's current forwarder.
pub(super) struct WatchCurrent {
    id: u64,
    task: AbortHandle,
}

/// One forwarder of a watched state: it stays current until a later read
/// of the state replaces it.
struct Slot {
    watched: Watched,
    id: u64,
}

impl Forwarders {
    /// Answers a `rho/config/get` with `watch`'s current state through
    /// `answer`, then forwards its later states' changed entries, replacing
    /// the namespace's earlier forwarder. Once the connection has ended, it
    /// only answers.
    ///
    /// # Panics
    ///
    /// Outside a Tokio runtime.
    pub(crate) fn watch_config(
        self: &Arc<Self>,
        namespace: ConfigNamespace,
        ConfigWatch(receiver): ConfigWatch,
        outbox: Outbox,
        answer: impl FnOnce(&ConfigGetResponse),
    ) {
        let watched = Watched::Config(namespace.clone());
        self.watch(watched, receiver, outbox, answer, move |sent, next| {
            config_changes(&namespace, sent, next)
        });
    }

    /// Answers a `rho/model/list` with `watch`'s current list through
    /// `answer`, then forwards each later list that differs, replacing the
    /// connection's earlier model forwarder. Once the connection has ended,
    /// it only answers.
    ///
    /// # Panics
    ///
    /// Outside a Tokio runtime.
    pub(crate) fn watch_models(
        self: &Arc<Self>,
        ModelWatch(receiver): ModelWatch,
        outbox: Outbox,
        answer: impl FnOnce(&ModelList),
    ) {
        self.watch(Watched::Models, receiver, outbox, answer, |sent, next| {
            (sent != next).then(|| {
                notification::<ModelChangedNotification>(next)
                    .expect("model notifications hold only JSON data")
            })
        });
    }

    /// Answers with `receiver`'s current state, then forwards what
    /// `changes` makes of each later one, replacing `watched`'s earlier
    /// forwarder.
    fn watch<T: Send + Sync + 'static>(
        self: &Arc<Self>,
        watched: Watched,
        mut receiver: watch::Receiver<Arc<T>>,
        outbox: Outbox,
        answer: impl FnOnce(&T),
        changes: impl Fn(&T, &T) -> Option<jsonrpc::Notification> + Send + 'static,
    ) {
        let mut table = self.lock();
        let sent = Arc::clone(&receiver.borrow_and_update());
        answer(&sent);
        if !table.open {
            return;
        }
        while table.tasks.try_join_next().is_some() {}
        let id = table.next_id;
        table.next_id += 1;
        let slot = Slot {
            watched: watched.clone(),
            id,
        };
        let task = table.tasks.spawn(forward_watched(
            Arc::clone(self),
            outbox,
            slot,
            receiver,
            sent,
            changes,
            self.ending.subscribe(),
        ));
        if let Some(replaced) = table.watched.insert(watched, WatchCurrent { id, task }) {
            replaced.task.abort();
        }
    }

    /// Queues `message` while `slot` is its state's current forwarder;
    /// false once it is not, or the writer is gone.
    async fn send_watched(
        &self,
        slot: &Slot,
        message: jsonrpc::Notification,
        outbox: &Outbox,
    ) -> bool {
        // Never closed, so acquiring fails only if that changes.
        let Ok(permit) = Arc::clone(&self.queue).acquire_owned().await else {
            return false;
        };
        let table = self.lock();
        table.is_current_watch(slot) && outbox.send(Outgoing::Notification(message, permit)).is_ok()
    }

    /// Forgets `slot` once its forwarder has finished, unless it was
    /// replaced.
    fn finish_watched(&self, slot: &Slot) {
        let mut table = self.lock();
        if table.is_current_watch(slot) {
            table.watched.remove(&slot.watched);
        }
    }
}

impl super::Table {
    fn is_current_watch(&self, slot: &Slot) -> bool {
        self.watched
            .get(&slot.watched)
            .is_some_and(|current| current.id == slot.id)
    }
}

/// Sends what `changes` makes of each state `receiver` publishes after
/// `sent`, until the publisher is gone or the connection ends.
async fn forward_watched<T>(
    forwarders: Arc<Forwarders>,
    outbox: Outbox,
    slot: Slot,
    mut receiver: watch::Receiver<Arc<T>>,
    mut sent: Arc<T>,
    changes: impl Fn(&T, &T) -> Option<jsonrpc::Notification>,
    mut ending: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            changed = receiver.changed() => {
                if changed.is_err() {
                    break;
                }
            }
            // Never fails: the sender lives as long as `forwarders`.
            _ = ending.wait_for(|ending| *ending) => break,
        }
        let next = Arc::clone(&receiver.borrow_and_update());
        if let Some(message) = changes(&sent, &next) {
            if !forwarders.send_watched(&slot, message, &outbox).await {
                return;
            }
        }
        sent = next;
    }
    forwarders.finish_watched(&slot);
}

/// `next` as a `rho/config/changed` for `namespace`: the entries `sent` did
/// not have, or had otherwise, and the whole error list. `None` when
/// neither changed.
fn config_changes(
    namespace: &ConfigNamespace,
    sent: &ConfigGetResponse,
    next: &ConfigGetResponse,
) -> Option<jsonrpc::Notification> {
    let values: BTreeMap<_, _> = next
        .values
        .iter()
        .filter(|(key, entry)| sent.values.get(*key) != Some(*entry))
        .map(|(key, entry)| (key.clone(), entry.clone()))
        .collect();
    if values.is_empty() && next.errors == sent.errors {
        return None;
    }
    let params = ConfigChangedParams {
        namespace: namespace.clone(),
        revision: next.revision.clone(),
        values,
        errors: next.errors.clone(),
    };
    Some(
        notification::<ConfigChangedNotification>(&params)
            .expect("config notifications hold only JSON data"),
    )
}
