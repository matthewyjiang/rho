//! ACP v2 prompts that arrive while the session is busy.
//!
//! v2 never rejects a prompt because foreground work is active: it holds
//! the prompt, with its response pending, and inserts it once the session
//! is free. Rho inserts held prompts one at a time, in the order they
//! arrived, each as the session's next turn.
//!
//! A prompt takes its [`Place`] in its session's line as the connection
//! reads it, before any later message is handled, so its order against the
//! session's other prompts and its cancels is the host's. A `session/cancel`
//! or a close drops every prompt still held when it arrives
//! ([`Line::cancels`]), and `$/cancel_request` drops the one it names; each
//! answers `-32800` and the prompt is never inserted. A prompt the daemon
//! already took is no longer held: a cancel then cancels its turn, as it
//! cancels any turn, including a cancel read while the prompt was on its
//! way to the daemon.
//!
//! A prompt that names an idempotency key an earlier prompt of the
//! session named holds no second place: see [`super::retry`].

use std::{
    collections::VecDeque,
    future::Future,
    sync::{Arc, Mutex, PoisonError},
};

use agent_client_protocol::{v2::schema as v2, Error as AcpError, RequestCancellation};
use rho_sdk::CancellationToken;
use tokio::sync::{watch, MutexGuard};

use super::retry::{self, Keys, Retries, Retry};
use crate::app::acp::sessions::{Busy, Live};

/// A session's held prompts, oldest first, its cancels, and the keys its
/// prompts named.
pub(in crate::app::acp) struct Holds {
    line: watch::Sender<Line>,
    keys: Mutex<Keys>,
}

impl Default for Holds {
    fn default() -> Self {
        Self {
            line: watch::channel(Line::default()).0,
            keys: Mutex::default(),
        }
    }
}

impl Holds {
    /// The session was cancelled or closed: every prompt held now is
    /// dropped.
    pub(in crate::app::acp) fn cancel(&self) {
        self.line.send_modify(|line| line.cancels += 1);
    }

    pub(super) fn keys(&self) -> std::sync::MutexGuard<'_, Keys> {
        self.keys.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[derive(Default)]
struct Line {
    /// The tickets of the prompts waiting for their turn, oldest first.
    waiting: VecDeque<u64>,
    next: u64,
    /// How many cancels and closes arrived. A prompt that arrived before
    /// the latest one is dropped.
    cancels: u64,
}

/// One held prompt's place in its session's line. Leaves the line once it
/// takes its turn, or when dropped.
pub(in crate::app::acp) struct Place {
    live: Arc<Live>,
    ticket: u64,
    /// [`Line::cancels`] when the prompt arrived.
    cancels: u64,
    /// The prompt request's `$/cancel_request`.
    cancellation: RequestCancellation,
    /// Tripped once a retry's `$/cancel_request` withdrew the prompt; never
    /// for a prompt that named no key.
    withdrawn: CancellationToken,
    /// The prompt's idempotency key.
    key: Option<v2::IdempotencyKey>,
}

/// How a prompt that names an idempotency key enters its session.
pub(in crate::app::acp) enum Entry {
    /// It takes its place in line; `retries` tells the prompt's retries
    /// how it ended.
    Held {
        place: Place,
        retries: Option<Retries>,
    },
    /// An earlier prompt of the session's named its key: it answers as that
    /// one does.
    Retry(Retry),
}

impl Place {
    /// Puts a prompt that just arrived at the end of `live`'s line.
    pub(in crate::app::acp) fn new(live: Arc<Live>, cancellation: RequestCancellation) -> Self {
        let (mut ticket, mut cancels) = (0, 0);
        live.held.line.send_modify(|line| {
            ticket = line.next;
            line.next += 1;
            line.waiting.push_back(ticket);
            cancels = line.cancels;
        });
        Self {
            live,
            ticket,
            cancels,
            cancellation,
            withdrawn: CancellationToken::new(),
            key: None,
        }
    }

    /// Names the prompt's idempotency key, as the connection reads it,
    /// before it reads on. A prompt whose key an earlier prompt of the
    /// session named leaves the line and answers as that one does, unless a
    /// cancel dropped that one since.
    pub(in crate::app::acp) fn keyed(mut self, key: Option<v2::IdempotencyKey>) -> Entry {
        let Some(key) = key else {
            return Entry::Held {
                place: self,
                retries: None,
            };
        };
        let claimed = retry::claim(
            &mut self.live.held.keys(),
            &key,
            self.cancels,
            &self.withdrawn,
        );
        match claimed {
            Ok(prompt) => {
                let retries = Retries::new(Arc::clone(&self.live), key.clone(), prompt);
                self.key = Some(key);
                Entry::Held {
                    place: self,
                    retries: Some(retries),
                }
            }
            Err(followed) => Entry::Retry(Retry {
                followed,
                cancellation: self.cancellation.clone(),
            }),
        }
    }

    /// The idempotency key the prompt named, which the daemon knows it by.
    pub(in crate::app::acp) fn key(&self) -> Option<&v2::IdempotencyKey> {
        self.key.as_ref()
    }

    pub(in crate::app::acp) fn live(&self) -> &Arc<Live> {
        &self.live
    }

    /// Waits until every prompt that arrived before this one has had its
    /// turn and the session's prompt slot is free, then takes the slot.
    pub(in crate::app::acp) async fn take_turn(&self) -> Result<MutexGuard<'_, ()>, AcpError> {
        let first_free = async {
            let mut line = self.live.held.line.subscribe();
            // The sender lives as long as `self.live`. A cancel ends the
            // wait too, and `unless_dropped` answers it.
            let _ = line
                .wait_for(|line| self.dropped(line) || line.waiting.front() == Some(&self.ticket))
                .await;
            self.live.prompt.lock().await
        };
        let turn = self.unless_dropped(first_free).await?;
        self.leave();
        Ok(turn)
    }

    /// Waits for the turn the daemon refused this prompt for, as `busy`
    /// saw it, to end: another client's, or one the daemon started to
    /// deliver finished background work.
    pub(in crate::app::acp) async fn until_free(&self, busy: Busy) -> Result<(), AcpError> {
        self.unless_dropped(busy.ended()).await
    }

    /// Fails as a dropped prompt fails, once a cancel covered it.
    pub(in crate::app::acp) fn check(&self) -> Result<(), AcpError> {
        if self.session_cancelled() {
            return Err(cancelled());
        }
        if self.cancellation.is_cancelled() || self.withdrawn.is_cancelled() {
            return Err(AcpError::request_cancelled());
        }
        Ok(())
    }

    /// Whether the session was cancelled or closed since the prompt
    /// arrived: one that was taken meanwhile has its turn stopped.
    pub(in crate::app::acp) fn session_cancelled(&self) -> bool {
        self.dropped(&self.live.held.line.borrow())
    }

    /// A cancel or close arrived since the prompt did.
    fn dropped(&self, line: &Line) -> bool {
        line.cancels != self.cancels
    }

    /// Runs `work` while the prompt is held, failing as soon as a cancel
    /// covers it.
    async fn unless_dropped<T>(&self, work: impl Future<Output = T>) -> Result<T, AcpError> {
        let mut line = self.live.held.line.subscribe();
        let session_cancelled = async {
            let _ = line.wait_for(|line| self.dropped(line)).await;
        };
        tokio::select! {
            biased;
            () = session_cancelled => Err(cancelled()),
            () = self.cancellation.cancelled() => Err(AcpError::request_cancelled()),
            () = self.withdrawn.cancelled() => Err(AcpError::request_cancelled()),
            value = work => Ok(value),
        }
    }

    fn leave(&self) {
        self.live.held.line.send_if_modified(|line| {
            let before = line.waiting.len();
            line.waiting.retain(|ticket| *ticket != self.ticket);
            line.waiting.len() != before
        });
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        self.leave();
    }
}

fn cancelled() -> AcpError {
    AcpError::request_cancelled().data("the session was cancelled before the prompt was inserted")
}
