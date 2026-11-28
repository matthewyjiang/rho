//! ACP v2 prompts sent again with an idempotency key.
//!
//! A prompt that names an idempotency key some earlier prompt of this
//! connection's session named holds no second place ([`claim`]): while that
//! prompt is held or on its way to the daemon, the retry answers when it
//! does, with the same outcome, and once it was inserted, with its message's
//! ID at once. A `$/cancel_request` for the retry withdraws the held prompt as
//! one for the prompt does. A key this connection never saw goes to the
//! daemon, which knows the keys of the session's saved messages.

use std::{collections::HashMap, sync::Arc};

use agent_client_protocol::{v2::schema as v2, Error as AcpError, RequestCancellation};
use rho_sdk::CancellationToken;
use tokio::sync::watch;

use crate::app::acp::sessions::Live;

/// What this connection's prompts that named a key came to, by key.
pub(super) type Keys = HashMap<v2::IdempotencyKey, Keyed>;

/// What the prompts that named one key came to.
#[derive(Clone)]
pub(super) enum Keyed {
    /// A prompt naming it waits to be inserted.
    Pending(Arc<KeyedPrompt>),
    /// A prompt naming it was inserted as this message.
    Inserted(v2::MessageId),
}

/// How a prompt that names a key ends, or its retries.
type Outcome = Result<v2::MessageId, AcpError>;

/// A held prompt that named a key, as its retries follow it.
pub(super) struct KeyedPrompt {
    /// Told the prompt's outcome once its answer is sent.
    outcome: watch::Sender<Option<Outcome>>,
    /// The prompt's own, which a retry's `$/cancel_request` trips to
    /// withdraw it while it is held.
    withdrawn: CancellationToken,
    /// How many cancels the session had when the prompt arrived: a cancel
    /// since dropped it.
    cancels: u64,
}

/// Claims `key` for a prompt that arrived after the session's `cancels`
/// cancels, which `withdrawn` withdraws, unless an earlier prompt that no
/// cancel dropped named it: then that prompt's `Err` says what it came to.
pub(super) fn claim(
    keys: &mut Keys,
    key: &v2::IdempotencyKey,
    cancels: u64,
    withdrawn: &CancellationToken,
) -> Result<Arc<KeyedPrompt>, Keyed> {
    match keys.get(key) {
        Some(earlier @ Keyed::Inserted(_)) => return Err(earlier.clone()),
        Some(earlier @ Keyed::Pending(prompt)) if prompt.cancels == cancels => {
            return Err(earlier.clone());
        }
        // Dropped by a cancel, or never seen: this prompt is the key's.
        Some(Keyed::Pending(_)) | None => {}
    }
    let prompt = Arc::new(KeyedPrompt {
        outcome: watch::channel(None).0,
        withdrawn: withdrawn.clone(),
        cancels,
    });
    keys.insert(key.clone(), Keyed::Pending(Arc::clone(&prompt)));
    Ok(prompt)
}

/// A prompt whose key an earlier prompt of the session named, which holds no
/// place of its own.
pub(in crate::app::acp) struct Retry {
    pub(super) followed: Keyed,
    /// The retry request's `$/cancel_request`.
    pub(super) cancellation: RequestCancellation,
}

impl Retry {
    /// The earlier prompt's outcome, once it has one. A `$/cancel_request`
    /// for the retry withdraws that prompt while it is held, and the retry
    /// then answers as it does: `-32800`, or its message's ID when it was
    /// inserted first.
    pub(in crate::app::acp) async fn outcome(self) -> Outcome {
        let prompt = match self.followed {
            Keyed::Inserted(message) => return Ok(message),
            Keyed::Pending(prompt) => prompt,
        };
        let mut outcome = prompt.outcome.subscribe();
        let told = async {
            match outcome.wait_for(Option::is_some).await {
                Ok(told) => told.clone().unwrap_or_else(|| Err(lost())),
                // `Retries` tells before it drops the sender.
                Err(_) => Err(lost()),
            }
        };
        tokio::pin!(told);
        tokio::select! {
            biased;
            outcome = &mut told => return outcome,
            () = self.cancellation.cancelled() => prompt.withdrawn.cancel(),
        }
        told.await
    }
}

/// Tells a held prompt's retries how it ended, and the session what its key
/// came to; told once the prompt is answered. One dropped untold fails
/// them.
pub(in crate::app::acp) struct Retries {
    live: Arc<Live>,
    key: v2::IdempotencyKey,
    prompt: Arc<KeyedPrompt>,
    told: bool,
}

impl Retries {
    /// Tells the retries of `prompt`, the one of `live`'s session that
    /// claimed `key`.
    pub(super) fn new(live: Arc<Live>, key: v2::IdempotencyKey, prompt: Arc<KeyedPrompt>) -> Self {
        Self {
            live,
            key,
            prompt,
            told: false,
        }
    }

    /// The prompt was answered with `outcome`: a prompt inserted keeps its
    /// key for the rest of the session's time on this connection, and one
    /// that failed leaves it unused.
    pub(in crate::app::acp) fn tell(mut self, outcome: Outcome) {
        self.told = true;
        self.settle(outcome);
    }

    fn settle(&self, outcome: Outcome) {
        {
            let mut keys = self.live.held.keys();
            // A prompt that arrived after a cancel dropped this one may own
            // the key by now.
            let owned = matches!(
                keys.get(&self.key),
                Some(Keyed::Pending(prompt)) if Arc::ptr_eq(prompt, &self.prompt)
            );
            if owned {
                match &outcome {
                    Ok(message) => {
                        keys.insert(self.key.clone(), Keyed::Inserted(message.clone()));
                    }
                    Err(_) => {
                        keys.remove(&self.key);
                    }
                }
            }
        }
        self.prompt.outcome.send_replace(Some(outcome));
    }
}

impl Drop for Retries {
    fn drop(&mut self) {
        if !self.told {
            self.settle(Err(lost()));
        }
    }
}

/// The outcome of a prompt that stopped without an answer.
fn lost() -> AcpError {
    AcpError::internal_error().data("the prompt this retries stopped without an answer")
}
