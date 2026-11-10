//! A connection's config forwarders: one per settings namespace the
//! connection read with `rho/config/get`, which writes `rho/config/changed`
//! whenever that namespace's values or errors change.
//!
//! The daemon publishes each namespace's whole state into a watch. A
//! forwarder compares each state it sees with the last one it sent and
//! sends only the entries that differ, so coalescing two quick writes never
//! loses a key, and there is no backlog to size: whatever changed since the
//! last notification is in the next one.
//!
//! Ordering follows the session forwarders' rule (see the parent module): a
//! forwarder queues a notification only while it is the namespace's current
//! one, so the response to a repeated `rho/config/get` follows every
//! notification of the forwarder it replaced.

use std::{collections::BTreeMap, sync::Arc};

use tokio::{sync::watch, task::AbortHandle};

use super::{notification, Forwarders};
use crate::{
    jsonrpc,
    methods::{ConfigChangedNotification, ConfigChangedParams, ConfigGetResponse, ConfigNamespace},
    server::{Outbox, Outgoing},
};

/// The daemon's state of one settings namespace, published whole each time
/// it changes. [`crate::Service::get_config`] returns one; the connection
/// answers with its current state, then sends each later state's changed
/// entries as a `rho/config/changed` notification.
pub struct ConfigWatch(watch::Receiver<Arc<ConfigGetResponse>>);

impl ConfigWatch {
    pub fn new(receiver: watch::Receiver<Arc<ConfigGetResponse>>) -> Self {
        Self(receiver)
    }
}

/// A namespace's current config forwarder.
pub(super) struct ConfigCurrent {
    id: u64,
    task: AbortHandle,
}

impl Forwarders {
    /// Answers a `rho/config/get` with `watch`'s current state through
    /// `answer`, then forwards its later states, replacing the namespace's
    /// earlier forwarder. Once the connection has ended, it only answers.
    ///
    /// # Panics
    ///
    /// Outside a Tokio runtime.
    pub(crate) fn watch_config(
        self: &Arc<Self>,
        namespace: ConfigNamespace,
        ConfigWatch(mut receiver): ConfigWatch,
        outbox: Outbox,
        answer: impl FnOnce(&ConfigGetResponse),
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
        let task = table.tasks.spawn(forward_config(
            Arc::clone(self),
            outbox,
            namespace.clone(),
            id,
            receiver,
            sent,
            self.ending.subscribe(),
        ));
        if let Some(replaced) = table.config.insert(namespace, ConfigCurrent { id, task }) {
            replaced.task.abort();
        }
    }

    /// Queues `message` while forwarder `id` is the namespace's current
    /// one; false once it is not, or the writer is gone.
    async fn send_config(
        &self,
        namespace: &ConfigNamespace,
        id: u64,
        message: jsonrpc::Notification,
        outbox: &Outbox,
    ) -> bool {
        // Never closed, so acquiring fails only if that changes.
        let Ok(permit) = Arc::clone(&self.queue).acquire_owned().await else {
            return false;
        };
        let table = self.lock();
        table
            .config
            .get(namespace)
            .is_some_and(|current| current.id == id)
            && outbox.send(Outgoing::Notification(message, permit)).is_ok()
    }

    /// Forgets forwarder `id` once it has finished, unless it was replaced.
    fn finish_config(&self, namespace: &ConfigNamespace, id: u64) {
        let mut table = self.lock();
        if table
            .config
            .get(namespace)
            .is_some_and(|current| current.id == id)
        {
            table.config.remove(namespace);
        }
    }
}

/// Sends each state `receiver` publishes after `sent` as the entries that
/// changed since the last one sent, until the publisher is gone or the
/// connection ends.
async fn forward_config(
    forwarders: Arc<Forwarders>,
    outbox: Outbox,
    namespace: ConfigNamespace,
    id: u64,
    mut receiver: watch::Receiver<Arc<ConfigGetResponse>>,
    mut sent: Arc<ConfigGetResponse>,
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
        let values = changed_entries(&sent, &next);
        if !values.is_empty() || next.errors != sent.errors {
            let params = ConfigChangedParams {
                namespace: namespace.clone(),
                revision: next.revision.clone(),
                values,
                errors: next.errors.clone(),
            };
            let message = notification::<ConfigChangedNotification>(&params)
                .expect("config notifications hold only JSON data");
            if !forwarders
                .send_config(&namespace, id, message, &outbox)
                .await
            {
                return;
            }
        }
        sent = next;
    }
    forwarders.finish_config(&namespace, id);
}

/// The entries of `next` that `sent` did not have, or had otherwise.
fn changed_entries(
    sent: &ConfigGetResponse,
    next: &ConfigGetResponse,
) -> BTreeMap<crate::SettingKey, crate::SettingEntry> {
    next.values
        .iter()
        .filter(|(key, entry)| sent.values.get(*key) != Some(*entry))
        .map(|(key, entry)| (key.clone(), entry.clone()))
        .collect()
}
