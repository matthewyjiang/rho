//! Correlates parent message bodies with the SDK's application acknowledgement.

use std::sync::{Arc, Mutex};

use futures_util::{
    future::{BoxFuture, Shared},
    stream::FuturesUnordered,
    FutureExt, StreamExt,
};

use super::subagent_messaging::{parent_message_prompt, ValidatedMessage};

type Receipt = Shared<BoxFuture<'static, Result<rho_sdk::SteeringId, String>>>;

struct PendingMessage {
    receipt: Receipt,
    message: ValidatedMessage,
}

#[derive(Clone, Default)]
pub(crate) struct ParentSteering {
    pending: Arc<Mutex<Vec<Arc<PendingMessage>>>>,
}

impl std::fmt::Debug for ParentSteering {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParentSteering").finish_non_exhaustive()
    }
}

impl ParentSteering {
    pub(crate) async fn send(
        &self,
        handle: rho_sdk::SteeringHandle,
        message: &ValidatedMessage,
    ) -> anyhow::Result<()> {
        let input = rho_sdk::UserInput::text(parent_message_prompt(message));
        let pending = {
            let mut messages = self.pending.lock().expect("parent steering receipts");
            // Queue and register without yielding. The pump can resolve this
            // receipt even if it sees SteeringApplied before the sender wakes.
            // A full/closed SDK command queue fails without retaining a body.
            let receipt = handle.request_steer_retractable(input)?;
            let pending = Arc::new(PendingMessage {
                receipt: receipt
                    .map(|result| result.map_err(|error| error.to_string()))
                    .boxed()
                    .shared(),
                message: message.clone(),
            });
            messages.push(Arc::clone(&pending));
            pending
        };
        let result = pending.receipt.clone().await;
        if result.is_err() {
            self.remove(&pending);
        }
        result.map(drop).map_err(anyhow::Error::msg)
    }

    pub(crate) async fn applied(&self, ids: &[rho_sdk::SteeringId]) -> Vec<ValidatedMessage> {
        let pending = self
            .pending
            .lock()
            .expect("parent steering receipts")
            .clone();
        let mut receipts = pending
            .into_iter()
            .map(|pending| async move { (pending.receipt.clone().await, pending) })
            .collect::<FuturesUnordered<_>>();
        let mut messages = std::collections::HashMap::new();
        while messages.len() < ids.len() {
            let Some((result, pending)) = receipts.next().await else {
                break;
            };
            match result {
                Ok(id) if ids.contains(&id) => {
                    messages.insert(id, pending.message.clone());
                    self.remove(&pending);
                }
                Ok(_) => {}
                Err(_) => self.remove(&pending),
            }
        }
        ids.iter().filter_map(|id| messages.remove(id)).collect()
    }

    fn remove(&self, message: &Arc<PendingMessage>) {
        self.pending
            .lock()
            .expect("parent steering receipts")
            .retain(|pending| !Arc::ptr_eq(pending, message));
    }

    pub(crate) fn clear(&self) {
        self.pending
            .lock()
            .expect("parent steering receipts")
            .clear();
    }
}

#[cfg(test)]
#[path = "parent_steering_tests.rs"]
mod tests;
