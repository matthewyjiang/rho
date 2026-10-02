use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use super::{scheduling::serialization_reason, ToolExecutionPolicy};

/// Admission-order dependencies for prepared, authorized calls in one host.
/// Waiting calls stay in the queue so an exclusive call is a barrier even
/// before it starts. The same conflict rules also drive the model batch planner.
#[derive(Default)]
pub(crate) struct ExecutionArbiter {
    queue: Mutex<Vec<Entry>>,
    changed: Notify,
}

struct Entry {
    key: Arc<()>,
    policy: ToolExecutionPolicy,
}

/// Removes both waiting and executing entries on drop, including cancellation.
pub(crate) struct ExecutionPermit {
    arbiter: Arc<ExecutionArbiter>,
    key: Arc<()>,
}

impl ExecutionArbiter {
    pub(crate) async fn acquire(self: &Arc<Self>, policy: &ToolExecutionPolicy) -> ExecutionPermit {
        let key = Arc::new(());
        self.queue
            .lock()
            .expect("tool execution queue lock")
            .push(Entry {
                key: Arc::clone(&key),
                policy: policy.clone(),
            });
        let permit = ExecutionPermit {
            arbiter: Arc::clone(self),
            key,
        };
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let ready = {
                let queue = self.queue.lock().expect("tool execution queue lock");
                queue
                    .iter()
                    .take_while(|entry| !Arc::ptr_eq(&entry.key, &permit.key))
                    .all(|entry| serialization_reason(&entry.policy, policy).is_none())
            };
            if ready {
                return permit;
            }
            changed.await;
        }
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
