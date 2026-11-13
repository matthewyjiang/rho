//! The host for sessions nobody watches live: `rho run`, Rho subagents, and
//! workflow agent nodes. Each runs one prompt on a
//! [`SessionRuntime`] through [`run_prompt`], and shows it through a
//! [`HeadlessView`] (see `headless_host/view.rs`), which writes the run's
//! events to the JSONL stream, `rho run`'s text output, and a subagent's
//! [`RunReporter`] status file.
//!
//! A headless prompt also ends early: a deadline, SIGINT/SIGTERM, or a
//! parent's cancel stops it. A stop cancels the prompt through the
//! runtime's [`super::session_runtime::SessionHandle`] and lets it finish, so
//! the session saves what it did and tears down before the host reports
//! the stop, which wins over however the prompt itself ended.
//!
//! `automation.rs` runs `rho run` on it; `headless_host/child.rs` runs a
//! delegated child or a workflow node.

use std::{fmt, future::Future, pin::Pin, sync::OnceLock};

use rho_sdk::{CancellationToken, RunOutcome};
use rho_tools::cancellation::RunCancellation;

use super::{
    session_runtime::{PromptScope, SessionHandle, SessionRuntime, TurnOutcome},
    shutdown_signal::{ShutdownSignal, ShutdownSignals},
};
use crate::config::Config;

mod child;
mod reporter;
mod view;

pub(super) use child::{run_child, ChildRun, ChildSession};
pub(super) use reporter::RunReporter;
pub(super) use view::{HeadlessView, ParentQuestions};

/// What stops a headless prompt before it ends on its own. Signals are
/// installed before the run is observable; see `shutdown_signal.rs`. The
/// default stops on nothing.
#[derive(Default)]
pub(super) struct Stops {
    signals: Option<ShutdownSignals>,
    deadline: Option<tokio::time::Instant>,
    cancellation: Option<RunCancellation>,
}

/// Why a stop ended a headless prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StopCause {
    /// `rho run --timeout` ran out.
    Timeout,
    Interrupted(ShutdownSignal),
    /// The parent cancelled a delegated child or workflow node.
    Cancelled,
}

impl fmt::Display for StopCause {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => formatter.write_str("timed out"),
            Self::Interrupted(signal) => write!(formatter, "interrupted by {signal}"),
            Self::Cancelled => formatter.write_str("cancelled by its parent"),
        }
    }
}

impl Stops {
    /// Stops on `signals` only.
    pub(super) fn on_signals(signals: ShutdownSignals) -> Self {
        Self {
            signals: Some(signals),
            ..Self::default()
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
            match signals {
                Some(signals) => match signals.recv().await {
                    Ok(signal) => signal,
                    Err(error) => {
                        tracing::warn!(%error, "could not watch for shutdown signals");
                        std::future::pending().await
                    }
                },
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            () = timeout => StopCause::Timeout,
            () = cancelled => StopCause::Cancelled,
            signal = signal => StopCause::Interrupted(signal),
        }
    }
}

/// What a headless prompt runs once it has begun, such as a model turn:
/// the future a [`run_prompt`] body returns, borrowing what the driver
/// lends it.
pub(super) type PromptBody<'r> =
    Pin<Box<dyn Future<Output = anyhow::Result<TurnOutcome>> + Send + 'r>>;

/// Runs one headless prompt, from opening its session to tearing it down,
/// until it ends or `stops` stops it, and returns how it ended.
///
/// `open` opens the session. Once it is open the driver binds `view` to it
/// and begins the prompt, then hands `body` the runtime, the view, the
/// prompt's scope, and `process_config` to run within the prompt. A stop
/// cancels the prompt, which reaches it however early it comes, and the
/// session still tears down before this returns. A cancelled prompt ends
/// the session as failed to its hooks, since nothing else reports it.
pub(super) async fn run_prompt<'v>(
    stops: &mut Stops,
    view: &mut HeadlessView<'v>,
    process_config: &Config,
    open: impl Future<Output = anyhow::Result<SessionRuntime>>,
    body: impl for<'r> FnOnce(
        &'r mut SessionRuntime,
        &'r mut HeadlessView<'v>,
        &'r PromptScope,
        &'r Config,
    ) -> PromptBody<'r>,
) -> PromptEnd {
    // Opening a session builds a large future; keep it and the work that
    // holds it on the heap, since a 1 MiB main-thread stack (Windows) cannot
    // hold the copies moving them by value makes.
    let open = Box::pin(open);
    let tripwire = Tripwire::default();
    let work = Box::pin(async {
        let mut runtime = open.await?;
        view.bind(&runtime, process_config);
        let ended = cancel_on_stop(&tripwire, runtime.handle(), async {
            // The prompt begins on the first poll, before a stop's cancel
            // can land, so even a stop that already came reaches it.
            let scope = runtime.begin_prompt(process_config).await?;
            body(&mut runtime, &mut *view, &scope, process_config).await
        })
        .await;
        if let Ok(TurnOutcome::Cancelled) = &ended {
            let reason = match tripwire.cause() {
                Some(cause) => cause.to_string(),
                None => "cancelled".to_owned(),
            };
            runtime.end_stopped(&reason);
        }
        runtime.shutdown().await;
        ended
    });
    let ended = until_stopped(stops, &tripwire, work).await;
    PromptEnd::of(ended, tripwire.cause())
}

/// Trips once, at the first stop, and keeps what it was.
#[derive(Default)]
struct Tripwire {
    token: CancellationToken,
    cause: OnceLock<StopCause>,
}

impl Tripwire {
    fn trip(&self, cause: StopCause) {
        let _ = self.cause.set(cause);
        self.token.cancel();
    }

    fn cause(&self) -> Option<StopCause> {
        self.cause.get().copied()
    }
}

/// Runs `work`, the whole headless session from opening to teardown, until
/// it ends. The first stop on the way trips `tripwire`, which the session's
/// prompt watches through [`cancel_on_stop`], and `work` still runs to its
/// end.
async fn until_stopped<T>(
    stops: &mut Stops,
    tripwire: &Tripwire,
    work: impl Future<Output = T>,
) -> T {
    tokio::pin!(work);
    tokio::select! {
        biased;
        ended = &mut work => ended,
        cause = stops.next() => {
            tripwire.trip(cause);
            work.await
        }
    }
}

/// Runs `prompt` on the runtime `handle` belongs to, cancelling it through
/// `handle` once `tripwire` trips, including when it already has. `prompt`
/// is polled first, so it begins before the cancel lands.
async fn cancel_on_stop<T>(
    tripwire: &Tripwire,
    handle: SessionHandle,
    prompt: impl Future<Output = T>,
) -> T {
    tokio::pin!(prompt);
    tokio::select! {
        biased;
        ended = &mut prompt => ended,
        () = tripwire.token.cancelled() => {
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
    /// by it fails or is cancelled. A prompt cancelled though nothing
    /// stopped it failed.
    fn of(ended: anyhow::Result<TurnOutcome>, stop: Option<StopCause>) -> Self {
        match (stop, ended) {
            (Some(cause), _) => Self::Stopped(cause),
            (None, Ok(TurnOutcome::Completed(outcome))) => Self::Finished(outcome),
            (None, Ok(TurnOutcome::Cancelled)) => {
                Self::Failed(anyhow::anyhow!("the prompt was cancelled"))
            }
            (None, Err(error)) => Self::Failed(error),
        }
    }
}

#[cfg(test)]
#[path = "headless_host_tests.rs"]
mod tests;
