//! Parent-session tracking for background workflow runs.
//!
//! Mirrors delegated-agent notifications: start records a handle for the parent
//! session, finish stores a bounded terminal snapshot, and the TUI drains
//! unobserved terminals at the next turn boundary.

use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
    time::Instant,
};

use crate::{
    app::workflow_runtime::RuntimeEvent,
    workflow::{NodeState, RunLifecycle, StoredRun, WorkflowOutcome, WorkflowValue},
};

const MODEL_NOTIFICATION_BYTES: usize = 16 * 1024;
const RESULT_EXCERPT_BYTES: usize = 4 * 1024;
const NOTIFICATION_HEADER: &str = "[workflow notification]\n\nRun status:\n";
const NOTIFICATION_FOOTER: &str = "\n\nAny omitted details remain available through the workflow tool (`status`) or `/workflow`. This is an automated notification, not a user message. Fold the results into your ongoing work; do not poll in a loop.\n";
const START_CONTEXT_FOOTER: &str = "\nCompletion is delivered automatically when the run finishes. Use the workflow tool with action `status` or `cancel` on this run_id only when you need a live check or stop. Do not poll in a loop.\n";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkflowNodeLine {
    pub(crate) node_id: String,
    pub(crate) state: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkflowFinishedSnapshot {
    pub(crate) lifecycle: String,
    pub(crate) outcome: Option<String>,
    pub(crate) nodes: Vec<WorkflowNodeLine>,
    pub(crate) error: Option<String>,
    /// Compact validated outputs from terminal nodes, when present.
    pub(crate) outputs: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkflowNotification {
    pub(crate) run_id: String,
    pub(crate) workflow_name: String,
    pub(crate) program_digest: String,
    pub(crate) finished: WorkflowFinishedSnapshot,
}

/// In-memory activity snapshot; drawing never reads the durable workflow store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkflowRailSummary {
    pub(crate) run_id: String,
    pub(crate) workflow_name: String,
    pub(crate) lifecycle: RunLifecycle,
    pub(crate) outcome: Option<WorkflowOutcome>,
    pub(crate) failed: bool,
    pub(crate) completed_tasks: usize,
    pub(crate) total_tasks: usize,
    pub(crate) active_task: Option<String>,
    pub(crate) elapsed_seconds: u64,
}

impl WorkflowRailSummary {
    pub(crate) fn is_live(&self) -> bool {
        !self.failed
            && matches!(
                self.lifecycle,
                RunLifecycle::Planned | RunLifecycle::Running | RunLifecycle::Cancelling
            )
    }

    pub(crate) fn is_failure(&self) -> bool {
        self.failed
            || self.lifecycle == RunLifecycle::NeedsRecovery
            || self
                .outcome
                .is_some_and(|outcome| outcome != WorkflowOutcome::Success)
    }
}

#[derive(Clone, Debug)]
struct WorkflowEntry {
    run_id: String,
    workflow_name: String,
    program_digest: String,
    session_id: Option<String>,
    started: Instant,
    finished_at: Option<Instant>,
    lifecycle: RunLifecycle,
    outcome: Option<WorkflowOutcome>,
    tasks: BTreeMap<String, NodeState>,
    finished: Option<WorkflowFinishedSnapshot>,
    observed: bool,
    explicitly_observed: bool,
}

#[derive(Default)]
struct Inner {
    parent_session_id: Option<String>,
    runs: HashMap<String, WorkflowEntry>,
}

/// Shared registry of parent-owned background workflow runs.
#[derive(Clone, Default)]
pub struct WorkflowRunTracker {
    inner: Arc<Mutex<Inner>>,
}

impl WorkflowRunTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bind_parent_session(&self, session_id: impl Into<String>) {
        self.inner
            .lock()
            .expect("workflow tracker lock")
            .parent_session_id = Some(session_id.into());
    }

    pub fn parent_session_id(&self) -> Option<String> {
        self.inner
            .lock()
            .expect("workflow tracker lock")
            .parent_session_id
            .clone()
    }

    /// Records a start, returning false if this tracker already owns its driver.
    /// A duplicate launch must not replace the original driver's delivery state.
    pub fn register_start(
        &self,
        run_id: impl Into<String>,
        workflow_name: impl Into<String>,
        program_digest: impl Into<String>,
        session_id: Option<String>,
    ) -> bool {
        let run_id = run_id.into();
        let session_id = session_id.or_else(|| self.parent_session_id());
        let mut inner = self.inner.lock().expect("workflow tracker lock");
        if inner
            .runs
            .get(&run_id)
            .is_some_and(|entry| entry.finished.is_none())
        {
            return false;
        }
        inner.runs.insert(
            run_id.clone(),
            WorkflowEntry {
                run_id,
                workflow_name: workflow_name.into(),
                program_digest: program_digest.into(),
                session_id,
                started: Instant::now(),
                finished_at: None,
                lifecycle: RunLifecycle::Planned,
                outcome: None,
                tasks: BTreeMap::new(),
                finished: None,
                observed: false,
                explicitly_observed: false,
            },
        );
        true
    }

    /// Seed starts/resumes and refresh task states/lifecycle at runtime
    /// state-change events, including tasks completed before a resume.
    pub(crate) fn update_from_stored(&self, run: &StoredRun) {
        let mut inner = self.inner.lock().expect("workflow tracker lock");
        let Some(entry) = inner.runs.get_mut(&run.manifest.run_id.to_string()) else {
            return;
        };
        if entry.finished.is_some() {
            return;
        }
        entry.lifecycle = run.state.state.lifecycle;
        entry.outcome = run.state.state.outcome();
        entry.tasks = run
            .state
            .state
            .tasks()
            .map(|(id, state)| (id.to_string(), state.clone()))
            .collect();
    }

    /// Apply transient node activity without filesystem reads. Late events from
    /// a drained driver cannot revive an already terminal notification.
    pub(crate) fn record_event(&self, run_id: &str, event: &RuntimeEvent) {
        let mut inner = self.inner.lock().expect("workflow tracker lock");
        let Some(entry) = inner.runs.get_mut(run_id) else {
            return;
        };
        if entry.finished.is_some() {
            return;
        }
        match event {
            RuntimeEvent::StateChanged { activity, .. } => {
                entry.lifecycle = activity.lifecycle;
                entry.outcome = activity.outcome;
                entry.tasks.clone_from(&activity.tasks);
            }
            RuntimeEvent::NodeProgress { .. } => {}
            RuntimeEvent::NodeStarted { node, attempt } => {
                if entry.lifecycle == RunLifecycle::Planned {
                    entry.lifecycle = RunLifecycle::Running;
                }
                entry
                    .tasks
                    .insert(node.to_string(), NodeState::Running { attempt: *attempt });
            }
            RuntimeEvent::NodeFinished { node, outcome } => {
                entry
                    .tasks
                    .insert(node.to_string(), NodeState::Terminal { outcome: *outcome });
            }
            RuntimeEvent::NeedsRecovery { .. } => entry.lifecycle = RunLifecycle::NeedsRecovery,
            RuntimeEvent::Completed => entry.lifecycle = RunLifecycle::Completed,
        }
    }

    /// Active and finished-undelivered rows belonging to this parent session.
    pub(crate) fn rail_summaries(&self, session_id: &str) -> Vec<WorkflowRailSummary> {
        let inner = self.inner.lock().expect("workflow tracker lock");
        let mut entries = inner
            .runs
            .values()
            .filter(|entry| {
                entry.session_id.as_deref() == Some(session_id)
                    && (entry.finished.is_none() || !entry.observed)
            })
            .collect::<Vec<_>>();
        entries.sort_by(|a, b| {
            a.started
                .cmp(&b.started)
                .then_with(|| a.run_id.cmp(&b.run_id))
        });
        entries
            .into_iter()
            .map(|entry| WorkflowRailSummary {
                run_id: entry.run_id.clone(),
                workflow_name: entry.workflow_name.clone(),
                lifecycle: entry.lifecycle,
                outcome: entry.outcome,
                failed: entry
                    .finished
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.error.is_some()),
                completed_tasks: entry
                    .tasks
                    .values()
                    .filter(|state| state.terminal().is_some())
                    .count(),
                total_tasks: entry.tasks.len(),
                active_task: entry.tasks.iter().find_map(|(id, state)| {
                    matches!(state, NodeState::Running { .. }).then(|| id.clone())
                }),
                elapsed_seconds: entry
                    .finished_at
                    .unwrap_or_else(Instant::now)
                    .duration_since(entry.started)
                    .as_secs(),
            })
            .collect()
    }

    /// Stores the terminal snapshot. No-op when the run was never registered.
    pub fn mark_finished(&self, run_id: &str, finished: WorkflowFinishedSnapshot) {
        let _delivery = crate::app::notification_delivery::lock();
        let mut inner = self.inner.lock().expect("workflow tracker lock");
        let Some(entry) = inner.runs.get_mut(run_id) else {
            return;
        };
        entry.finished_at.get_or_insert_with(Instant::now);
        entry.lifecycle = match finished.lifecycle.as_str() {
            "planned" => RunLifecycle::Planned,
            "running" => RunLifecycle::Running,
            "cancelling" => RunLifecycle::Cancelling,
            "needs_recovery" => RunLifecycle::NeedsRecovery,
            "completed" => RunLifecycle::Completed,
            _ => entry.lifecycle,
        };
        entry.outcome = match finished.outcome.as_deref() {
            Some("success") => Some(WorkflowOutcome::Success),
            Some("failure") => Some(WorkflowOutcome::Failure),
            Some("denial") => Some(WorkflowOutcome::Denial),
            Some("cancellation") => Some(WorkflowOutcome::Cancellation),
            Some("blocked") => Some(WorkflowOutcome::Blocked),
            _ => entry.outcome,
        };
        entry.finished = Some(finished);
    }

    pub fn mark_finished_from_stored(&self, run: &StoredRun) {
        self.update_from_stored(run);
        self.mark_finished(&run.manifest.run_id.to_string(), snapshot_from_stored(run));
    }

    pub fn mark_failed(&self, run_id: &str, error: impl Into<String>) {
        self.mark_finished(
            run_id,
            WorkflowFinishedSnapshot {
                lifecycle: "failed".into(),
                outcome: None,
                nodes: Vec::new(),
                error: Some(error.into()),
                outputs: Vec::new(),
            },
        );
    }

    /// Marks a terminal run observed so automatic delivery does not repeat it.
    pub fn observe(&self, run_id: &str) {
        let _delivery = crate::app::notification_delivery::lock();
        let mut inner = self.inner.lock().expect("workflow tracker lock");
        let Some(entry) = inner.runs.get_mut(run_id) else {
            return;
        };
        if entry.finished.is_some() {
            entry.observed = true;
            entry.explicitly_observed = true;
        }
    }

    pub fn has_active_or_pending_notification(&self, session_id: &str) -> bool {
        self.inner
            .lock()
            .expect("workflow tracker lock")
            .runs
            .values()
            .any(|entry| {
                entry.session_id.as_deref() == Some(session_id)
                    && (entry.finished.is_none() || !entry.observed)
            })
    }

    /// Drains unobserved terminal runs for the session, oldest first.
    pub fn take_notifications(&self, session_id: &str) -> Vec<WorkflowNotification> {
        let mut inner = self.inner.lock().expect("workflow tracker lock");
        let mut notifications = inner
            .runs
            .values_mut()
            .filter_map(|entry| {
                if entry.session_id.as_deref() != Some(session_id)
                    || entry.observed
                    || entry.finished.is_none()
                {
                    return None;
                }
                entry.observed = true;
                let finished = entry.finished.clone().expect("checked above");
                Some((
                    entry.started,
                    WorkflowNotification {
                        run_id: entry.run_id.clone(),
                        workflow_name: entry.workflow_name.clone(),
                        program_digest: entry.program_digest.clone(),
                        finished,
                    },
                ))
            })
            .collect::<Vec<_>>();
        notifications.sort_by(|(a_started, a), (b_started, b)| {
            a_started
                .cmp(b_started)
                .then_with(|| a.run_id.cmp(&b.run_id))
        });
        notifications
            .into_iter()
            .map(|(_, notification)| notification)
            .collect()
    }

    /// Returns drained notifications so a failed turn setup can deliver them
    /// again. Only terminal entries still present are reopened.
    pub fn restore_notifications(&self, notifications: &[WorkflowNotification]) {
        if notifications.is_empty() {
            return;
        }
        let mut inner = self.inner.lock().expect("workflow tracker lock");
        for notification in notifications {
            let Some(entry) = inner.runs.get_mut(&notification.run_id) else {
                continue;
            };
            if entry.finished.is_some() && entry.observed && !entry.explicitly_observed {
                entry.observed = false;
            }
        }
    }
}

pub(crate) fn start_context_prompts(
    run_id: &str,
    workflow_name: &str,
    program_digest: &str,
) -> (String, String) {
    let model = format!(
        "[workflow started]\n\nrun_id: {run_id}\nworkflow: {workflow_name}\nprogram_digest: {program_digest}\nstate: running\n{START_CONTEXT_FOOTER}"
    );
    let display = format!("workflow {workflow_name} started (run {run_id})");
    (model, display)
}

pub(crate) fn notification_prompts(notifications: &[WorkflowNotification]) -> (String, String) {
    let body_budget = MODEL_NOTIFICATION_BYTES
        .saturating_sub(NOTIFICATION_HEADER.len() + NOTIFICATION_FOOTER.len());
    let mut body = super::notification_format::join_budgeted_sections(
        notifications.iter().map(format_notification_summary),
        "\n\n",
        body_budget,
        |remaining| {
            format!("... {remaining} workflow status section(s) omitted; use workflow status")
        },
    );

    let mut outputs_section = String::new();
    for notification in notifications {
        for (name, value) in &notification.finished.outputs {
            let label = format!("\n\n{}/{}:\n", notification.run_id, name);
            if outputs_section.is_empty() {
                outputs_section.push_str("\n\nWorkflow exports:");
            }
            if body.len() + outputs_section.len() + label.len() >= body_budget {
                break;
            }
            outputs_section.push_str(&label);
            let available = (body_budget.saturating_sub(body.len() + outputs_section.len()))
                .min(RESULT_EXCERPT_BYTES);
            push_excerpt(&mut outputs_section, value, available);
        }
    }
    if body.len() + outputs_section.len() <= body_budget {
        body.push_str(&outputs_section);
    }

    let model = format!("{NOTIFICATION_HEADER}{body}{NOTIFICATION_FOOTER}");
    let display = notifications
        .iter()
        .map(|notification| {
            let outcome = notification
                .finished
                .outcome
                .as_deref()
                .unwrap_or(notification.finished.lifecycle.as_str());
            format!(
                "workflow {} ({}) finished - {}",
                notification.run_id, notification.workflow_name, outcome
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    (model, display)
}

fn format_notification_summary(notification: &WorkflowNotification) -> String {
    let mut lines = vec![format!(
        "workflow {} ({}): {}",
        notification.run_id, notification.workflow_name, notification.finished.lifecycle
    )];
    if let Some(outcome) = &notification.finished.outcome {
        lines.push(format!("outcome: {outcome}"));
    }
    lines.push(format!("program_digest: {}", notification.program_digest));
    if let Some(error) = &notification.finished.error {
        lines.push(format!("error: {error}"));
    }
    if !notification.finished.nodes.is_empty() {
        lines.push("nodes:".into());
        for node in &notification.finished.nodes {
            lines.push(format!("  {} · {}", node.node_id, node.state));
        }
    }
    lines.join("\n")
}

pub(crate) fn snapshot_from_stored(run: &StoredRun) -> WorkflowFinishedSnapshot {
    let lifecycle = run.state.state.lifecycle.as_str().into();
    let outcome = run
        .state
        .state
        .outcome()
        .map(WorkflowOutcome::as_str)
        .map(str::to_owned);
    let nodes = run
        .state
        .state
        .tasks()
        .map(|(node_id, state)| WorkflowNodeLine {
            node_id: node_id.to_string(),
            state: state.as_str().into(),
        })
        .collect();
    let mut outputs = Vec::new();
    if let Some(result) = run.state.state.run_result() {
        for (name, value) in &result.outputs {
            if let Some(text) = compact_output(value) {
                outputs.push((name.clone(), text));
            }
        }
    }
    WorkflowFinishedSnapshot {
        lifecycle,
        outcome,
        nodes,
        error: None,
        outputs,
    }
}

fn compact_output(value: &WorkflowValue) -> Option<String> {
    serde_json::to_string(value).ok()
}

fn push_excerpt(body: &mut String, text: &str, budget: usize) {
    if budget == 0 {
        return;
    }
    if text.len() <= budget {
        body.push_str(text);
        return;
    }
    let keep = budget.saturating_sub(rho_sdk::ASCII_ELLIPSIS.len());
    let boundary = rho_sdk::floor_char_boundary(text, keep);
    body.push_str(&text[..boundary]);
    body.push_str(rho_sdk::ASCII_ELLIPSIS);
}

#[cfg(test)]
#[path = "workflow_tracker_tests.rs"]
mod tests;
