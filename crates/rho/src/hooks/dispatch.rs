//! Blocking and observational hook dispatch.
//!
//! The blocking path fails closed: anything short of a valid `continue` denies,
//! and the denial names the hook so a broken program is survivable rather than
//! mysterious. The observational path never blocks the agent: events go through
//! a bounded queue whose overflow is counted, not awaited.

use std::{
    sync::{Arc, RwLock},
    time::Duration,
};

use rho_sdk::{
    hooks::{
        HookDecision, HookEnvelope, HookEventKind, HookGateFuture, HookObserveFuture, HookObserver,
        HookPayloadBounds, PreToolUseGate, PreToolUseRequest,
    },
    CancellationToken,
};
use tokio::sync::mpsc;

use super::{
    activity::{HookActivity, HookActivityLog, HookOutcome},
    catalog::HookCatalog,
    command::{run_hook, HookRunError, HookRunOutput},
    config::HookDefinition,
    protocol::parse_decision,
};

/// Ceiling on the total time one `before_tool_use` dispatch may take.
///
/// Per-hook timeouts alone let a long chain stall a tool call, so the batch has
/// its own deadline. Exhausting it denies, like any other blocking failure.
pub const MAX_BLOCKING_DISPATCH: Duration = Duration::from_secs(30);

/// How many observational events may wait for the worker.
///
/// Overflow drops the newest event and records it. Dropping is the documented
/// behavior because the alternative, waiting, would make an observational hook
/// block the turn it was supposed to only watch.
pub const OBSERVATIONAL_QUEUE_CAPACITY: usize = 256;

/// Shared hook state, swappable on config reload.
///
/// A blocking dispatch takes one `Arc` snapshot up front, so a reload can never
/// change the hook set halfway through a decision.
pub struct HookEngine {
    catalog: RwLock<Arc<HookCatalog>>,
    activity: HookActivityLog,
    bounds: HookPayloadBounds,
}

impl HookEngine {
    pub fn new(catalog: HookCatalog, bounds: HookPayloadBounds) -> Self {
        Self {
            catalog: RwLock::new(Arc::new(catalog)),
            activity: HookActivityLog::default(),
            bounds,
        }
    }

    /// Replaces the hook set for dispatches that have not started yet.
    pub fn reload(&self, catalog: HookCatalog) {
        *self
            .catalog
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Arc::new(catalog);
    }

    pub fn catalog(&self) -> Arc<HookCatalog> {
        Arc::clone(
            &self
                .catalog
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }

    pub fn activity(&self) -> Vec<HookActivity> {
        self.activity.snapshot()
    }

    fn record(&self, activity: HookActivity) {
        self.activity.record(activity);
    }
}

/// Deny-only gate backed by configured `before_tool_use` hooks.
pub struct CommandHookGate {
    engine: Arc<HookEngine>,
}

impl CommandHookGate {
    pub fn new(engine: Arc<HookEngine>) -> Self {
        Self { engine }
    }
}

impl PreToolUseGate for CommandHookGate {
    fn applies_to_tool(&self, tool_name: &str) -> bool {
        !self
            .engine
            .catalog()
            .matching(HookEventKind::BeforeToolUse, Some(tool_name))
            .is_empty()
    }

    fn evaluate(&self, request: PreToolUseRequest) -> HookGateFuture<'_> {
        Box::pin(async move {
            let catalog = self.engine.catalog();
            let tool = request.envelope().payload().tool_name();
            let hooks = catalog.matching(HookEventKind::BeforeToolUse, tool);
            if hooks.is_empty() {
                return HookDecision::Continue;
            }
            let deadline = tokio::time::Instant::now() + MAX_BLOCKING_DISPATCH;
            let Ok(encoded) = request.envelope().to_bounded_json(self.engine.bounds) else {
                // A payload we cannot bound is an infrastructure failure, and
                // blocking infrastructure failures deny.
                return deny_all(
                    &self.engine,
                    &hooks,
                    "event payload exceeded its size bound",
                );
            };
            for hook in hooks {
                match evaluate_one(&self.engine, hook, &encoded, deadline).await {
                    HookDecision::Continue => {}
                    denial => return denial,
                }
            }
            HookDecision::Continue
        })
    }
}

/// Runs one blocking hook and turns everything except a valid `continue` into a
/// denial that names the hook and the failure.
async fn evaluate_one(
    engine: &HookEngine,
    hook: &HookDefinition,
    event: &str,
    deadline: tokio::time::Instant,
) -> HookDecision {
    let id = hook.qualified_id();
    if tokio::time::Instant::now() >= deadline {
        return record_denial(
            engine,
            &id,
            HookEventKind::BeforeToolUse,
            None,
            false,
            format!("hook `{id}` was not run: the blocking hook budget was exhausted"),
        );
    }
    let result = tokio::time::timeout_at(deadline, run_hook(hook, event, CancellationToken::new()))
        .await
        .unwrap_or(Err(HookRunError::TimedOut {
            timeout: hook.timeout(),
        }));
    let output = match result {
        Ok(output) => output,
        Err(error) => {
            return record_denial(
                engine,
                &id,
                HookEventKind::BeforeToolUse,
                None,
                false,
                format!("denied: hook `{id}` {error}"),
            )
        }
    };
    interpret(engine, &id, hook, output)
}

fn interpret(
    engine: &HookEngine,
    id: &str,
    hook: &HookDefinition,
    output: HookRunOutput,
) -> HookDecision {
    let duration = Some(output.duration);
    if !output.succeeded() {
        // A nonzero exit may still carry a valid deny; anything else is a
        // failure, and failures deny.
        if let Ok(HookDecision::Deny { reason }) = parse_decision(&output.stdout) {
            return record_denial(
                engine,
                id,
                hook.event(),
                duration,
                output.truncated,
                format!("denied by hook `{id}`: {reason}"),
            );
        }
        let detail = exit_detail(&output);
        return record_denial(
            engine,
            id,
            hook.event(),
            duration,
            output.truncated,
            format!("denied: hook `{id}` {detail}"),
        );
    }
    match parse_decision(&output.stdout) {
        Ok(HookDecision::Continue) => {
            engine.record(HookActivity {
                hook_id: id.to_owned(),
                event: hook.event().wire_name(),
                outcome: HookOutcome::Continued,
                duration,
                truncated: output.truncated,
            });
            HookDecision::Continue
        }
        Ok(HookDecision::Deny { reason }) => record_denial(
            engine,
            id,
            hook.event(),
            duration,
            output.truncated,
            format!("denied by hook `{id}`: {reason}"),
        ),
        Ok(_) => record_denial(
            engine,
            id,
            hook.event(),
            duration,
            output.truncated,
            format!("denied: hook `{id}` returned an unsupported decision"),
        ),
        Err(error) => record_denial(
            engine,
            id,
            hook.event(),
            duration,
            output.truncated,
            format!("denied: hook `{id}` {error}"),
        ),
    }
}

fn exit_detail(output: &HookRunOutput) -> String {
    let code = output
        .exit_code
        .map(|code| format!("exited with status {code}"))
        .unwrap_or_else(|| "was terminated by a signal".into());
    let stderr = output.stderr_summary();
    if stderr.is_empty() {
        code
    } else {
        format!("{code}: {stderr}")
    }
}

fn record_denial(
    engine: &HookEngine,
    id: &str,
    event: HookEventKind,
    duration: Option<Duration>,
    truncated: bool,
    reason: String,
) -> HookDecision {
    engine.record(HookActivity {
        hook_id: id.to_owned(),
        event: event.wire_name(),
        outcome: HookOutcome::Denied {
            reason: reason.clone(),
        },
        duration,
        truncated,
    });
    HookDecision::Deny { reason }
}

fn deny_all(engine: &HookEngine, hooks: &[&HookDefinition], detail: &str) -> HookDecision {
    let id = hooks
        .first()
        .map(|hook| hook.qualified_id())
        .unwrap_or_else(|| "<unknown>".into());
    record_denial(
        engine,
        &id,
        HookEventKind::BeforeToolUse,
        None,
        true,
        format!("denied: hook `{id}` was not run because the {detail}"),
    )
}

/// Observational sink that enqueues and returns.
pub struct QueuedHookObserver {
    engine: Arc<HookEngine>,
    sender: mpsc::Sender<HookEnvelope>,
}

impl HookObserver for QueuedHookObserver {
    fn observe(&self, envelope: HookEnvelope) -> HookObserveFuture<'_> {
        Box::pin(async move {
            if self.sender.try_send(envelope).is_err() {
                // Full queue or a stopped worker. Dropping keeps the turn free;
                // the drop is recorded so it is never silent.
                self.engine.record(HookActivity {
                    hook_id: "<queue>".into(),
                    event: "observational",
                    outcome: HookOutcome::Dropped,
                    duration: None,
                    truncated: false,
                });
            }
        })
    }
}

/// Background worker that runs observational hooks off the agent's path.
pub struct ObservationalWorker {
    handle: tokio::task::JoinHandle<()>,
}

impl ObservationalWorker {
    /// Waits a bounded time for queued events to finish, then stops.
    pub async fn drain(self, grace: Duration) {
        let _ = tokio::time::timeout(grace, self.handle).await;
    }
}

/// Starts the observational pipeline and returns its sink and worker.
pub fn observational_channel(
    engine: Arc<HookEngine>,
    cancellation: CancellationToken,
) -> (QueuedHookObserver, ObservationalWorker) {
    let (sender, mut receiver) = mpsc::channel(OBSERVATIONAL_QUEUE_CAPACITY);
    let worker_engine = Arc::clone(&engine);
    let handle = tokio::spawn(async move {
        while let Some(envelope) = receiver.recv().await {
            run_observational(&worker_engine, envelope, &cancellation).await;
        }
    });
    (
        QueuedHookObserver { engine, sender },
        ObservationalWorker { handle },
    )
}

async fn run_observational(
    engine: &HookEngine,
    envelope: HookEnvelope,
    cancellation: &CancellationToken,
) {
    let catalog = engine.catalog();
    let hooks = catalog.matching(envelope.event(), envelope.payload().tool_name());
    if hooks.is_empty() {
        return;
    }
    let Ok(encoded) = envelope.to_bounded_json(engine.bounds) else {
        engine.record(HookActivity {
            hook_id: "<queue>".into(),
            event: envelope.event().wire_name(),
            outcome: HookOutcome::Failed {
                reason: "event payload exceeded its size bound".into(),
            },
            duration: None,
            truncated: true,
        });
        return;
    };
    for hook in hooks {
        let id = hook.qualified_id();
        // An observational failure is visible but never fails the run.
        let outcome = match run_hook(hook, &encoded, cancellation.clone()).await {
            Ok(output) if output.succeeded() => (HookOutcome::Observed, Some(output)),
            Ok(output) => (
                HookOutcome::Failed {
                    reason: exit_detail(&output),
                },
                Some(output),
            ),
            Err(error) => (
                HookOutcome::Failed {
                    reason: error.to_string(),
                },
                None,
            ),
        };
        engine.record(HookActivity {
            hook_id: id,
            event: hook.event().wire_name(),
            outcome: outcome.0,
            duration: outcome.1.as_ref().map(|output| output.duration),
            truncated: outcome.1.is_some_and(|output| output.truncated),
        });
    }
}

#[cfg(test)]
#[path = "dispatch_tests.rs"]
mod tests;
