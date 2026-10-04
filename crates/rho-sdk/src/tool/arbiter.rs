use std::{
    num::NonZeroUsize,
    sync::{Arc, Mutex},
};

use tokio::sync::Notify;

use super::{scheduling::serialization_reason, ToolExecutionPolicy};

/// Admission-order dependencies for prepared, authorized calls in one host.
/// Waiting calls stay in the queue so an exclusive call is a barrier even
/// before it starts. The same conflict rules also drive the model batch planner.
///
/// An optional limit bounds calls executing at once. Calls clear of conflicts
/// take free slots in admission order, so a later call cannot pass an
/// earlier one that is only waiting for a slot.
#[derive(Default)]
pub(crate) struct ExecutionArbiter {
    limit: Option<NonZeroUsize>,
    queue: Mutex<Vec<Entry>>,
    changed: Notify,
}

struct Entry {
    key: Arc<()>,
    policy: ToolExecutionPolicy,
    executing: bool,
}

/// Removes both waiting and executing entries on drop, including cancellation.
pub(crate) struct ExecutionPermit {
    arbiter: Arc<ExecutionArbiter>,
    key: Arc<()>,
}

impl ExecutionArbiter {
    /// An arbiter that executes at most `limit` calls at once.
    pub(crate) fn with_limit(limit: NonZeroUsize) -> Self {
        Self {
            limit: Some(limit),
            ..Self::default()
        }
    }

    pub(crate) async fn acquire(self: &Arc<Self>, policy: &ToolExecutionPolicy) -> ExecutionPermit {
        let key = Arc::new(());
        self.queue
            .lock()
            .expect("tool execution queue lock")
            .push(Entry {
                key: Arc::clone(&key),
                policy: policy.clone(),
                executing: false,
            });
        let permit = ExecutionPermit {
            arbiter: Arc::clone(self),
            key,
        };
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.admit(&permit.key) {
                return permit;
            }
            changed.await;
        }
    }

    /// Marks the entry for `key` executing when it may start: no earlier
    /// entry conflicts with it, and a slot is free for it after the earlier
    /// clear entries still waiting for one.
    fn admit(&self, key: &Arc<()>) -> bool {
        let mut queue = self.queue.lock().expect("tool execution queue lock");
        let mut slots = self
            .limit
            .map(|limit| limit.get() - queue.iter().filter(|entry| entry.executing).count());
        for index in 0..queue.len() {
            let clear = queue[..index].iter().all(|earlier| {
                serialization_reason(&earlier.policy, &queue[index].policy).is_none()
            });
            let entry = &mut queue[index];
            let waiting_for_slot = clear && !entry.executing;
            let slot_free = slots.is_none_or(|slots| slots > 0);
            if Arc::ptr_eq(&entry.key, key) {
                entry.executing = waiting_for_slot && slot_free;
                return entry.executing;
            }
            if waiting_for_slot {
                slots = slots.map(|slots| slots.saturating_sub(1));
            }
        }
        unreachable!("an acquiring entry stays queued until its permit drops")
    }
}

impl Drop for ExecutionPermit {
    fn drop(&mut self) {
        self.arbiter
            .queue
            .lock()
            .expect("tool execution queue lock")
            .retain(|entry| !Arc::ptr_eq(&entry.key, &self.key));
        self.arbiter.changed.notify_waiters();
    }
}

#[cfg(test)]
#[path = "arbiter_tests.rs"]
mod tests;
