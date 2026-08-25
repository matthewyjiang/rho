//! In-app `/workflow` hub: start workflows and check runs from the chat TUI.

use std::{cmp::Reverse, collections::BTreeMap, str::FromStr};

use ratatui::DefaultTerminal;

use super::{
    picker_overlay::OverlayChrome, workflow_discover, App, ComposerMode, Entry, InlineChoice,
    InlineChoiceModal, InlineChoiceOption, InlineChoicePending, PickerAction, PickerBadge,
    PickerBadgeTone, PickerItem, PickerLayout, UiPicker,
};
use crate::{
    agent::AgentCapabilities,
    app::{
        workflow_cli::{self, WorkflowOps},
        workflow_runtime::RecoveryDecision,
    },
    workflow::{
        PlanId, PlanInventoryItem, RunId, RunInventoryItem, RunLifecycle, StoredRun,
        WorkflowOutcome, WorkflowValue,
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

fn run_progress(done: usize, total: usize) -> String {
    let total = total.max(1);
    format!("{done}/{total} steps done")
}

/// Root list: start workflows, open runs, or reuse a saved plan.
pub(super) fn hub_picker(
    sources: &[workflow_discover::DiscoveredWorkflow],
    plans: &[PlanInventoryItem],
    runs: &[RunInventoryItem],
) -> UiPicker {
    let mut items = Vec::new();

    if sources.is_empty() {
        items.push(item(
            Some("START"),
            "No workflows yet",
            "Ask Rho to create your first workflow.",
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
        .filter(|run| run.lifecycle != RunLifecycle::Completed)
        .collect::<Vec<_>>();
    active.sort_by_key(|run| Reverse((run.created_at_unix_nanos, run.run_id)));

    let mut finished = runs
        .iter()
        .filter(|run| run.lifecycle == RunLifecycle::Completed)
        .collect::<Vec<_>>();
    finished.sort_by_key(|run| Reverse((run.created_at_unix_nanos, run.run_id)));
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
            let id = run.run_id.to_string();
            let short = short_id(&id);
            let life = lifecycle_label(run.lifecycle);
            let name = run.name.as_str();
            items.push(item(
                Some("RUNS"),
                format!("Watch  {life}  ·  {short}"),
                format!(
                    "{name}\n{life} · {}\nEnter opens the DAG watch screen. Press d to delete.\nRun id {short}",
                    run_progress(run.done_steps, run.total_steps)
                ),
                format!("{RUN_PREFIX}{id}"),
                Some((life.into(), lifecycle_tone(run.lifecycle))),
                Some("watch"),
            ));
        }
        for run in finished {
            let id = run.run_id.to_string();
            let short = short_id(&id);
            let outcome = outcome_label(run.outcome);
            let name = run.name.as_str();
            let tone = outcome_tone(run.outcome);
            items.push(item(
                Some("RUNS"),
                format!("Watch  {outcome}  ·  {short}"),
                format!(
                    "{name}\nFinished · {outcome} · {}\nEnter opens the DAG watch screen. Press d to delete.\nRun id {short}",
                    run_progress(run.done_steps, run.total_steps)
                ),
                format!("{RUN_PREFIX}{id}"),
                Some((outcome, tone)),
                Some("watch"),
            ));
        }
    }

    if plans.is_empty() {
        // Keep the list focused; empty plans stay hidden.
    } else {
        for plan in plans {
            let id = plan.plan_id.to_string();
            let short = short_id(&id);
            let name = plan.name.as_str();
            let steps = plan.step_count;
            items.push(item(
                Some("SAVED PLANS"),
                format!("Run plan  ·  {short}"),
                format!(
                    "{name}\n{steps} steps already frozen.\nEnter starts a new run. Press d to delete this plan.\nPlan id {short}\nRuns that already used this plan keep their own copy."
                ),
                format!("{PLAN_PREFIX}{id}"),
                Some(("saved".into(), PickerBadgeTone::Internal)),
                Some("run"),
            ));
        }
    }

    UiPicker::new("Workflows", items, PickerAction::Workflow)
        .with_key_hints(super::PickerKeyHints {
            tab_complete: false,
            row_delete: true,
            ..Default::default()
        })
        .with_layout(PickerLayout::Overlay)
        .with_overlay_chrome(OverlayChrome {
            nav_label: " WORKFLOWS".into(),
            detail_label: Some(" DETAILS".into()),
            nav_keys_hint: "↑↓ items".into(),
        })
        .with_confirm_verb("open")
}

fn outcome_tone(outcome: Option<WorkflowOutcome>) -> PickerBadgeTone {
    match outcome {
        Some(WorkflowOutcome::Success) => PickerBadgeTone::Healthy,
        Some(
            WorkflowOutcome::Failure
            | WorkflowOutcome::Denial
            | WorkflowOutcome::Cancellation
            | WorkflowOutcome::Blocked,
        ) => PickerBadgeTone::Warning,
        None => PickerBadgeTone::Internal,
    }
}

impl App {
    pub(super) async fn execute_workflow_command(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        self.open_workflow_hub_or_report();
        let _ = terminal;
        Ok(())
    }

    pub(super) fn open_workflow_hub_or_report(&mut self) {
        if let Err(error) = self.open_workflow_hub() {
            self.input_ui.set_composer(ComposerMode::Input);
            self.insert_entry(&Entry::Error(format!("could not open workflows: {error}")));
            self.set_status("workflow hub failed");
        }
    }

    pub(super) fn open_workflow_hub(&mut self) -> anyhow::Result<()> {
        let ops = self.workflow_ops()?;
        let sources = workflow_discover::discover_workflow_sources(&self.info.runtime.cwd);
        let plans = ops.list_workspace_plans()?;
        let runs = ops.list_workspace_runs()?;
        let picker = hub_picker(&sources, &plans, &runs);
        self.input_ui.set_composer(ComposerMode::Picker(picker));
        self.set_status("workflows");
        Ok(())
    }

    pub(super) fn prompt_delete_selected_workflow_item(&mut self) -> anyhow::Result<()> {
        let Some(value) = self.selected_workflow_value() else {
            return Ok(());
        };
        if let Some(plan_id) = value.strip_prefix(PLAN_PREFIX) {
            let short = short_id(plan_id);
            let choice = InlineChoice::new(
                format!("Delete plan {short}?"),
                "Removes this saved plan. Existing runs keep their own graph copy and still open.",
                vec![
                    InlineChoiceOption::available(
                        "delete",
                        'd',
                        "Delete",
                        "Permanently remove this plan",
                    ),
                    InlineChoiceOption::available(
                        "cancel",
                        'c',
                        "Cancel",
                        "Keep the plan and return to workflows",
                    )
                    .with_alternate_shortcut('n'),
                ],
            )?;
            self.input_ui
                .set_composer(ComposerMode::InlineChoice(InlineChoiceModal {
                    choice,
                    pending: InlineChoicePending::DeleteWorkflowPlan {
                        plan_id: plan_id.to_owned(),
                    },
                    parent_picker: None,
                }));
            self.set_status("confirm delete plan");
            return Ok(());
        }
        if let Some(run_id) = value.strip_prefix(RUN_PREFIX) {
            let short = short_id(run_id);
            let choice = InlineChoice::new(
                format!("Delete run {short}?"),
                "Removes this run's durable status and artifacts. Active runs must be stopped first.",
                vec![
                    InlineChoiceOption::available(
                        "delete",
                        'd',
                        "Delete",
                        "Permanently remove this run",
                    ),
                    InlineChoiceOption::available(
                        "cancel",
                        'c',
                        "Cancel",
                        "Keep the run and return to workflows",
                    )
                    .with_alternate_shortcut('n'),
                ],
            )?;
            self.input_ui
                .set_composer(ComposerMode::InlineChoice(InlineChoiceModal {
                    choice,
                    pending: InlineChoicePending::DeleteWorkflowRun {
                        run_id: run_id.to_owned(),
                    },
                    parent_picker: None,
                }));
            self.set_status("confirm delete run");
            return Ok(());
        }
        self.set_status(
            "Only saved plans and runs can be deleted here. Local workflow files stay on disk.",
        );
        Ok(())
    }

    pub(super) fn submit_delete_workflow_plan_choice(
        &mut self,
        value: &str,
        plan_id: &str,
    ) -> anyhow::Result<()> {
        if value != "delete" {
            return self.open_workflow_hub();
        }
        let short = short_id(plan_id);
        let parsed = PlanId::from_str(plan_id)?;
        let status = match self.workflow_ops()?.delete_workspace_plan(parsed) {
            Ok(()) => format!("deleted plan {short}"),
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("could not delete plan: {error:#}")));
                "delete failed".into()
            }
        };
        self.open_workflow_hub()?;
        self.set_status(status);
        Ok(())
    }

    pub(super) fn submit_delete_workflow_run_choice(
        &mut self,
        value: &str,
        run_id: &str,
    ) -> anyhow::Result<()> {
        if value != "delete" {
            return self.open_workflow_hub();
        }
        let short = short_id(run_id);
        let parsed = RunId::from_str(run_id)?;
        let status = match self.workflow_ops()?.delete_workspace_run(parsed) {
            Ok(()) => format!("deleted run {short}"),
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("could not delete run: {error:#}")));
                "delete failed".into()
            }
        };
        self.open_workflow_hub()?;
        self.set_status(status);
        Ok(())
    }

    fn selected_workflow_value(&self) -> Option<String> {
        match self.input_ui.composer() {
            ComposerMode::Picker(picker) if picker.action == PickerAction::Workflow => {
                picker.selected_item().map(|item| item.value.clone())
            }
            _ => None,
        }
    }

    pub(super) async fn submit_workflow_selection(
        &mut self,
        value: &str,
        terminal: &mut DefaultTerminal,
        agent: &mut super::InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if value.starts_with("noop:") {
            return Ok(());
        }
        match value {
            // Enter on a workflow starts it. No extra menu.
            value if value.starts_with(SOURCE_PREFIX) => {
                let path = value
                    .strip_prefix(SOURCE_PREFIX)
                    .expect("prefix checked above");
                self.start_workflow_source(path, terminal, agent).await
            }
            // Enter on a saved plan runs it.
            value if value.starts_with(PLAN_PREFIX) => {
                let id = value
                    .strip_prefix(PLAN_PREFIX)
                    .expect("prefix checked above");
                self.run_workflow_plan(id, terminal, agent).await
            }
            // Enter on a run opens the live screen or finished status.
            value if value.starts_with(RUN_PREFIX) => {
                let id = value
                    .strip_prefix(RUN_PREFIX)
                    .expect("prefix checked above");
                self.open_workflow_run_primary(id, terminal, agent).await
            }
            other => {
                self.insert_entry(&Entry::Error(format!(
                    "unknown workflow selection '{other}'"
                )));
                self.set_status("workflow selection failed");
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
        agent: &mut super::InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let parsed = RunId::from_str(run_id)?;
        let run = self.workflow_ops()?.load_run_id(parsed)?;
        match run.state.state.lifecycle {
            RunLifecycle::NeedsRecovery => {
                // Recover in the background, then open the watch screen.
                self.resume_workflow_run(run_id, /*recover_uncertain*/ true, terminal, agent)
                    .await?;
                let run = self.workflow_ops()?.load_run_id(parsed)?;
                self.open_workflow_watch(run, terminal).await
            }
            RunLifecycle::Planned
            | RunLifecycle::Running
            | RunLifecycle::Cancelling
            | RunLifecycle::Completed => self.open_workflow_watch(run, terminal).await,
        }
    }

    async fn open_workflow_watch(
        &mut self,
        run: StoredRun,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let run_id = run.manifest.run_id;
        self.input_ui.set_composer(ComposerMode::Input);
        let mut terminal_session = match self.terminal_session.take() {
            Some(session) => session,
            None => {
                self.insert_entry(&Entry::Error(
                    "terminal session is unavailable for workflow watch".into(),
                ));
                self.set_status("watch failed");
                return Ok(());
            }
        };
        let suspended = terminal_session
            .run_suspended(terminal, "Opening workflow watch…", || async move {
                workflow_cli::watch_run(run).await
            })
            .await;
        self.terminal_session = Some(terminal_session);

        if let Err(resume_error) = suspended.resume_result {
            self.insert_entry(&Entry::Error(format!(
                "could not return to chat after workflow watch: {resume_error:#}"
            )));
            if let Err(operation_error) = suspended.operation_result {
                self.insert_entry(&Entry::Error(format!(
                    "could not watch workflow: {operation_error:#}"
                )));
            }
            self.set_status("watch handoff failed");
            return Ok(());
        }
        self.ctrl_c_streak = 0;
        match suspended.operation_result {
            Ok(()) => {
                self.set_status(format!(
                    "left watch for run {}",
                    short_id(&run_id.to_string())
                ));
            }
            Err(error) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not watch workflow: {error:#}"
                )));
                self.set_status("watch failed");
            }
        }
        Ok(())
    }

    async fn start_workflow_source(
        &mut self,
        relative_path: &str,
        _terminal: &mut DefaultTerminal,
        agent: &mut super::InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let absolute = self.info.runtime.cwd.join(relative_path);
        self.set_status(format!("starting {relative_path}"));
        let ops = self.workflow_ops()?;
        let available_tools = agent.workflow_host_capabilities();
        let prepared = match self.prepare_source(&absolute, &available_tools).await {
            Ok(prepared) => prepared,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not start {relative_path}: {error:#}"
                )));
                self.set_status("start failed");
                return Ok(());
            }
        };
        let plan = match ops.store_plan(&prepared) {
            Ok(plan) => plan,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("could not save plan: {error:#}")));
                self.set_status("start failed");
                return Ok(());
            }
        };
        let plan = match ops.prepare_run_id(plan.manifest.plan_id) {
            Ok(plan) => plan,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("could not prepare run: {error:#}")));
                self.set_status("start failed");
                return Ok(());
            }
        };
        let run = match ops.create_confirmed_run(&plan) {
            Ok(run) => run,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("could not create run: {error:#}")));
                self.set_status("start failed");
                return Ok(());
            }
        };
        let run_id = run.manifest.run_id;
        self.input_ui.set_composer(ComposerMode::Input);
        self.insert_entry(&Entry::Notice(format!(
            "Starting '{}' in the background (run {}). Default inputs only. Completion is delivered automatically.",
            plan.graph.graph.name,
            short_id(&run_id.to_string())
        )));
        self.launch_workflow_execution(run, RecoveryDecision::NormalResume, agent)
            .await
    }

    async fn prepare_source(
        &self,
        absolute: &std::path::Path,
        available_tools: &AgentCapabilities,
    ) -> anyhow::Result<workflow_cli::PreparedPlan> {
        let ops = self.workflow_ops()?;
        let config = self.info.services.config_repository.load()?;
        let limits = workflow_cli::planning_limits()?;
        let inputs: BTreeMap<_, WorkflowValue> = BTreeMap::new();
        ops.prepare_local(absolute, inputs, &config, available_tools, &limits)
            .await
    }

    async fn run_workflow_plan(
        &mut self,
        plan_id: &str,
        _terminal: &mut DefaultTerminal,
        agent: &mut super::InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let plan_id = PlanId::from_str(plan_id)?;
        let ops = self.workflow_ops()?;
        let plan = match ops.prepare_run_id(plan_id) {
            Ok(plan) => plan,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("could not prepare plan: {error:#}")));
                self.set_status("run failed");
                return Ok(());
            }
        };
        let run = match ops.create_confirmed_run(&plan) {
            Ok(run) => run,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("could not create run: {error:#}")));
                self.set_status("run failed");
                return Ok(());
            }
        };
        let run_id = run.manifest.run_id;
        self.input_ui.set_composer(ComposerMode::Input);
        self.insert_entry(&Entry::Notice(format!(
            "Starting plan {} in the background (run {}). Completion is delivered automatically.",
            short_id(&plan_id.to_string()),
            short_id(&run_id.to_string())
        )));
        self.launch_workflow_execution(run, RecoveryDecision::NormalResume, agent)
            .await
    }

    async fn resume_workflow_run(
        &mut self,
        run_id: &str,
        recover_uncertain: bool,
        _terminal: &mut DefaultTerminal,
        agent: &mut super::InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let run_id = RunId::from_str(run_id)?;
        let ops = self.workflow_ops()?;
        let run = match ops.load_run_id(run_id) {
            Ok(run) => run,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("could not load run: {error:#}")));
                self.set_status("open failed");
                return Ok(());
            }
        };
        let recovery = match ops.prepare_resume(&run, recover_uncertain) {
            Ok(recovery) => recovery,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("could not open run: {error:#}")));
                self.set_status("open failed");
                return Ok(());
            }
        };
        self.input_ui.set_composer(ComposerMode::Input);
        self.insert_entry(&Entry::Notice(format!(
            "Resuming run {} in the background. Completion is delivered automatically.",
            short_id(&run_id.to_string())
        )));
        self.launch_workflow_execution(run, recovery, agent).await
    }

    async fn launch_workflow_execution(
        &mut self,
        run: StoredRun,
        recovery: RecoveryDecision,
        agent: &mut super::InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let run_id = run.manifest.run_id;
        let workflow_name = run.graph.graph.name.as_str().to_owned();
        let graph_digest = run.manifest.graph_digest.0.clone();
        let config_path = self.info.services.config_repository.configured_path().ok();
        // Background runs keep the chat TUI, so workflow approvals are headless.
        let tracker = agent.workflow_tracker().clone();
        tracker.register_start(
            run_id.to_string(),
            workflow_name.clone(),
            graph_digest.clone(),
            Some(agent.session_id().to_string()),
        );
        match workflow_cli::spawn_background_run(run, recovery, config_path, Some(tracker)).await {
            Ok(_) => {
                let (model, display) = crate::tools::workflow_tracker::start_context_prompts(
                    &run_id.to_string(),
                    &workflow_name,
                    &graph_digest,
                );
                if let Err(error) = agent.append_user_context_with_display(model, display.clone()) {
                    self.insert_entry(&Entry::Error(format!(
                        "workflow started, but could not add run id to context: {error:#}"
                    )));
                } else {
                    self.insert_entry(&Entry::Notice(display));
                }
                self.set_status(format!(
                    "workflow run {} is running in the background",
                    short_id(&run_id.to_string())
                ));
            }
            Err(error) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not start workflow in the background: {error:#}"
                )));
                self.set_status("workflow failed");
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
