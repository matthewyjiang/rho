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
        HookDecision, HookEnvelope, HookEventKind, HookGateFuture, HookObserver, HookPayloadBounds,
        PreToolUseGate, PreToolUseRequest,
    },
    CancellationToken,
};
use serde::Serialize;
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinSet,
};

#[path = "workflow_event.rs"]
mod workflow_event;

use super::{
    activity::{HookActivity, HookActivityLog, HookOutcome},
    catalog::HookCatalog,
    command::{run_hook, HookRunOutput},
    config::{ConfiguredHookEvent, HookDefinition, WorkflowHookEventKind},
    protocol::parse_decision,
};
use workflow_event::{AppHookEnvelope, WorkflowPayload};

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

/// Maximum number of independent observational handlers running at once.
///
/// This isolates healthy hooks from a slow peer without turning queue pressure
/// into unbounded child-process creation.
pub const OBSERVATIONAL_MAX_IN_FLIGHT: usize = 32;

/// Shared hook state, swappable on config reload.
///
/// A blocking dispatch takes one `Arc` snapshot up front, so a reload can never
/// change the hook set halfway through a decision.
pub struct HookEngine {
    catalog: RwLock<Arc<HookCatalog>>,
    activity: HookActivityLog,
    bounds: HookPayloadBounds,
    observational_sender: RwLock<Option<mpsc::Sender<ObservationalEvent>>>,
}

impl HookEngine {
    pub fn new(catalog: HookCatalog, bounds: HookPayloadBounds) -> Self {
        Self {
            catalog: RwLock::new(Arc::new(catalog)),
            activity: HookActivityLog::default(),
            bounds,
            observational_sender: RwLock::new(None),
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

    fn install_observational_sender(&self, sender: mpsc::Sender<ObservationalEvent>) {
        *self
            .observational_sender
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(sender);
    }

    fn enqueue(&self, event: ObservationalEvent) {
        let event_name = event.wire_name();
        if let Some(sender) = self
            .observational_sender
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
        {
            if sender.try_send(event).is_ok() {
                return;
            }
        }
        // A missing, full, or stopped queue cannot affect app state. Keep a
        // diagnostic record and return without waiting.
        self.record(HookActivity {
            hook_id: "<queue>".into(),
            event: event_name,
            outcome: HookOutcome::Dropped,
            duration: None,
            truncated: false,
        });
    }

    /// Notifies hooks that a frozen workflow run started.
    pub fn observe_workflow_started(&self, workflow_run_id: &str, plan_digest: &str) {
        self.enqueue_workflow(
            WorkflowHookEventKind::Started,
            WorkflowPayload::Run {
                workflow_run_id,
                plan_digest,
                outcome: None,
                duration_ms: None,
                artifacts: &[],
            },
        );
    }

    /// Notifies hooks that one workflow node attempt started.
    pub fn observe_workflow_node_started(
        &self,
        workflow_run_id: &str,
        plan_digest: &str,
        node_id: &str,
        attempt: u32,
    ) {
        self.enqueue_workflow(
            WorkflowHookEventKind::NodeStarted,
            WorkflowPayload::Node {
                workflow_run_id,
                plan_digest,
                node_id,
                attempt,
                outcome: None,
                duration_ms: None,
                artifacts: &[],
            },
        );
    }

    /// Notifies hooks that one workflow node attempt reached a terminal state.
    ///
    /// The workflow layer supplies its own serde enum. Only the six frozen node
    /// outcomes are accepted on the wire; invalid values are dropped and logged.
    pub fn observe_workflow_node_finished<O: Serialize>(
        &self,
        observation: WorkflowNodeFinished<'_, O>,
    ) {
        let WorkflowNodeFinished {
            workflow_run_id,
            plan_digest,
            node_id,
            attempt,
            outcome,
            duration,
            artifacts,
        } = observation;
        let Some(outcome) =
            self.workflow_outcome(WorkflowHookEventKind::NodeFinished, outcome, NODE_OUTCOMES)
        else {
            return;
        };
        self.enqueue_workflow(
            WorkflowHookEventKind::NodeFinished,
            WorkflowPayload::Node {
                workflow_run_id,
                plan_digest,
                node_id,
                attempt,
                outcome: Some(&outcome),
                duration_ms: Some(duration_millis(duration)),
                artifacts,
            },
        );
    }

    /// Notifies hooks that a workflow completed successfully.
    pub fn observe_workflow_completed(
        &self,
        workflow_run_id: &str,
        plan_digest: &str,
        duration: Duration,
        artifacts: &[String],
    ) {
        self.enqueue_workflow(
            WorkflowHookEventKind::Completed,
            WorkflowPayload::Run {
                workflow_run_id,
                plan_digest,
                outcome: Some("success"),
                duration_ms: Some(duration_millis(duration)),
                artifacts,
            },
        );
    }

    /// Notifies hooks that a workflow ended with a non-cancellation failure.
    pub fn observe_workflow_failed<O: Serialize>(
        &self,
        workflow_run_id: &str,
        plan_digest: &str,
        outcome: &O,
        duration: Duration,
        artifacts: &[String],
    ) {
        let Some(outcome) = self.workflow_outcome(
            WorkflowHookEventKind::Failed,
            outcome,
            WORKFLOW_FAILURE_OUTCOMES,
        ) else {
            return;
        };
        self.enqueue_workflow(
            WorkflowHookEventKind::Failed,
            WorkflowPayload::Run {
                workflow_run_id,
                plan_digest,
                outcome: Some(&outcome),
                duration_ms: Some(duration_millis(duration)),
                artifacts,
            },
        );
    }

    /// Notifies hooks that cancellation intent ended a workflow.
    pub fn observe_workflow_cancelled(
        &self,
        workflow_run_id: &str,
        plan_digest: &str,
        duration: Duration,
        artifacts: &[String],
    ) {
        self.enqueue_workflow(
            WorkflowHookEventKind::Cancelled,
            WorkflowPayload::Run {
                workflow_run_id,
                plan_digest,
                outcome: Some("cancellation"),
                duration_ms: Some(duration_millis(duration)),
                artifacts,
            },
        );
    }

    fn enqueue_workflow(&self, event: WorkflowHookEventKind, payload: WorkflowPayload<'_>) {
        if self.catalog().matching_workflow(event).is_empty() {
            return;
        }
        match AppHookEnvelope::new(event, payload, self.bounds) {
            Ok(envelope) => self.enqueue(ObservationalEvent::Workflow(envelope)),
            Err(reason) => self.record_app_event_failure(event, reason, true),
        }
    }

    fn workflow_outcome<O: Serialize>(
        &self,
        event: WorkflowHookEventKind,
        outcome: &O,
        accepted: &[&str],
    ) -> Option<String> {
        let serialized = serde_json::to_value(outcome).ok();
        let outcome = serialized.as_ref().and_then(serde_json::Value::as_str);
        if let Some(outcome) = outcome.filter(|outcome| accepted.contains(outcome)) {
            return Some((*outcome).to_owned());
        }
        self.record_app_event_failure(
            event,
            "workflow hook outcome was not a supported typed outcome".into(),
            false,
        );
        None
    }

    fn record_app_event_failure(
        &self,
        event: WorkflowHookEventKind,
        reason: String,
        truncated: bool,
    ) {
        self.record(HookActivity {
            hook_id: "<queue>".into(),
            event: event.wire_name(),
            outcome: HookOutcome::Failed { reason },
            duration: None,
            truncated,
        });
    }
}

/// App-owned data for one terminal workflow node attempt.
pub(crate) struct WorkflowNodeFinished<'a, O> {
    pub workflow_run_id: &'a str,
    pub plan_digest: &'a str,
    pub node_id: &'a str,
    pub attempt: u32,
    pub outcome: &'a O,
    pub duration: Duration,
    pub artifacts: &'a [String],
}

const NODE_OUTCOMES: &[&str] = &[
    "success",
    "failure",
    "denial",
    "cancellation",
    "skipped",
    "blocked",
];
const WORKFLOW_FAILURE_OUTCOMES: &[&str] = &["denial", "failure", "blocked"];

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

enum ObservationalEvent {
    Sdk(HookEnvelope),
    Workflow(AppHookEnvelope),
}

impl ObservationalEvent {
    fn wire_name(&self) -> &'static str {
        match self {
            Self::Sdk(envelope) => envelope.event().wire_name(),
            Self::Workflow(envelope) => envelope.wire_name(),
        }
    }

    fn configured_event(&self) -> ConfiguredHookEvent {
        match self {
            Self::Sdk(envelope) => ConfiguredHookEvent::Sdk(envelope.event()),
            Self::Workflow(envelope) => ConfiguredHookEvent::Workflow(envelope.event()),
        }
    }

    fn tool_name(&self) -> Option<&str> {
        match self {
            Self::Sdk(envelope) => envelope.payload().tool_name(),
            Self::Workflow(_) => None,
        }
    }

    fn to_bounded_json(&self, bounds: HookPayloadBounds) -> Result<String, String> {
        match self {
            Self::Sdk(envelope) => envelope
                .to_bounded_json(bounds)
                .map_err(|error| error.to_string()),
            Self::Workflow(envelope) => envelope.to_bounded_json(),
        }
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
    let result =
        match tokio::time::timeout_at(deadline, run_hook(hook, event, CancellationToken::new()))
            .await
        {
            Ok(result) => result,
            Err(_) => {
                return record_denial(
                    engine,
                    &id,
                    HookEventKind::BeforeToolUse,
                    None,
                    false,
                    format!("hook `{id}` was stopped: the blocking hook budget was exhausted"),
                );
            }
        };
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
            );
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
                HookEventKind::BeforeToolUse,
                duration,
                output.truncated,
                format!("denied by hook `{id}`: {reason}"),
            );
        }
        let detail = exit_detail(&output);
        return record_denial(
            engine,
            id,
            HookEventKind::BeforeToolUse,
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
            HookEventKind::BeforeToolUse,
            duration,
            output.truncated,
            format!("denied by hook `{id}`: {reason}"),
        ),
        Ok(_) => record_denial(
            engine,
            id,
            HookEventKind::BeforeToolUse,
            duration,
            output.truncated,
            format!("denied: hook `{id}` returned an unsupported decision"),
        ),
        Err(error) => record_denial(
            engine,
            id,
            HookEventKind::BeforeToolUse,
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
    sender: mpsc::Sender<ObservationalEvent>,
}

impl HookObserver for QueuedHookObserver {
    fn observe(&self, envelope: HookEnvelope) {
        let event = ObservationalEvent::Sdk(envelope);
        let event_name = event.wire_name();
        if self.sender.try_send(event).is_err() {
            self.engine.record(HookActivity {
                hook_id: "<queue>".into(),
                event: event_name,
                outcome: HookOutcome::Dropped,
                duration: None,
                truncated: false,
            });
        }
    }
}

/// Background worker that runs observational hooks off the agent's path.
pub struct ObservationalWorker {
    handle: Option<tokio::task::JoinHandle<()>>,
    shutdown: Option<oneshot::Sender<()>>,
    cancellation: CancellationToken,
    finished: bool,
}

impl ObservationalWorker {
    /// Waits a bounded time for queued events to finish, then stops.
    pub async fn drain(mut self, grace: Duration) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        let mut handle = self.handle.take().expect("worker handle is present");
        if tokio::time::timeout(grace, &mut handle).await.is_ok() {
            self.finished = true;
            return;
        }
        self.cancellation.cancel();
        handle.abort();
        let _ = handle.await;
        self.finished = true;
    }
}

impl Drop for ObservationalWorker {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        self.cancellation.cancel();
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

/// Starts the observational pipeline and returns its sink and worker.
pub fn observational_channel(
    engine: Arc<HookEngine>,
    cancellation: CancellationToken,
) -> (QueuedHookObserver, ObservationalWorker) {
    let (sender, mut receiver) = mpsc::channel(OBSERVATIONAL_QUEUE_CAPACITY);
    engine.install_observational_sender(sender.clone());
    let (shutdown, mut shutdown_requested) = oneshot::channel();
    let worker_engine = Arc::clone(&engine);
    let worker_cancellation = cancellation.clone();
    let handle = tokio::spawn(async move {
        let mut tasks = JoinSet::new();
        loop {
            tokio::select! {
                biased;
                () = cancellation.cancelled() => break,
                _ = &mut shutdown_requested => {
                    receiver.close();
                    while let Some(event) = receiver.recv().await {
                        dispatch_observational(
                            &worker_engine,
                            event,
                            &cancellation,
                            &mut tasks,
                        ).await;
                    }
                    while tasks.join_next().await.is_some() {}
                    break;
                }
                event = receiver.recv() => {
                    let Some(event) = event else {
                        while tasks.join_next().await.is_some() {}
                        break;
                    };
                    dispatch_observational(
                        &worker_engine,
                        event,
                        &cancellation,
                        &mut tasks,
                    ).await;
                }
                _ = tasks.join_next(), if !tasks.is_empty() => {}
            }
        }
    });
    (
        QueuedHookObserver { engine, sender },
        ObservationalWorker {
            handle: Some(handle),
            shutdown: Some(shutdown),
            cancellation: worker_cancellation,
            finished: false,
        },
    )
}

async fn dispatch_observational(
    engine: &Arc<HookEngine>,
    event: ObservationalEvent,
    cancellation: &CancellationToken,
    tasks: &mut JoinSet<()>,
) {
    let catalog = engine.catalog();
    let hooks = catalog
        .matching_configured(event.configured_event(), event.tool_name())
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    if hooks.is_empty() {
        return;
    }
    let Ok(encoded) = event.to_bounded_json(engine.bounds) else {
        engine.record(HookActivity {
            hook_id: "<queue>".into(),
            event: event.wire_name(),
            outcome: HookOutcome::Failed {
                reason: "event payload exceeded its size bound".into(),
            },
            duration: None,
            truncated: true,
        });
        return;
    };
    let encoded = Arc::new(encoded);
    for hook in hooks {
        while tasks.len() >= OBSERVATIONAL_MAX_IN_FLIGHT {
            let _ = tasks.join_next().await;
        }
        let engine = Arc::clone(engine);
        let encoded = Arc::clone(&encoded);
        let cancellation = cancellation.clone();
        tasks.spawn(async move {
            run_observational_hook(engine, hook, encoded, cancellation).await;
        });
    }
}

async fn run_observational_hook(
    engine: Arc<HookEngine>,
    hook: HookDefinition,
    encoded: Arc<String>,
    cancellation: CancellationToken,
) {
    let id = hook.qualified_id();
    // An observational failure is visible but never fails the run.
    let outcome = match run_hook(&hook, &encoded, cancellation).await {
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

#[cfg(test)]
#[path = "dispatch_tests.rs"]
mod tests;
