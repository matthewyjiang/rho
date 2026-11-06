//! The host for sessions nobody watches live: `rho run`, Rho subagents, and
//! workflow agent nodes. Each runs one prompt on a
//! [`SessionRuntime`](super::session_runtime::SessionRuntime) and shows it through a [`HeadlessView`] (see `headless_host/view.rs`), which
//! writes the run's events to the JSONL stream, `rho run`'s text output,
//! and a subagent's [`RunReporter`] status file.
//!
//! A headless prompt also ends early: a deadline, SIGINT/SIGTERM, or a
//! parent's cancel stops it. A stop cancels the prompt through the
//! runtime's [`super::session_runtime::SessionHandle`] and lets it finish, so
//! the session saves what it did and tears down before the host reports
//! the stop, which wins over however the prompt itself ended.
//!
//! `automation.rs` runs `rho run` on it; `headless_host/child.rs` runs a
//! delegated child or a workflow node.

use std::future::Future;

use rho_sdk::{CancellationToken, RunOutcome};
use rho_tools::cancellation::RunCancellation;

use super::{
    session_runtime::{SessionHandle, TurnOutcome},
    shutdown_signal::{ShutdownSignal, ShutdownSignals},
};

mod child;
mod reporter;
mod view;

pub(super) use child::{run_child, ChildRun, SessionCheckpoint};
pub(super) use reporter::RunReporter;
pub(super) use view::{HeadlessView, ParentQuestions};

/// What stops a headless prompt before it ends on its own. Signals are
/// installed before the run is observable; see `shutdown_signal.rs`.
pub(super) struct Stops {
    signals: ShutdownSignals,
    deadline: Option<tokio::time::Instant>,
    cancellation: Option<RunCancellation>,
}

/// Why a stop ended a headless prompt.
#[derive(Clone, Copy, Debug)]
pub(super) enum StopCause {
    /// `rho run --timeout` ran out.
    Timeout,
    Interrupted(ShutdownSignal),
    /// The parent cancelled a delegated child or workflow node.
    Cancelled,
}

impl Stops {
    /// Stops on `signals` only.
    pub(super) fn on_signals(signals: ShutdownSignals) -> Self {
        Self {
            signals,
            deadline: None,
            cancellation: None,
        }
    }

    /// Also stops at `deadline`, when there is one.
    pub(super) fn or_at(mut self, deadline: Option<tokio::time::Instant>) -> Self {
        self.deadline = deadline;
        self
    }

    /// Also stops when `cancellation` trips.
    pub(super) fn or_cancelled(mut self, cancellation: RunCancellation) -> Self {
        self.cancellation = Some(cancellation);
        self
    }

    async fn next(&mut self) -> StopCause {
        let Self {
            signals,
            deadline,
            cancellation,
        } = self;
        let deadline = *deadline;
        let timeout = async move {
            match deadline {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => std::future::pending().await,
            }
        };
        let cancelled = async {
            match cancellation {
                Some(cancellation) => cancellation.cancelled().await,
                None => std::future::pending().await,
            }
        };
        // A signal watch that fails stops watching signals, not the others.
        let signal = async {
            match signals.recv().await {
                Ok(signal) => signal,
                Err(error) => {
                    tracing::warn!(%error, "could not watch for shutdown signals");
                    std::future::pending().await
                }
            }
        };
        tokio::select! {
            () = timeout => StopCause::Timeout,
            () = cancelled => StopCause::Cancelled,
            signal = signal => StopCause::Interrupted(signal),
        }
    }
}

/// Runs `work`, the whole headless session from opening to teardown, until
/// it ends. The first stop on the way trips `stop`, which the session's
/// prompt watches through [`cancel_on_stop`], and `work` still runs to its
/// end. Returns what `work` came to and the stop, if one came first.
pub(super) async fn until_stopped<T>(
    stops: &mut Stops,
    stop: &CancellationToken,
    work: impl Future<Output = T>,
) -> (T, Option<StopCause>) {
    tokio::pin!(work);
    tokio::select! {
        biased;
        ended = &mut work => (ended, None),
        cause = stops.next() => {
            stop.cancel();
            (work.await, Some(cause))
        }
    }
}

/// Runs `prompt` on the runtime `handle` belongs to, cancelling it through
/// `handle` once `stop` trips, including when it already has. Begin the
/// prompt first thing in `prompt`, so the cancel reaches it.
pub(super) async fn cancel_on_stop<T>(
    stop: &CancellationToken,
    handle: SessionHandle,
    prompt: impl Future<Output = T>,
) -> T {
    tokio::pin!(prompt);
    tokio::select! {
        biased;
        ended = &mut prompt => ended,
        () = stop.cancelled() => {
            handle.cancel();
            prompt.await
        }
    }
}

/// How a headless prompt ended.
#[derive(Debug)]
pub(super) enum PromptEnd {
    /// The run finished, at its answer or its step limit.
    Finished(RunOutcome),
    Stopped(StopCause),
    Failed(anyhow::Error),
}

impl PromptEnd {
    /// The end of a prompt that came to `ended`, or that `stop` stopped
    /// first: a stop wins over the prompt's own end, as a run cancelled
    /// by it fails or is cancelled. A cancel nothing here asked for counts
    /// as the parent's.
    pub(super) fn of(ended: anyhow::Result<TurnOutcome>, stop: Option<StopCause>) -> Self {
        match (stop, ended) {
            (Some(cause), _) => Self::Stopped(cause),
            (None, Ok(TurnOutcome::Completed(outcome))) => Self::Finished(outcome),
            (None, Ok(TurnOutcome::Cancelled)) => Self::Stopped(StopCause::Cancelled),
            (None, Err(error)) => Self::Failed(error),
        }
    }
}
