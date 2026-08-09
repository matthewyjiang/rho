//! Ties a server-initiated request back to the tool call that provoked it.
//!
//! `elicitation/create` and `sampling/createMessage` arrive on the session
//! transport with nothing in them that names the `tools/call` they belong to:
//! the protocol carries no correlation field for either. Both nevertheless need
//! a caller, because the only route to the user runs through a live tool call,
//! and because the caller's cancellation token is what ends the work when the
//! call ends.
//!
//! Rho records every in-flight `tools/call` for a session and answers a
//! server-initiated request only when exactly one call is running **and** no
//! other call has ended recently enough that a delayed request from it could
//! still be in flight. Zero calls means the request belongs to no user-visible
//! work; more than one means Rho would have to guess; a recent release means a
//! late message from the finished call could be mis-attributed to the next one.
//! All three fail closed.
//!
//! Each registration owns a call-scoped cancellation token that is cancelled
//! when the call ends for any reason (success, error, turn cancel, or budget),
//! so nested sampling stops with the call rather than only with the turn.
//!
//! The registry is per session, so two servers calling tools at the same time
//! never make each other ambiguous.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use rho_sdk::{CancellationToken, Error, HostInputRequest, HostInputResponse};
use tokio::sync::{mpsc, oneshot};

/// How long a finished call can still poison attribution for a later sole call.
///
/// MCP does not correlate nested requests with the `tools/call` that caused
/// them. Local transports reorder by milliseconds; a short grace is enough to
/// refuse the A-then-B misroute without blocking ordinary multi-call sessions
/// that wait between tools.
const NESTED_ATTRIBUTION_GRACE: Duration = Duration::from_secs(5);

/// One question the session's request router wants put to the user.
///
/// The caller's executor owns the only handle that can reach a person, so the
/// question travels to it and the answer travels back.
pub(crate) struct McpUserQuestion {
    pub(crate) request: HostInputRequest,
    pub(crate) reply: oneshot::Sender<Result<HostInputResponse, Error>>,
}

/// Only ever one question outstanding per call: the caller answers each before
/// reading the next, and a server that pipelines requests should still queue.
const QUESTION_QUEUE_CAPACITY: usize = 4;

/// What a server-initiated request may use from the call it was routed to.
#[derive(Clone, Debug)]
pub(crate) struct McpCaller {
    questions: mpsc::Sender<McpUserQuestion>,
    cancellation: CancellationToken,
}

impl McpCaller {
    /// Put a question to the user through the owning tool call.
    pub(crate) async fn ask(&self, request: HostInputRequest) -> Result<HostInputResponse, Error> {
        let (reply, answer) = oneshot::channel();
        self.questions
            .send(McpUserQuestion { request, reply })
            .await
            .map_err(|_| Error::Interrupted {
                message: "the MCP tool call stopped accepting questions".into(),
            })?;
        answer.await.map_err(|_| Error::Interrupted {
            message: "the MCP tool call ended before the question was answered".into(),
        })?
    }

    pub(crate) fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }
}

/// The `tools/call` invocations currently running against one MCP session.
#[derive(Clone, Debug, Default)]
pub(crate) struct McpInFlightCalls {
    state: Arc<Mutex<State>>,
}

#[derive(Debug, Default)]
struct State {
    /// Monotonic key. Two concurrent calls of the same tool are otherwise
    /// indistinguishable, so registration mints its own identity.
    next_key: u64,
    callers: BTreeMap<u64, McpCaller>,
    /// When the most recent call left the map, if any. Used to fail closed
    /// across call transitions while a delayed nested request might still
    /// arrive.
    last_release_at: Option<Instant>,
}

/// Why a server-initiated request could not be tied to exactly one tool call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum McpRouteError {
    /// Nothing was running, so there is no caller to answer for.
    NoCallInFlight,
    /// Several calls were running and the protocol says nothing about which one
    /// asked.
    AmbiguousCall { in_flight: usize },
    /// A prior call ended recently enough that a delayed nested request from it
    /// could still arrive, so routing to the current sole caller is unsafe.
    AttributionUncertain,
}

impl McpRouteError {
    /// Secret-free explanation Rho sends back to the server.
    pub(crate) fn reason(self) -> String {
        match self {
            Self::NoCallInFlight => {
                "Rho has no MCP tool call in flight to attribute this request to".into()
            }
            Self::AmbiguousCall { in_flight } => format!(
                "Rho has {in_flight} MCP tool calls in flight on this server and cannot tell which one this request belongs to"
            ),
            Self::AttributionUncertain => {
                "Rho cannot safely attribute this request: a previous MCP tool call on this server ended recently and the protocol does not identify which call nested requests belong to".into()
            }
        }
    }
}

impl McpInFlightCalls {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Publish one running call. The guard withdraws it when the call ends,
    /// however it ends, cancels the call-scoped token, and the receiver is how
    /// the caller learns of questions.
    ///
    /// The token is owned by the registration, not the turn: sampling and other
    /// nested work must stop when this call ends even if the turn continues.
    pub(crate) fn register(&self) -> (McpCallRegistration, mpsc::Receiver<McpUserQuestion>) {
        let cancellation = CancellationToken::new();
        let (questions, receiver) = mpsc::channel(QUESTION_QUEUE_CAPACITY);
        let mut state = self.lock();
        let key = state.next_key;
        state.next_key += 1;
        state.callers.insert(
            key,
            McpCaller {
                questions,
                cancellation: cancellation.clone(),
            },
        );
        drop(state);
        (
            McpCallRegistration {
                key,
                cancellation,
                calls: self.clone(),
            },
            receiver,
        )
    }

    /// The one running call that can safely own a nested request, or why not.
    pub(crate) fn sole_caller(&self) -> Result<McpCaller, McpRouteError> {
        let state = self.lock();
        let recent_release = state
            .last_release_at
            .is_some_and(|released| released.elapsed() < NESTED_ATTRIBUTION_GRACE);
        let mut running = state.callers.values();
        match (running.next(), running.next()) {
            (Some(_caller), None) if recent_release => {
                // A finished call may still deliver a nested request. Refusing
                // here is the fail-closed answer when the protocol cannot name
                // the owner.
                Err(McpRouteError::AttributionUncertain)
            }
            (Some(caller), None) => Ok(caller.clone()),
            (None, _) => Err(McpRouteError::NoCallInFlight),
            (Some(_), Some(_)) => Err(McpRouteError::AmbiguousCall {
                in_flight: state.callers.len(),
            }),
        }
    }

    fn release(&self, key: u64, cancellation: &CancellationToken) {
        // Stop nested work tied to this call before (or as) it leaves the map.
        cancellation.cancel();
        let mut state = self.lock();
        state.callers.remove(&key);
        state.last_release_at = Some(Instant::now());
    }

    /// Test-only: pretend the last release happened at `at` so grace recovery
    /// can be checked without sleeping.
    #[cfg(test)]
    pub(crate) fn set_last_release_at_for_test(&self, at: Instant) {
        self.lock().last_release_at = Some(at);
    }

    /// A poisoned lock means a panic while the map was borrowed. The map stays
    /// structurally valid, so recover rather than fail an unrelated tool call.
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }
}

/// Withdraws one call's registration when the call ends and cancels its token.
pub(crate) struct McpCallRegistration {
    key: u64,
    cancellation: CancellationToken,
    calls: McpInFlightCalls,
}

impl Drop for McpCallRegistration {
    fn drop(&mut self) {
        self.calls.release(self.key, &self.cancellation);
    }
}

#[cfg(test)]
#[path = "inflight_tests.rs"]
mod tests;
