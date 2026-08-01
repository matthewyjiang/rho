//! Shared workflow plan/run policy used by the CLI and the model tool.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use crate::{
    app::workflow_runtime::RecoveryDecision,
    workflow::{
        normalize_workflow, validate_runtime_budgets, validate_workflow, verify_directory_identity,
        verify_executable_identity, CollectedSources, Digest, FrozenSchedulerSettings,
        FrozenWorkflow, InputName, PlanConsent, PlanId, PlannerIdentity, PlanningLimits,
        ResolvedNode, RunId, RunLifecycle, SourceCollector, StoredPlan, StoredRun, WorkflowService,
        WorkflowStore, WorkflowValue, FROZEN_WORKFLOW_SCHEMA_VERSION,
    },
};

use super::{
    cancel::CancellationOutcome,
    plan_host::{resolve_nodes_with_host, DiscoveringPlanHost, PlanHost},
    planner_worker::{self, run_supervised_planner, PlannerWorkerPlan},
};

// Receipt: matches agent_executor::DEFAULT_TOTAL_CONCURRENCY. Kind limits
// use the same ceiling and cannot raise total parallel work.
const DEFAULT_PARALLEL_NODES: u32 = 4;
const DEFAULT_PARALLEL_AGENTS: u32 = 4;
const DEFAULT_PARALLEL_COMMANDS: u32 = 4;
const PLANNER_FORMAT_VERSION: u32 = 1;

pub(crate) struct PreparedPlan {
    pub(crate) sources: CollectedSources,
    pub(crate) workflow: FrozenWorkflow,
}

/// Owns validate | plan | run | status | cancel | resume policy for both adapters.
pub(crate) struct WorkflowOps {
    service: WorkflowService,
    workspace: PathBuf,
    config_path: Option<PathBuf>,
}

impl WorkflowOps {
    pub(crate) fn open(workspace: PathBuf, config_path: Option<PathBuf>) -> anyhow::Result<Self> {
        let workspace = workspace.canonicalize()?;
        Ok(Self {
            service: WorkflowService::new(WorkflowStore::new(&crate::paths::rho_dir()?)?),
            workspace,
            config_path,
        })
    }

    pub(crate) async fn collect_local_sources(
        &self,
        entry: &Path,
        limits: &PlanningLimits,
    ) -> anyhow::Result<CollectedSources> {
        Ok(SourceCollector::new(&self.workspace, limits)?
            .collect(entry)
            .await?)
    }

    pub(crate) async fn prepare_from_sources(
        &self,
        sources: CollectedSources,
        inputs: BTreeMap<InputName, WorkflowValue>,
        host: &dyn PlanHost,
        limits: &PlanningLimits,
    ) -> anyhow::Result<PreparedPlan> {
        let planned = run_supervised_planner(&sources, inputs, limits).await?;
        let resolved_nodes = resolve_nodes_with_host(&planned.graph, host)?;
        freeze_planned_workflow(sources, planned, resolved_nodes, limits)
    }

    pub(crate) async fn prepare_local(
        &self,
        entry: &Path,
        inputs: BTreeMap<InputName, WorkflowValue>,
        config: &crate::config::Config,
        available_tools: &crate::agent::AgentCapabilities,
        limits: &PlanningLimits,
    ) -> anyhow::Result<PreparedPlan> {
        let sources = self.collect_local_sources(entry, limits).await?;
        let host = DiscoveringPlanHost::new(&self.workspace, config, available_tools, entry)?;
        self.prepare_from_sources(sources, inputs, &host, limits)
            .await
    }

    /// Single freeze pipeline output stored once. No second normalize/validate build.
    pub(crate) fn store_plan(&self, prepared: &PreparedPlan) -> anyhow::Result<StoredPlan> {
        Ok(self.service.store_frozen(
            &prepared.workflow,
            workspace_identity(&self.workspace)?,
            &prepared.sources.sources,
        )?)
    }

    pub(crate) fn load_plan_prefix(&self, prefix: &str) -> anyhow::Result<StoredPlan> {
        let plan_id = self.service.store().resolve_plan(prefix)?;
        Ok(self.service.store().load_plan(plan_id)?)
    }

    pub(crate) fn load_plan_id(&self, plan_id: PlanId) -> anyhow::Result<StoredPlan> {
        Ok(self.service.store().load_plan(plan_id)?)
    }

    pub(crate) fn load_run_prefix(&self, prefix: &str) -> anyhow::Result<StoredRun> {
        let run_id = self.service.store().resolve_run(prefix)?;
        Ok(self.service.store().load_run(run_id)?)
    }

    pub(crate) fn load_run_id(&self, run_id: RunId) -> anyhow::Result<StoredRun> {
        Ok(self.service.store().load_run(run_id)?)
    }

    pub(crate) fn list_workspace_plans(&self) -> anyhow::Result<Vec<StoredPlan>> {
        let identity = workspace_identity(&self.workspace)?;
        Ok(self
            .service
            .store()
            .list_plans()?
            .into_iter()
            .filter(|plan| plan.manifest.workspace_identity == identity)
            .collect())
    }

    pub(crate) fn list_workspace_runs(&self) -> anyhow::Result<Vec<StoredRun>> {
        let identity = workspace_identity(&self.workspace)?;
        Ok(self
            .service
            .store()
            .list_runs()?
            .into_iter()
            .filter(|run| run.manifest.workspace_identity == identity)
            .collect())
    }

    pub(crate) fn delete_workspace_plan(&self, plan_id: PlanId) -> anyhow::Result<()> {
        let plan = self.load_plan_id(plan_id)?;
        let identity = workspace_identity(&self.workspace)?;
        if plan.manifest.workspace_identity != identity {
            anyhow::bail!("plan belongs to another workspace");
        }
        Ok(self.service.store().delete_plan(plan_id)?)
    }

    pub(crate) fn delete_workspace_run(&self, run_id: RunId) -> anyhow::Result<()> {
        let run = self.load_run_id(run_id)?;
        let identity = workspace_identity(&self.workspace)?;
        if run.manifest.workspace_identity != identity {
            anyhow::bail!("run belongs to another workspace");
        }
        if matches!(
            run.state.state.lifecycle,
            RunLifecycle::Running | RunLifecycle::Cancelling
        ) {
            anyhow::bail!(
                "run {} is still {}, stop it before deleting",
                run_id,
                format!("{:?}", run.state.state.lifecycle).to_ascii_lowercase()
            );
        }
        Ok(self.service.store().delete_run(run_id)?)
    }

    pub(crate) fn recheck_plan(&self, plan: &StoredPlan) -> anyhow::Result<()> {
        let current_workspace = workspace_identity(&self.workspace)?;
        if current_workspace != plan.manifest.workspace_identity {
            anyhow::bail!(
                "workflow plan workspace is '{}', but the current workspace is '{}'",
                plan.manifest.workspace_identity,
                current_workspace
            );
        }
        recheck_frozen_graph(&plan.graph, self.config_path.clone())
    }

    pub(crate) fn recheck_run(&self, run: &StoredRun) -> anyhow::Result<()> {
        let current_workspace = workspace_identity(&self.workspace)?;
        if current_workspace != run.manifest.workspace_identity {
            anyhow::bail!(
                "workflow run workspace is '{}', but the current workspace is '{}'",
                run.manifest.workspace_identity,
                current_workspace
            );
        }
        if crate::workflow::graph_digest(&run.graph)? != run.manifest.graph_digest {
            anyhow::bail!("workflow run digest does not match its copied frozen graph");
        }
        if !run.manifest.consent.confirmed
            || run.manifest.consent.graph_digest != run.manifest.graph_digest
        {
            anyhow::bail!("workflow run consent does not match its copied frozen graph");
        }
        recheck_frozen_graph(&run.graph, self.config_path.clone())
    }

    pub(crate) fn create_confirmed_run(&self, plan: &StoredPlan) -> anyhow::Result<StoredRun> {
        Ok(self.service.create_run(
            plan,
            PlanConsent {
                graph_digest: plan.manifest.graph_digest.clone(),
                confirmed: true,
            },
        )?)
    }

    pub(crate) fn prepare_run(&self, plan_prefix_or_id: &str) -> anyhow::Result<StoredPlan> {
        let plan = self.load_plan_prefix(plan_prefix_or_id)?;
        self.recheck_plan(&plan)?;
        Ok(plan)
    }

    pub(crate) fn prepare_run_id(&self, plan_id: PlanId) -> anyhow::Result<StoredPlan> {
        let plan = self.load_plan_id(plan_id)?;
        self.recheck_plan(&plan)?;
        Ok(plan)
    }

    pub(crate) fn prepare_resume(
        &self,
        run: &StoredRun,
        recover_uncertain: bool,
    ) -> anyhow::Result<RecoveryDecision> {
        self.recheck_run(run)?;
        if run.state.state.lifecycle == RunLifecycle::NeedsRecovery && !recover_uncertain {
            anyhow::bail!(
                "workflow run {} has uncertain attempts; confirm that no prior process remains, then use --recover-uncertain",
                run.manifest.run_id
            );
        }
        Ok(if recover_uncertain {
            RecoveryDecision::ConfirmNoProcess
        } else {
            RecoveryDecision::NormalResume
        })
    }

    pub(crate) async fn cancel(
        &self,
        run_id: RunId,
        lifecycle: RunLifecycle,
    ) -> anyhow::Result<CancellationOutcome> {
        let rho_home = crate::paths::rho_dir()?;
        Ok(super::request_cancellation(&rho_home, run_id, lifecycle).await?)
    }
}

pub(crate) fn freeze_planned_workflow(
    sources: CollectedSources,
    planned: PlannerWorkerPlan,
    resolved_nodes: BTreeMap<crate::workflow::NodeId, ResolvedNode>,
    limits: &PlanningLimits,
) -> anyhow::Result<PreparedPlan> {
    // Authoritative freeze: normalize + validate exactly once before store/validate display.
    let workflow = normalize_workflow(FrozenWorkflow {
        schema_version: FROZEN_WORKFLOW_SCHEMA_VERSION,
        planner: PlannerIdentity {
            name: "rho".to_owned(),
            format_version: PLANNER_FORMAT_VERSION,
            starlark_version: "0.14.2".to_owned(),
        },
        graph_digest: Digest(String::new()),
        sources: sources.manifest.clone(),
        inputs: planned.inputs,
        graph: planned.graph,
        resolved_nodes,
        scheduler: FrozenSchedulerSettings {
            max_parallel_nodes: DEFAULT_PARALLEL_NODES,
            max_parallel_agents: DEFAULT_PARALLEL_AGENTS,
            max_parallel_commands: DEFAULT_PARALLEL_COMMANDS,
        },
        runtime_limits: limits.frozen_runtime_limits(),
    })?;
    validate_workflow(&workflow)?;
    validate_runtime_budgets(&workflow, limits)?;
    Ok(PreparedPlan { sources, workflow })
}

pub(crate) fn workspace_identity(path: &Path) -> anyhow::Result<String> {
    Ok(crate::paths::display(&path.canonicalize()?))
}

fn recheck_frozen_graph(
    graph: &FrozenWorkflow,
    config_path: Option<PathBuf>,
) -> anyhow::Result<()> {
    validate_workflow(graph)?;
    validate_runtime_budgets(graph, &planner_worker::planning_limits()?)?;
    let config = crate::app::config_repository::ConfigRepository::new(config_path).load()?;
    for resolved in graph.resolved_nodes.values() {
        let agent = match resolved {
            ResolvedNode::Command(command) => {
                verify_executable_identity(&command.executable_identity)?;
                verify_directory_identity(&command.cwd_identity)?;
                continue;
            }
            ResolvedNode::Agent(agent) => agent,
        };
        match (&agent.executable, &agent.executable_identity) {
            (Some(_), Some(identity)) => {
                verify_executable_identity(identity)?;
            }
            (Some(path), None) => anyhow::bail!(
                "frozen agent '{}' records executable '{}' without a frozen identity",
                agent.agent_id,
                path
            ),
            (None, _) => {}
        }
        if agent.trust_required
            && std::env::var_os("RHO_TRUST_PROJECT_AGENTS").as_deref()
                != Some(std::ffi::OsStr::new("1"))
        {
            anyhow::bail!(
                "workflow plan requires trusted project agent '{}'; trust is not active",
                agent.agent_id
            );
        }
        let current_rank = permission_rank(config.permission_mode.as_str()).ok_or_else(|| {
            anyhow::anyhow!(
                "current permission mode '{}' is unsupported",
                config.permission_mode
            )
        })?;
        let ceiling_rank = permission_rank(&agent.permission_ceiling).ok_or_else(|| {
            anyhow::anyhow!(
                "frozen permission ceiling '{}' for agent '{}' is unsupported",
                agent.permission_ceiling,
                agent.agent_id
            )
        })?;
        if current_rank > ceiling_rank {
            anyhow::bail!(
                "current permission mode '{}' would widen frozen authority '{}' for agent '{}'",
                config.permission_mode,
                agent.permission_ceiling,
                agent.agent_id
            );
        }
    }
    Ok(())
}

fn permission_rank(value: &str) -> Option<u8> {
    match value {
        "plan" => Some(0),
        "supervised" => Some(1),
        "auto" => Some(2),
        _ => None,
    }
}
