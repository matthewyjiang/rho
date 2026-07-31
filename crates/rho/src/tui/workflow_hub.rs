//! In-app `/workflow` hub: browse sources, plans, and runs from the chat TUI.

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
        derive_workflow_outcome, PlanId, RunId, RunLifecycle, StoredPlan, StoredRun, WorkflowValue,
    },
};

const HUB_SOURCES: &str = "hub:sources";
const HUB_PLANS: &str = "hub:plans";
const HUB_RUNS: &str = "hub:runs";

const SOURCE_PREFIX: &str = "source:";
const SOURCE_VALIDATE_PREFIX: &str = "source_validate:";
const SOURCE_PLAN_PREFIX: &str = "source_plan:";
const PLAN_PREFIX: &str = "plan:";
const PLAN_RUN_PREFIX: &str = "plan_run:";
const PLAN_DETAIL_PREFIX: &str = "plan_detail:";
const RUN_PREFIX: &str = "run:";
const RUN_STATUS_PREFIX: &str = "run_status:";
const RUN_CANCEL_PREFIX: &str = "run_cancel:";
const RUN_RESUME_PREFIX: &str = "run_resume:";
const RUN_RECOVER_PREFIX: &str = "run_recover:";

fn badge(text: impl Into<String>) -> PickerBadge {
    PickerBadge {
        text: text.into(),
        tone: PickerBadgeTone::Selected,
    }
}

fn item(
    label: impl Into<String>,
    detail: impl Into<String>,
    value: impl Into<String>,
    badge_text: Option<String>,
) -> PickerItem {
    PickerItem {
        section: None,
        label: label.into(),
        detail: Some(detail.into()),
        preview: None,
        badge: badge_text.map(badge),
        value: value.into(),
        selection_verb: None,
    }
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn short_digest(digest: &str) -> String {
    let body = digest.strip_prefix("sha256:").unwrap_or(digest);
    body.chars().take(12).collect()
}

pub(super) fn hub_picker(source_count: usize, plan_count: usize, run_count: usize) -> UiPicker {
    UiPicker::new(
        "workflows",
        "browse sources, plans, and runs · type filter · esc close",
        vec![
            item(
                "Sources",
                format!(
                    "{source_count} workflow entr{}",
                    if source_count == 1 { "y" } else { "ies" }
                ),
                HUB_SOURCES,
                Some(source_count.to_string()),
            ),
            item(
                "Plans",
                format!(
                    "{plan_count} frozen plan{}",
                    if plan_count == 1 { "" } else { "s" }
                ),
                HUB_PLANS,
                Some(plan_count.to_string()),
            ),
            item(
                "Runs",
                format!(
                    "{run_count} durable run{}",
                    if run_count == 1 { "" } else { "s" }
                ),
                HUB_RUNS,
                Some(run_count.to_string()),
            ),
        ],
        PickerAction::Workflow,
    )
    .with_confirm_verb("open")
}

fn sources_picker(sources: Vec<workflow_discover::DiscoveredWorkflow>) -> UiPicker {
    let items = sources
        .into_iter()
        .map(|source| {
            item(
                source.label,
                source.relative_path.clone(),
                format!("{SOURCE_PREFIX}{}", source.relative_path),
                None,
            )
        })
        .collect::<Vec<_>>();
    UiPicker::new(
        "workflow sources",
        "enter opens actions · esc back",
        items,
        PickerAction::Workflow,
    )
    .with_confirm_verb("open")
}

fn source_actions_picker(relative_path: &str) -> UiPicker {
    UiPicker::new(
        format!("source {relative_path}"),
        "validate or plan with default inputs · esc back",
        vec![
            item(
                "Validate",
                "Check the graph without storing a plan",
                format!("{SOURCE_VALIDATE_PREFIX}{relative_path}"),
                None,
            ),
            item(
                "Plan",
                "Freeze a plan using default inputs only",
                format!("{SOURCE_PLAN_PREFIX}{relative_path}"),
                None,
            ),
        ],
        PickerAction::Workflow,
    )
    .with_confirm_verb("run")
}

fn plans_picker(plans: Vec<StoredPlan>) -> UiPicker {
    let items = plans
        .into_iter()
        .map(|plan| {
            let id = plan.manifest.plan_id.to_string();
            item(
                plan.graph.graph.name.to_string(),
                format!(
                    "plan {} · {} nodes · {}",
                    short_id(&id),
                    plan.graph.graph.nodes.len(),
                    short_digest(&plan.manifest.graph_digest.0)
                ),
                format!("{PLAN_PREFIX}{id}"),
                Some(short_id(&id)),
            )
        })
        .collect::<Vec<_>>();
    UiPicker::new(
        "workflow plans",
        "enter opens actions · esc back",
        items,
        PickerAction::Workflow,
    )
    .with_confirm_verb("open")
}

fn plan_actions_picker(plan: &StoredPlan) -> UiPicker {
    let id = plan.manifest.plan_id.to_string();
    UiPicker::new(
        format!("plan {}", short_id(&id)),
        "run opens the workflow screen · esc back",
        vec![
            item(
                "Run",
                format!(
                    "Start a run of {} ({})",
                    plan.graph.graph.name,
                    short_digest(&plan.manifest.graph_digest.0)
                ),
                format!("{PLAN_RUN_PREFIX}{id}"),
                None,
            ),
            item(
                "Details",
                format!(
                    "{} nodes · digest {}",
                    plan.graph.graph.nodes.len(),
                    short_digest(&plan.manifest.graph_digest.0)
                ),
                format!("{PLAN_DETAIL_PREFIX}{id}"),
                None,
            ),
        ],
        PickerAction::Workflow,
    )
    .with_confirm_verb("select")
}

fn runs_picker(runs: Vec<StoredRun>) -> UiPicker {
    let items = runs
        .into_iter()
        .map(|run| {
            let id = run.manifest.run_id.to_string();
            let lifecycle = format!("{:?}", run.state.state.lifecycle).to_ascii_lowercase();
            item(
                run.graph.graph.name.to_string(),
                format!(
                    "run {} · {lifecycle} · plan {}",
                    short_id(&id),
                    short_id(&run.manifest.plan_id.to_string())
                ),
                format!("{RUN_PREFIX}{id}"),
                Some(lifecycle),
            )
        })
        .collect::<Vec<_>>();
    UiPicker::new(
        "workflow runs",
        "enter opens actions · esc back",
        items,
        PickerAction::Workflow,
    )
    .with_confirm_verb("open")
}

fn run_actions_picker(run: &StoredRun) -> UiPicker {
    let id = run.manifest.run_id.to_string();
    let lifecycle = run.state.state.lifecycle;
    let mut items = vec![item(
        "Status",
        "Show node states and outcome",
        format!("{RUN_STATUS_PREFIX}{id}"),
        None,
    )];
    if matches!(
        lifecycle,
        RunLifecycle::Running | RunLifecycle::Cancelling | RunLifecycle::Planned
    ) {
        items.push(item(
            "Cancel",
            "Request cancellation of active work",
            format!("{RUN_CANCEL_PREFIX}{id}"),
            None,
        ));
    }
    match lifecycle {
        RunLifecycle::NeedsRecovery => {
            items.push(item(
                "Recover and resume",
                "Confirm no prior process remains, then resume",
                format!("{RUN_RECOVER_PREFIX}{id}"),
                None,
            ));
        }
        RunLifecycle::Completed => {}
        RunLifecycle::Running | RunLifecycle::Cancelling | RunLifecycle::Planned => {
            items.push(item(
                "Resume",
                "Resume from the frozen run graph",
                format!("{RUN_RESUME_PREFIX}{id}"),
                None,
            ));
        }
    }
    UiPicker::new(
        format!("run {}", short_id(&id)),
        "status, cancel, or resume · esc back",
        items,
        PickerAction::Workflow,
    )
    .with_confirm_verb("select")
}

fn status_overlay(run: &StoredRun) -> UiPicker {
    let outcome = derive_workflow_outcome(&run.graph, &run.state.state);
    let mut items = vec![item(
        "Summary",
        format!(
            "lifecycle {:?} · outcome {} · plan {} · digest {}",
            run.state.state.lifecycle,
            outcome
                .map(|value| format!("{value:?}"))
                .unwrap_or_else(|| "pending".into()),
            short_id(&run.manifest.plan_id.to_string()),
            short_digest(&run.manifest.graph_digest.0)
        ),
        "status:summary",
        None,
    )];
    for (node_id, state) in &run.state.state.nodes {
        items.push(item(
            node_id.to_string(),
            format!("{state:?}"),
            format!("status:node:{node_id}"),
            Some(format!("{state:?}")),
        ));
    }
    UiPicker::new(
        format!("run {} status", short_id(&run.manifest.run_id.to_string())),
        "enter or esc closes",
        items,
        PickerAction::Dismiss,
    )
    .with_layout(PickerLayout::Overlay)
    .with_overlay_chrome(OverlayChrome {
        nav_label: " NODES".into(),
        detail_label: Some(" STATE".into()),
        nav_keys_hint: "↑↓ nodes".into(),
    })
    .with_confirm_verb("close")
}

fn plan_detail_overlay(plan: &StoredPlan) -> UiPicker {
    let mut items = vec![item(
        "Summary",
        format!(
            "plan {} · {} nodes · digest {}",
            plan.manifest.plan_id,
            plan.graph.graph.nodes.len(),
            short_digest(&plan.manifest.graph_digest.0)
        ),
        "plan_detail:summary",
        None,
    )];
    for node in plan.graph.graph.nodes.values() {
        let kind = match &node.execution {
            crate::workflow::NodeExecution::Agent(_) => "agent",
            crate::workflow::NodeExecution::Command(_) => "command",
        };
        items.push(item(
            node.id.to_string(),
            format!("{kind} · {}", node.display_name),
            format!("plan_detail:node:{}", node.id),
            None,
        ));
    }
    UiPicker::new(
        format!(
            "plan {} detail",
            short_id(&plan.manifest.plan_id.to_string())
        ),
        "enter or esc closes",
        items,
        PickerAction::Dismiss,
    )
    .with_layout(PickerLayout::Overlay)
    .with_overlay_chrome(OverlayChrome {
        nav_label: " NODES".into(),
        detail_label: Some(" DETAIL".into()),
        nav_keys_hint: "↑↓ nodes".into(),
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
        let picker = hub_picker(sources.len(), plans.len(), runs.len());
        self.input_ui.set_composer(ComposerMode::Picker(picker));
        self.status = "workflows".into();
        Ok(())
    }

    pub(super) async fn submit_workflow_selection(
        &mut self,
        value: &str,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        match value {
            HUB_SOURCES => self.open_workflow_sources_picker(),
            HUB_PLANS => self.open_workflow_plans_picker(),
            HUB_RUNS => self.open_workflow_runs_picker(),
            value if let Some(path) = value.strip_prefix(SOURCE_VALIDATE_PREFIX) => {
                self.validate_workflow_source(path).await
            }
            value if let Some(path) = value.strip_prefix(SOURCE_PLAN_PREFIX) => {
                self.plan_workflow_source(path).await
            }
            value if let Some(path) = value.strip_prefix(SOURCE_PREFIX) => {
                self.open_child_picker(source_actions_picker(path));
                self.status = format!("source {path}");
                Ok(())
            }
            value if let Some(id) = value.strip_prefix(PLAN_RUN_PREFIX) => {
                self.run_workflow_plan(id, terminal).await
            }
            value if let Some(id) = value.strip_prefix(PLAN_DETAIL_PREFIX) => {
                self.open_workflow_plan_detail(id)
            }
            value if let Some(id) = value.strip_prefix(PLAN_PREFIX) => {
                self.open_workflow_plan_actions(id)
            }
            value if let Some(id) = value.strip_prefix(RUN_STATUS_PREFIX) => {
                self.open_workflow_run_status(id)
            }
            value if let Some(id) = value.strip_prefix(RUN_CANCEL_PREFIX) => {
                self.cancel_workflow_run(id).await
            }
            value if let Some(id) = value.strip_prefix(RUN_RESUME_PREFIX) => {
                self.resume_workflow_run(id, /*recover_uncertain*/ false, terminal)
                    .await
            }
            value if let Some(id) = value.strip_prefix(RUN_RECOVER_PREFIX) => {
                self.resume_workflow_run(id, /*recover_uncertain*/ true, terminal)
                    .await
            }
            value if let Some(id) = value.strip_prefix(RUN_PREFIX) => {
                self.open_workflow_run_actions(id)
            }
            other => {
                self.insert_entry(&Entry::Error(format!(
                    "unknown workflow hub selection '{other}'"
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

    fn open_workflow_sources_picker(&mut self) -> anyhow::Result<()> {
        let sources = workflow_discover::discover_workflow_sources(&self.info.runtime.cwd);
        if sources.is_empty() {
            self.insert_entry(&Entry::Notice(
                "no workflow sources under .rho/workflows (add folder/workflow.star or a .star file)"
                    .into(),
            ));
            self.status = "no workflow sources".into();
            return Ok(());
        }
        self.open_child_picker(sources_picker(sources));
        self.status = "workflow sources".into();
        Ok(())
    }

    fn open_workflow_plans_picker(&mut self) -> anyhow::Result<()> {
        let plans = self.workflow_ops()?.list_workspace_plans()?;
        if plans.is_empty() {
            self.insert_entry(&Entry::Notice(
                "no frozen plans for this workspace yet; plan a source first".into(),
            ));
            self.status = "no workflow plans".into();
            return Ok(());
        }
        self.open_child_picker(plans_picker(plans));
        self.status = "workflow plans".into();
        Ok(())
    }

    fn open_workflow_runs_picker(&mut self) -> anyhow::Result<()> {
        let runs = self.workflow_ops()?.list_workspace_runs()?;
        if runs.is_empty() {
            self.insert_entry(&Entry::Notice(
                "no durable runs for this workspace yet".into(),
            ));
            self.status = "no workflow runs".into();
            return Ok(());
        }
        self.open_child_picker(runs_picker(runs));
        self.status = "workflow runs".into();
        Ok(())
    }

    fn open_workflow_plan_actions(&mut self, plan_id: &str) -> anyhow::Result<()> {
        let plan_id = PlanId::from_str(plan_id)?;
        let plan = self.workflow_ops()?.load_plan_id(plan_id)?;
        self.open_child_picker(plan_actions_picker(&plan));
        self.status = format!("plan {}", short_id(&plan_id.to_string()));
        Ok(())
    }

    fn open_workflow_plan_detail(&mut self, plan_id: &str) -> anyhow::Result<()> {
        let plan_id = PlanId::from_str(plan_id)?;
        let plan = self.workflow_ops()?.load_plan_id(plan_id)?;
        self.open_child_picker(plan_detail_overlay(&plan));
        self.status = "plan detail".into();
        Ok(())
    }

    fn open_workflow_run_actions(&mut self, run_id: &str) -> anyhow::Result<()> {
        let run_id = RunId::from_str(run_id)?;
        let run = self.workflow_ops()?.load_run_id(run_id)?;
        self.open_child_picker(run_actions_picker(&run));
        self.status = format!("run {}", short_id(&run_id.to_string()));
        Ok(())
    }

    fn open_workflow_run_status(&mut self, run_id: &str) -> anyhow::Result<()> {
        let run_id = RunId::from_str(run_id)?;
        let run = self.workflow_ops()?.load_run_id(run_id)?;
        self.open_child_picker(status_overlay(&run));
        self.status = "run status".into();
        Ok(())
    }

    async fn validate_workflow_source(&mut self, relative_path: &str) -> anyhow::Result<()> {
        let absolute = self.info.runtime.cwd.join(relative_path);
        self.status = format!("validating {relative_path}");
        match self.prepare_source(&absolute).await {
            Ok(prepared) => {
                self.insert_entry(&Entry::Notice(format!(
                    "valid workflow '{}' · {} nodes · {}",
                    prepared.workflow.graph.name,
                    prepared.workflow.graph.nodes.len(),
                    relative_path
                )));
                self.status = "workflow valid".into();
            }
            Err(error) => {
                self.insert_entry(&Entry::Error(format!(
                    "workflow validate failed for {relative_path}: {error:#}"
                )));
                self.status = "validate failed".into();
            }
        }
        Ok(())
    }

    async fn plan_workflow_source(&mut self, relative_path: &str) -> anyhow::Result<()> {
        let absolute = self.info.runtime.cwd.join(relative_path);
        self.status = format!("planning {relative_path}");
        let ops = self.workflow_ops()?;
        match self.prepare_source(&absolute).await {
            Ok(prepared) => match ops.store_plan(&prepared) {
                Ok(plan) => {
                    self.insert_entry(&Entry::Notice(format!(
                        "planned '{}' as {} · digest {} · defaults only (use CLI --input for custom values)",
                        plan.graph.graph.name,
                        plan.manifest.plan_id,
                        short_digest(&plan.manifest.graph_digest.0)
                    )));
                    self.status = "workflow planned".into();
                }
                Err(error) => {
                    self.insert_entry(&Entry::Error(format!(
                        "workflow plan store failed: {error:#}"
                    )));
                    self.status = "plan failed".into();
                }
            },
            Err(error) => {
                self.insert_entry(&Entry::Error(format!(
                    "workflow plan failed for {relative_path}: {error:#}"
                )));
                self.status = "plan failed".into();
            }
        }
        Ok(())
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
                self.insert_entry(&Entry::Error(format!("could not prepare plan: {error:#}")));
                self.status = "run failed".into();
                return Ok(());
            }
        };
        let run = match ops.create_confirmed_run(&plan) {
            Ok(run) => run,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("could not create run: {error:#}")));
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
            format!("workflow run {run_id} finished"),
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
                self.insert_entry(&Entry::Error(format!("could not load run: {error:#}")));
                self.status = "resume failed".into();
                return Ok(());
            }
        };
        let recovery = match ops.prepare_resume(&run, recover_uncertain) {
            Ok(recovery) => recovery,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("could not resume run: {error:#}")));
                self.status = "resume failed".into();
                return Ok(());
            }
        };
        self.input_ui.set_composer(ComposerMode::Input);
        self.launch_workflow_execution(
            run,
            recovery,
            terminal,
            format!("workflow resume {run_id} finished"),
        )
        .await
    }

    async fn cancel_workflow_run(&mut self, run_id: &str) -> anyhow::Result<()> {
        let run_id = RunId::from_str(run_id)?;
        let ops = self.workflow_ops()?;
        let run = match ops.load_run_id(run_id) {
            Ok(run) => run,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("could not load run: {error:#}")));
                self.status = "cancel failed".into();
                return Ok(());
            }
        };
        match ops.cancel(run_id, run.state.state.lifecycle).await {
            Ok(outcome) => {
                self.insert_entry(&Entry::Notice(format!(
                    "cancel run {} · state {:?} · lifecycle {:?}",
                    short_id(&run_id.to_string()),
                    outcome.state,
                    outcome.lifecycle
                )));
                self.status = "cancel requested".into();
            }
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("cancel failed: {error:#}")));
                self.status = "cancel failed".into();
            }
        }
        Ok(())
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
                    "terminal session is unavailable for workflow execution".into(),
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
                "failed to resume chat after workflow: {resume_error:#}"
            )));
            if let Err(operation_error) = suspended.operation_result {
                self.insert_entry(&Entry::Error(format!(
                    "workflow also failed: {operation_error:#}"
                )));
            }
            self.status = "workflow handoff failed".into();
            return Ok(());
        }
        self.ctrl_c_streak = 0;
        match suspended.operation_result {
            Ok(()) => {
                self.insert_entry(&Entry::Notice(format!(
                    "workflow run {} returned to chat",
                    short_id(&run_id.to_string())
                )));
                self.status = success_status;
            }
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("workflow failed: {error:#}")));
                self.status = "workflow failed".into();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "workflow_hub_tests.rs"]
mod tests;
