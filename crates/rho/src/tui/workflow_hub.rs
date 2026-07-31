//! In-app `/workflow` hub: start workflows and check runs from the chat TUI.

use std::{collections::BTreeMap, str::FromStr};

use ratatui::DefaultTerminal;

use super::{
    picker_overlay::OverlayChrome, workflow_discover, App, ComposerMode, Entry, PickerAction,
    PickerBadge, PickerBadgeTone, PickerItem, PickerLayout, UiPicker,
};
use crate::{
    agent::AgentCapabilities,
    app::{
        workflow_cli::{self, WorkflowOps},
        workflow_runtime::RecoveryDecision,
    },
    workflow::{
        derive_workflow_outcome, NodeState, NodeTerminalState, PlanId, RunId, RunLifecycle,
        StoredPlan, StoredRun, WorkflowOutcome, WorkflowValue,
    },
};

const SOURCE_PREFIX: &str = "source:";
const PLAN_PREFIX: &str = "plan:";
const RUN_PREFIX: &str = "run:";

const MAX_FINISHED_RUNS: usize = 8;

fn badge(text: impl Into<String>, tone: PickerBadgeTone) -> PickerBadge {
    PickerBadge {
        text: text.into(),
        tone,
    }
}

fn item(
    section: Option<&str>,
    label: impl Into<String>,
    detail: impl Into<String>,
    value: impl Into<String>,
    badge_text: Option<(String, PickerBadgeTone)>,
    selection_verb: Option<&'static str>,
) -> PickerItem {
    PickerItem {
        section: section.map(str::to_owned),
        label: label.into(),
        detail: Some(detail.into()),
        preview: None,
        badge: badge_text.map(|(text, tone)| badge(text, tone)),
        value: value.into(),
        selection_verb,
    }
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn lifecycle_label(lifecycle: RunLifecycle) -> &'static str {
    match lifecycle {
        RunLifecycle::Planned => "ready",
        RunLifecycle::Running => "running",
        RunLifecycle::Cancelling => "stopping",
        RunLifecycle::Completed => "finished",
        RunLifecycle::NeedsRecovery => "needs recovery",
    }
}

fn lifecycle_tone(lifecycle: RunLifecycle) -> PickerBadgeTone {
    match lifecycle {
        RunLifecycle::Running | RunLifecycle::Planned => PickerBadgeTone::Selected,
        RunLifecycle::NeedsRecovery | RunLifecycle::Cancelling => PickerBadgeTone::Warning,
        RunLifecycle::Completed => PickerBadgeTone::Internal,
    }
}

fn outcome_label(outcome: Option<WorkflowOutcome>) -> String {
    match outcome {
        Some(WorkflowOutcome::Success) => "success".into(),
        Some(WorkflowOutcome::Failure) => "failed".into(),
        Some(WorkflowOutcome::Denial) => "denied".into(),
        Some(WorkflowOutcome::Cancellation) => "cancelled".into(),
        Some(WorkflowOutcome::Blocked) => "blocked".into(),
        None => "pending".into(),
    }
}

fn node_state_label(state: &NodeState) -> String {
    match state {
        NodeState::Pending => "waiting".into(),
        NodeState::Ready => "ready".into(),
        NodeState::Running { attempt } => format!("running (try {})", attempt),
        NodeState::Terminal { outcome } => match outcome {
            NodeTerminalState::Success => "done".into(),
            NodeTerminalState::Failure => "failed".into(),
            NodeTerminalState::Denial => "denied".into(),
            NodeTerminalState::Cancellation => "cancelled".into(),
            NodeTerminalState::Skipped => "skipped".into(),
            NodeTerminalState::Blocked => "blocked".into(),
        },
    }
}

fn run_progress(run: &StoredRun) -> String {
    let total = run.state.state.nodes.len().max(1);
    let done = run
        .state
        .state
        .nodes
        .values()
        .filter(|state| state.terminal().is_some())
        .count();
    format!("{done}/{total} steps done")
}

/// Root list: start workflows, open runs, or reuse a saved plan.
pub(super) fn hub_picker(
    sources: &[workflow_discover::DiscoveredWorkflow],
    plans: &[StoredPlan],
    runs: &[StoredRun],
) -> UiPicker {
    let mut items = Vec::new();

    if sources.is_empty() {
        items.push(item(
            Some("START"),
            "No local workflows yet",
            "Add .rho/workflows/<name>/workflow.star or .rho/workflows/<name>.star, then reopen /workflow.",
            "noop:empty_sources",
            None,
            Some("close"),
        ));
    } else {
        for source in sources {
            items.push(item(
                Some("START"),
                format!("Start  {}", source.label),
                format!(
                    "Create a new run with default inputs.\nFile: {}",
                    source.relative_path
                ),
                format!("{SOURCE_PREFIX}{}", source.relative_path),
                Some(("new run".into(), PickerBadgeTone::Selected)),
                Some("start"),
            ));
        }
    }

    let mut active = runs
        .iter()
        .filter(|run| run.state.state.lifecycle != RunLifecycle::Completed)
        .collect::<Vec<_>>();
    active.sort_by_key(|run| run.manifest.run_id);
    active.reverse();

    let mut finished = runs
        .iter()
        .filter(|run| run.state.state.lifecycle == RunLifecycle::Completed)
        .collect::<Vec<_>>();
    finished.sort_by_key(|run| run.manifest.run_id);
    finished.reverse();
    finished.truncate(MAX_FINISHED_RUNS);

    if active.is_empty() && finished.is_empty() {
        items.push(item(
            Some("RUNS"),
            "No runs yet",
            "Start a workflow above. Finished and active runs show up here.",
            "noop:empty_runs",
            None,
            Some("close"),
        ));
    } else {
        for run in active {
            let id = run.manifest.run_id.to_string();
            let short = short_id(&id);
            let life = lifecycle_label(run.state.state.lifecycle);
            let name = run.graph.graph.name.as_str();
            items.push(item(
                Some("RUNS"),
                format!("Open  {life}  ·  {short}"),
                format!(
                    "{name}\n{life} · {}\nEnter opens the live graph.\nRun id {short}",
                    run_progress(run)
                ),
                format!("{RUN_PREFIX}{id}"),
                Some((life.into(), lifecycle_tone(run.state.state.lifecycle))),
                Some("open"),
            ));
        }
        for run in finished {
            let id = run.manifest.run_id.to_string();
            let short = short_id(&id);
            let outcome = outcome_label(derive_workflow_outcome(&run.graph, &run.state.state));
            let name = run.graph.graph.name.as_str();
            let tone = outcome_tone(&outcome);
            items.push(item(
                Some("RUNS"),
                format!("Status  {outcome}  ·  {short}"),
                format!(
                    "{name}\nFinished · {outcome} · {}\nEnter shows step status.\nRun id {short}",
                    run_progress(run)
                ),
                format!("{RUN_PREFIX}{id}"),
                Some((outcome, tone)),
                Some("show"),
            ));
        }
    }

    if plans.is_empty() {
        // Keep the list focused; empty plans stay hidden.
    } else {
        for plan in plans {
            let id = plan.manifest.plan_id.to_string();
            let short = short_id(&id);
            let name = plan.graph.graph.name.as_str();
            let steps = plan.graph.graph.nodes.len();
            items.push(item(
                Some("SAVED PLANS"),
                format!("Run plan  ·  {short}"),
                format!(
                    "{name}\n{steps} steps already frozen.\nEnter starts a new run from this plan.\nPlan id {short}"
                ),
                format!("{PLAN_PREFIX}{id}"),
                Some(("saved".into(), PickerBadgeTone::Internal)),
                Some("run"),
            ));
        }
    }

    UiPicker::new(
        "Workflows",
        "enter acts · type to filter · esc close",
        items,
        PickerAction::Workflow,
    )
    .with_layout(PickerLayout::Overlay)
    .with_overlay_chrome(OverlayChrome {
        nav_label: " WORKFLOWS".into(),
        detail_label: Some(" DETAILS".into()),
        nav_keys_hint: "↑↓ items".into(),
    })
    .with_confirm_verb("open")
}

fn outcome_tone(outcome: &str) -> PickerBadgeTone {
    match outcome {
        "success" => PickerBadgeTone::Healthy,
        "failed" | "denied" | "blocked" | "cancelled" => PickerBadgeTone::Warning,
        _ => PickerBadgeTone::Internal,
    }
}

fn status_overlay(run: &StoredRun) -> UiPicker {
    let outcome = derive_workflow_outcome(&run.graph, &run.state.state);
    let mut items = vec![item(
        Some("Summary"),
        run.graph.graph.name.to_string(),
        format!(
            "{} · {} · id {}",
            lifecycle_label(run.state.state.lifecycle),
            outcome_label(outcome),
            short_id(&run.manifest.run_id.to_string())
        ),
        "status:summary",
        Some((
            lifecycle_label(run.state.state.lifecycle).into(),
            lifecycle_tone(run.state.state.lifecycle),
        )),
        Some("close"),
    )];
    for (node_id, state) in &run.state.state.nodes {
        let label = node_state_label(state);
        items.push(item(
            Some("Steps"),
            node_id.to_string(),
            label.clone(),
            format!("status:node:{node_id}"),
            Some((label, PickerBadgeTone::Internal)),
            Some("close"),
        ));
    }
    UiPicker::new(
        format!("Status · {}", short_id(&run.manifest.run_id.to_string())),
        "enter or esc closes",
        items,
        PickerAction::Dismiss,
    )
    .with_layout(PickerLayout::Overlay)
    .with_overlay_chrome(OverlayChrome {
        nav_label: " STEPS".into(),
        detail_label: Some(" DETAIL".into()),
        nav_keys_hint: "↑↓ steps".into(),
    })
    .with_confirm_verb("close")
}

impl App {
    pub(super) async fn execute_workflow_command(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        match self.open_workflow_hub() {
            Ok(()) => Ok(()),
            Err(error) => {
                self.input_ui.set_composer(ComposerMode::Input);
                self.insert_entry(&Entry::Error(format!("could not open workflows: {error}")));
                self.status = "workflow hub failed".into();
                let _ = terminal;
                Ok(())
            }
        }
    }

    pub(super) fn open_workflow_hub(&mut self) -> anyhow::Result<()> {
        let ops = self.workflow_ops()?;
        let sources = workflow_discover::discover_workflow_sources(&self.info.runtime.cwd);
        let plans = ops.list_workspace_plans().unwrap_or_default();
        let runs = ops.list_workspace_runs().unwrap_or_default();
        if sources.is_empty() && plans.is_empty() && runs.is_empty() {
            self.input_ui.set_composer(ComposerMode::Input);
            self.insert_entry(&Entry::Notice(
                "No workflows yet. Add .rho/workflows/<name>/workflow.star, then run /workflow again."
                    .into(),
            ));
            self.status = "no workflows".into();
            return Ok(());
        }
        let picker = hub_picker(&sources, &plans, &runs);
        self.input_ui.set_composer(ComposerMode::Picker(picker));
        self.status = "workflows".into();
        Ok(())
    }

    pub(super) async fn submit_workflow_selection(
        &mut self,
        value: &str,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        if value.starts_with("noop:") {
            return Ok(());
        }
        match value {
            // Enter on a workflow starts it. No extra menu.
            value if let Some(path) = value.strip_prefix(SOURCE_PREFIX) => {
                self.start_workflow_source(path, terminal).await
            }
            // Enter on a saved plan runs it.
            value if let Some(id) = value.strip_prefix(PLAN_PREFIX) => {
                self.run_workflow_plan(id, terminal).await
            }
            // Enter on a run opens the live screen or finished status.
            value if let Some(id) = value.strip_prefix(RUN_PREFIX) => {
                self.open_workflow_run_primary(id, terminal).await
            }
            other => {
                self.insert_entry(&Entry::Error(format!(
                    "unknown workflow selection '{other}'"
                )));
                self.status = "workflow selection failed".into();
                Ok(())
            }
        }
    }

    fn workflow_ops(&self) -> anyhow::Result<WorkflowOps> {
        let path = self.info.services.config_repository.configured_path().ok();
        WorkflowOps::open(self.info.runtime.cwd.clone(), path)
    }

    async fn open_workflow_run_primary(
        &mut self,
        run_id: &str,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let parsed = RunId::from_str(run_id)?;
        let run = self.workflow_ops()?.load_run_id(parsed)?;
        match run.state.state.lifecycle {
            RunLifecycle::Completed => {
                self.open_child_picker(status_overlay(&run));
                self.status = "run status".into();
                Ok(())
            }
            RunLifecycle::NeedsRecovery => {
                self.resume_workflow_run(run_id, /*recover_uncertain*/ true, terminal)
                    .await
            }
            RunLifecycle::Planned | RunLifecycle::Running | RunLifecycle::Cancelling => {
                self.resume_workflow_run(run_id, /*recover_uncertain*/ false, terminal)
                    .await
            }
        }
    }

    async fn start_workflow_source(
        &mut self,
        relative_path: &str,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let absolute = self.info.runtime.cwd.join(relative_path);
        self.status = format!("starting {relative_path}");
        let ops = self.workflow_ops()?;
        let prepared = match self.prepare_source(&absolute).await {
            Ok(prepared) => prepared,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!(
                    "Could not start {relative_path}: {error:#}"
                )));
                self.status = "start failed".into();
                return Ok(());
            }
        };
        let plan = match ops.store_plan(&prepared) {
            Ok(plan) => plan,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("Could not save plan: {error:#}")));
                self.status = "start failed".into();
                return Ok(());
            }
        };
        let plan = match ops.prepare_run_id(plan.manifest.plan_id) {
            Ok(plan) => plan,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("Could not prepare run: {error:#}")));
                self.status = "start failed".into();
                return Ok(());
            }
        };
        let run = match ops.create_confirmed_run(&plan) {
            Ok(run) => run,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("Could not create run: {error:#}")));
                self.status = "start failed".into();
                return Ok(());
            }
        };
        let run_id = run.manifest.run_id;
        self.input_ui.set_composer(ComposerMode::Input);
        self.insert_entry(&Entry::Notice(format!(
            "Starting '{}' (run {}). Default inputs only.",
            plan.graph.graph.name,
            short_id(&run_id.to_string())
        )));
        self.launch_workflow_execution(
            run,
            RecoveryDecision::NormalResume,
            terminal,
            format!("workflow {} finished", short_id(&run_id.to_string())),
        )
        .await
    }

    async fn prepare_source(
        &self,
        absolute: &std::path::Path,
    ) -> anyhow::Result<workflow_cli::PreparedPlan> {
        let ops = self.workflow_ops()?;
        let config = self.info.services.config_repository.load()?;
        let limits = workflow_cli::planning_limits()?;
        let inputs: BTreeMap<_, WorkflowValue> = BTreeMap::new();
        ops.prepare_local(
            absolute,
            inputs,
            &config,
            &AgentCapabilities::all_host_tools(),
            &limits,
        )
        .await
    }

    async fn run_workflow_plan(
        &mut self,
        plan_id: &str,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let plan_id = PlanId::from_str(plan_id)?;
        let ops = self.workflow_ops()?;
        let plan = match ops.prepare_run_id(plan_id) {
            Ok(plan) => plan,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("Could not prepare plan: {error:#}")));
                self.status = "run failed".into();
                return Ok(());
            }
        };
        let run = match ops.create_confirmed_run(&plan) {
            Ok(run) => run,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("Could not create run: {error:#}")));
                self.status = "run failed".into();
                return Ok(());
            }
        };
        let run_id = run.manifest.run_id;
        self.input_ui.set_composer(ComposerMode::Input);
        self.launch_workflow_execution(
            run,
            RecoveryDecision::NormalResume,
            terminal,
            format!("workflow {} finished", short_id(&run_id.to_string())),
        )
        .await
    }

    async fn resume_workflow_run(
        &mut self,
        run_id: &str,
        recover_uncertain: bool,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let run_id = RunId::from_str(run_id)?;
        let ops = self.workflow_ops()?;
        let run = match ops.load_run_id(run_id) {
            Ok(run) => run,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("Could not load run: {error:#}")));
                self.status = "open failed".into();
                return Ok(());
            }
        };
        let recovery = match ops.prepare_resume(&run, recover_uncertain) {
            Ok(recovery) => recovery,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("Could not open run: {error:#}")));
                self.status = "open failed".into();
                return Ok(());
            }
        };
        self.input_ui.set_composer(ComposerMode::Input);
        self.launch_workflow_execution(
            run,
            recovery,
            terminal,
            format!("workflow {} finished", short_id(&run_id.to_string())),
        )
        .await
    }

    async fn launch_workflow_execution(
        &mut self,
        run: StoredRun,
        recovery: RecoveryDecision,
        terminal: &mut DefaultTerminal,
        success_status: String,
    ) -> anyhow::Result<()> {
        let run_id = run.manifest.run_id;
        let config_path = self.info.services.config_repository.configured_path().ok();
        let mut terminal_session = match self.terminal_session.take() {
            Some(session) => session,
            None => {
                self.insert_entry(&Entry::Error(
                    "Terminal session is unavailable for workflow execution.".into(),
                ));
                self.status = "workflow failed".into();
                return Ok(());
            }
        };
        let suspended = terminal_session
            .run_suspended(terminal, "Opening workflow…", || async move {
                workflow_cli::execute_run(run, recovery, None, config_path).await
            })
            .await;
        self.terminal_session = Some(terminal_session);

        if let Err(resume_error) = suspended.resume_result {
            self.insert_entry(&Entry::Error(format!(
                "Failed to return to chat after workflow: {resume_error:#}"
            )));
            if let Err(operation_error) = suspended.operation_result {
                self.insert_entry(&Entry::Error(format!(
                    "Workflow also failed: {operation_error:#}"
                )));
            }
            self.status = "workflow handoff failed".into();
            return Ok(());
        }
        self.ctrl_c_streak = 0;
        match suspended.operation_result {
            Ok(()) => {
                self.insert_entry(&Entry::Notice(format!(
                    "Returned from workflow run {}.",
                    short_id(&run_id.to_string())
                )));
                self.status = success_status;
            }
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("Workflow failed: {error:#}")));
                self.status = "workflow failed".into();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "workflow_hub_tests.rs"]
mod tests;

#[cfg(test)]
fn test_source(label: &str, relative: &str) -> workflow_discover::DiscoveredWorkflow {
    workflow_discover::DiscoveredWorkflow {
        relative_path: relative.into(),
        absolute_path: std::path::PathBuf::from(relative),
        label: label.into(),
    }
}
