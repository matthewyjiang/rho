//! Owns the live parent steering port and correlates application receipts.

use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};

use futures_util::{
    future::{BoxFuture, Shared},
    FutureExt,
};

use super::subagent_messaging::{parent_message_prompt, ValidatedMessage};

type Receipt = Shared<BoxFuture<'static, Result<rho_sdk::SteeringId, String>>>;

struct PendingMessage {
    receipt: Receipt,
    message: ValidatedMessage,
}

#[derive(Default)]
struct SteeringState {
    handle: Option<rho_sdk::SteeringHandle>,
    pending: Vec<Arc<PendingMessage>>,
}

/// Keeps the live window and its receipts under one lock so clear cannot race
/// with a sender registering a message against an obsolete handle.
#[derive(Clone, Default)]
pub(crate) struct SteeringSlot {
    state: Arc<Mutex<SteeringState>>,
}

impl std::fmt::Debug for SteeringSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SteeringSlot").finish_non_exhaustive()
    }
}

impl SteeringSlot {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn publish(&self, handle: rho_sdk::SteeringHandle) {
        self.state.lock().expect("parent steering state").handle = Some(handle);
    }

    pub(crate) async fn send(&self, message: &ValidatedMessage) -> anyhow::Result<()> {
        let input = rho_sdk::UserInput::text(parent_message_prompt(message));
        let pending = {
            let mut state = self.state.lock().expect("parent steering state");
            let Some(handle) = &state.handle else {
                anyhow::bail!("delegated run is not accepting messages; wait until status is running, then message again");
            };
            // Queue and register without yielding or releasing the slot lock.
            // A full/closed SDK command queue fails without retaining a body.
            let receipt = handle.request_steer_retractable(input)?;
            let pending = Arc::new(PendingMessage {
                receipt: receipt
                    .map(|result| result.map_err(|error| error.to_string()))
                    .boxed()
                    .shared(),
                message: message.clone(),
            });
            state.pending.push(Arc::clone(&pending));
            pending
        };
        let result = self.wait_receipt(pending.receipt.clone()).await;
        if result.is_err() {
            self.state
                .lock()
                .expect("parent steering state")
                .pending
                .retain(|message| !Arc::ptr_eq(message, &pending));
        }
        result.map(drop).map_err(anyhow::Error::msg)
    }

    async fn wait_receipt(&self, mut receipt: Receipt) -> Result<rho_sdk::SteeringId, String> {
        // Shared can report Pending while another clone is mid-poll, even after
        // SDK acceptance. Serialize every poll with applied so its single poll
        // cannot miss the application event. Never hold the guard across await.
        // The inner SDK oneshot only registers/wakes tasks; our Tokio task wakers
        // schedule work rather than reenter this lock synchronously. Keep Shared
        // to fan out wakes to the sender even when applied polls the receipt.
        futures_util::future::poll_fn(|cx| {
            let _state = self.state.lock().expect("parent steering state");
            Pin::new(&mut receipt).poll(cx)
        })
        .await
    }

    pub(crate) fn applied(&self, ids: &[rho_sdk::SteeringId]) -> Vec<ValidatedMessage> {
        let mut state = self.state.lock().expect("parent steering state");
        let mut messages = std::collections::HashMap::new();
        // The SDK resolves acceptance before emitting SteeringApplied. Poll each
        // shared receipt once, including receipts the sender has not yet polled.
        // Unregistered IDs and later unresolved sends must never stall the pump.
        state
            .pending
            .retain(|pending| match pending.receipt.clone().now_or_never() {
                Some(Ok(id)) if ids.contains(&id) => {
                    messages.insert(id, pending.message.clone());
                    false
                }
                Some(Err(_)) => false,
                Some(Ok(_)) | None => true,
            });
        ids.iter().filter_map(|id| messages.remove(id)).collect()
    }

    pub(crate) fn clear(&self) {
        let mut state = self.state.lock().expect("parent steering state");
        state.handle = None;
        state.pending.clear();
    }
}

#[cfg(test)]
#[path = "parent_steering_tests.rs"]
mod tests;
