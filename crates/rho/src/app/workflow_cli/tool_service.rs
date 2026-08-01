use std::{
    collections::BTreeMap,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use rho_sdk::{
    tool::{ToolContext, ToolError, ToolErrorKind},
    CapabilityRequest, CapabilitySource, HostChoice, HostInputRequest, HostQuestion, PathScope,
    ProcessEnvironment, ProcessExecution, ProcessInvocation, ProcessOutputLimits, SelectionMode,
};

use crate::{
    agent::AgentCapabilities,
    app::workflow_runtime::RecoveryDecision,
    tools::{
        workflow::{
            WorkflowArtifactSummary, WorkflowCancellationStateSummary, WorkflowDiagnosticSummary,
            WorkflowNodeStateSummary, WorkflowNodeSummary, WorkflowRunStateSummary,
            WorkflowToolRequest, WorkflowToolResult, WorkflowToolService,
        },
        workflow_tracker::WorkflowRunTracker,
    },
    workflow::{
        InputName, NodeState, NodeTerminalState, PlanningLimits, RunLifecycle, SourceBytes,
        SourceCollector, StoredRun, WorkflowError, WorkflowResult, WorkflowValue,
    },
};

use super::{diagnostic_for_model_error, ops::WorkflowOps, plan_host::AuthorizedPlanHost, runtime};

mod capabilities;

pub(in crate::app) fn workflow_tool_service(
    cwd: PathBuf,
    config_path: Option<PathBuf>,
    tracker: WorkflowRunTracker,
) -> Arc<dyn WorkflowToolService> {
    Arc::new(AppWorkflowToolService {
        cwd,
        config_path,
        tracker,
    })
}

struct AppWorkflowToolService {
    cwd: PathBuf,
    config_path: Option<PathBuf>,
    tracker: WorkflowRunTracker,
}

impl WorkflowToolService for AppWorkflowToolService {
    fn prepare(&self, request: &WorkflowToolRequest) -> Result<Vec<CapabilityRequest>, ToolError> {
        self.capabilities_for(request)
            .map_err(model_workflow_tool_error)
    }

    fn execute<'a>(
        &'a self,
        request: WorkflowToolRequest,
        context: &'a ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<WorkflowToolResult, ToolError>> + Send + 'a>> {
        Box::pin(async move { self.execute_request(request, context).await })
    }
}

impl AppWorkflowToolService {
    fn capabilities_for(
        &self,
        request: &WorkflowToolRequest,
    ) -> anyhow::Result<Vec<CapabilityRequest>> {
        let rho_home = crate::paths::rho_dir()?;
        let executable = std::env::current_exe()?;
        self.capabilities_for_paths(
            request,
            &rho_home,
            crate::paths::home_dir().as_deref(),
            &executable,
            project_agent_catalogs_trusted(),
        )
    }

    fn capabilities_for_paths(
        &self,
        request: &WorkflowToolRequest,
        rho_home: &Path,
        home: Option<&Path>,
        executable: &Path,
        project_agents_trusted: bool,
    ) -> anyhow::Result<Vec<CapabilityRequest>> {
        let source = || CapabilitySource::built_in_tool("workflow");
        let plans = rho_home.join("workflows/plans");
        let runs = rho_home.join("workflows/runs");
        let capabilities = match request {
            WorkflowToolRequest::Validate { file, .. } | WorkflowToolRequest::Plan { file, .. } => {
                let mut requests =
                    self.planning_read_capabilities(file, rho_home, home, project_agents_trusted)?;
                requests.push(CapabilityRequest::process(
                    self.planner_process_request(executable)?,
                    source(),
                ));
                if matches!(request, WorkflowToolRequest::Plan { .. }) {
                    requests.push(CapabilityRequest::write_path(
                        plans,
                        PathScope::UnrestrictedFilesystem,
                        source(),
                    ));
                }
                requests
            }
            WorkflowToolRequest::Run { plan_id } => vec![
                CapabilityRequest::read_path(
                    durable_id_path(&plans, plan_id),
                    PathScope::UnrestrictedFilesystem,
                    source(),
                ),
                CapabilityRequest::write_path(runs, PathScope::UnrestrictedFilesystem, source()),
            ],
            WorkflowToolRequest::Status { run_id } => vec![CapabilityRequest::read_path(
                durable_id_path(&runs, run_id),
                PathScope::UnrestrictedFilesystem,
                source(),
            )],
            WorkflowToolRequest::Cancel { run_id } => {
                let run = durable_id_path(&runs, run_id);
                vec![
                    CapabilityRequest::read_path(
                        run.clone(),
                        PathScope::UnrestrictedFilesystem,
                        source(),
                    ),
                    CapabilityRequest::write_path(
                        run.join("cancel.request"),
                        PathScope::UnrestrictedFilesystem,
                        source(),
                    ),
                ]
            }
            WorkflowToolRequest::Resume { run_id, .. } => {
                let run = durable_id_path(&runs, run_id);
                vec![
                    CapabilityRequest::read_path(
                        run.clone(),
                        PathScope::UnrestrictedFilesystem,
                        source(),
                    ),
                    CapabilityRequest::write_path(run, PathScope::UnrestrictedFilesystem, source()),
                ]
            }
        };
        Ok(capabilities)
    }

    fn planning_read_capabilities(
        &self,
        file: &str,
        rho_home: &Path,
        home: Option<&Path>,
        project_agents_trusted: bool,
    ) -> anyhow::Result<Vec<CapabilityRequest>> {
        let source = || CapabilitySource::built_in_tool("workflow");
        let mut requests = vec![
            CapabilityRequest::read_path(
                self.source_request_path(file)?,
                PathScope::PrimaryWorkspace,
                source(),
            ),
            CapabilityRequest::read_path(
                self.config_request_path(rho_home),
                PathScope::UnrestrictedFilesystem,
                source(),
            ),
        ];
        for path in agent_catalog_roots_for(&self.cwd, home, project_agents_trusted) {
            let scope = if path.starts_with(&self.cwd) {
                PathScope::PrimaryWorkspace
            } else {
                PathScope::UnrestrictedFilesystem
            };
            requests.push(CapabilityRequest::read_path(path, scope, source()));
        }
        let workflow_agents = crate::agent::workflow_local_agents_root(Path::new(file));
        let workflow_agents = if workflow_agents.is_absolute() {
            workflow_agents
        } else {
            self.cwd.join(workflow_agents)
        };
        requests.push(CapabilityRequest::read_path(
            workflow_agents,
            PathScope::PrimaryWorkspace,
            source(),
        ));
        Ok(requests)
    }

    fn config_request_path(&self, rho_home: &Path) -> PathBuf {
        match &self.config_path {
            Some(path) if path.is_absolute() => path.clone(),
            Some(path) => self.cwd.join(path),
            None => rho_home.join("config.toml"),
        }
    }

    fn source_request_path(&self, file: &str) -> anyhow::Result<PathBuf> {
        let path = Path::new(file);
        if path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            anyhow::bail!("workflow source path must not contain '..'");
        }
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.cwd.join(path)
        };
        if !path.starts_with(&self.cwd) {
            anyhow::bail!("workflow source is outside the workspace");
        }
        Ok(path)
    }

    fn planner_process_request(&self, executable: &Path) -> anyhow::Result<ProcessExecution> {
        let limits = super::planning_limits().map_err(anyhow::Error::from)?;
        Ok(ProcessExecution::new(
            &self.cwd,
            ProcessInvocation::executable(
                executable,
                vec![crate::cli::WORKFLOW_PLANNER_WORKER_COMMAND.into()],
            ),
            ProcessEnvironment::InheritListed {
                variable_names: vec![super::PLANNER_WORKER_ENV.to_owned()],
            },
            ProcessOutputLimits::new(
                usize::try_from(super::PLANNER_RESPONSE_FRAME_BYTES + 8)
                    .map_err(|_| anyhow::anyhow!("planner response limit does not fit platform"))?,
                Some(std::time::Duration::from_millis(
                    limits.worker_wall_millis.limit,
                )),
            ),
        ))
    }

    async fn execute_request(
        &self,
        request: WorkflowToolRequest,
        context: &ToolContext,
    ) -> Result<WorkflowToolResult, ToolError> {
        match request {
            WorkflowToolRequest::Validate { file, inputs } => {
                match self.prepare(Path::new(&file), inputs, context).await {
                    Ok(_) => Ok(WorkflowToolResult::Validate {
                        valid: true,
                        diagnostics: Vec::new(),
                    }),
                    Err(error) => Ok(WorkflowToolResult::Validate {
                        valid: false,
                        diagnostics: vec![diagnostic_summary(diagnostic_for_model_error(&error))],
                    }),
                }
            }
            WorkflowToolRequest::Plan { file, inputs } => {
                let prepared = self
                    .prepare(Path::new(&file), inputs, context)
                    .await
                    .map_err(model_workflow_tool_error)?;
                let ops = self.ops().map_err(model_workflow_tool_error)?;
                let stored = ops
                    .store_plan(&prepared)
                    .map_err(model_workflow_tool_error)?;
                Ok(WorkflowToolResult::Plan {
                    plan_id: stored.manifest.plan_id.to_string(),
                    graph_digest: stored.manifest.graph_digest.0.clone(),
                    workflow_name: stored.graph.graph.name.as_str().to_owned(),
                    node_count: stored.graph.graph.nodes.len() as u64,
                })
            }
            WorkflowToolRequest::Run { plan_id } => {
                let ops = self.ops().map_err(model_workflow_tool_error)?;
                let plan = ops
                    .prepare_run_id(plan_id)
                    .map_err(model_workflow_tool_error)?;
                confirm_exact_plan(context, "Run", &plan.manifest.graph_digest.0).await?;
                let run = ops
                    .create_confirmed_run(&plan)
                    .map_err(model_workflow_tool_error)?;
                self.tracker.register_start(
                    run.manifest.run_id.to_string(),
                    run.graph.graph.name.as_str(),
                    run.manifest.graph_digest.0.clone(),
                    None,
                );
                let started = runtime::spawn_background_run(
                    run,
                    RecoveryDecision::NormalResume,
                    self.config_path.clone(),
                    context.child_approval_session(),
                    Some(self.tracker.clone()),
                )
                .await
                .map_err(model_workflow_tool_error)?;
                run_result(started, RunResultKind::Run)
            }
            WorkflowToolRequest::Status { run_id } => {
                let ops = self.ops().map_err(model_workflow_tool_error)?;
                let run = ops.load_run_id(run_id).map_err(model_workflow_tool_error)?;
                observe_if_terminal(&self.tracker, &run);
                run_result(run, RunResultKind::Status)
            }
            WorkflowToolRequest::Cancel { run_id } => {
                let ops = self.ops().map_err(model_workflow_tool_error)?;
                let run = ops.load_run_id(run_id).map_err(model_workflow_tool_error)?;
                let outcome = ops
                    .cancel(run.manifest.run_id, run.state.state.lifecycle)
                    .await
                    .map_err(model_workflow_tool_error)?;
                if matches!(
                    outcome.lifecycle,
                    RunLifecycle::Completed | RunLifecycle::NeedsRecovery
                ) {
                    self.tracker.observe(&run.manifest.run_id.to_string());
                }
                Ok(WorkflowToolResult::Cancel {
                    run_id: run.manifest.run_id.to_string(),
                    request_id: outcome.request_id,
                    cancellation_state: match outcome.state {
                        super::CancellationState::Acknowledged => {
                            WorkflowCancellationStateSummary::Acknowledged
                        }
                        super::CancellationState::Pending => {
                            WorkflowCancellationStateSummary::Pending
                        }
                        super::CancellationState::AlreadyCompleted => {
                            WorkflowCancellationStateSummary::AlreadyCompleted
                        }
                    },
                    state: state_summary(outcome.lifecycle),
                })
            }
            WorkflowToolRequest::Resume {
                run_id,
                recover_uncertain,
            } => {
                let ops = self.ops().map_err(model_workflow_tool_error)?;
                let run = ops.load_run_id(run_id).map_err(model_workflow_tool_error)?;
                let recovery = ops
                    .prepare_resume(&run, recover_uncertain)
                    .map_err(|error| {
                        if error.to_string().contains("uncertain attempts") {
                            ToolError::new(
                                ToolErrorKind::InvalidArguments,
                                "the run has uncertain attempts; confirm that no prior process remains and set recover_uncertain to true",
                            )
                        } else {
                            model_workflow_tool_error(error)
                        }
                    })?;
                confirm_exact_plan(context, "Resume", &run.manifest.graph_digest.0).await?;
                self.tracker.register_start(
                    run.manifest.run_id.to_string(),
                    run.graph.graph.name.as_str(),
                    run.manifest.graph_digest.0.clone(),
                    None,
                );
                let started = runtime::spawn_background_run(
                    run,
                    recovery,
                    self.config_path.clone(),
                    context.child_approval_session(),
                    Some(self.tracker.clone()),
                )
                .await
                .map_err(model_workflow_tool_error)?;
                run_result(started, RunResultKind::Resume)
            }
        }
    }

    fn ops(&self) -> anyhow::Result<WorkflowOps> {
        WorkflowOps::open(self.cwd.clone(), self.config_path.clone())
    }

    async fn prepare(
        &self,
        file: &Path,
        inputs: BTreeMap<String, serde_json::Value>,
        context: &ToolContext,
    ) -> anyhow::Result<super::PreparedPlan> {
        let catalog = self.authorized_agent_catalog(context, file).await?;
        let rho_home = crate::paths::rho_dir()?;
        let config_path = self.config_request_path(&rho_home);
        let config = self.authorized_config(context, &config_path).await?;
        let supplied_inputs = inputs
            .into_iter()
            .map(|(name, value)| Ok((InputName::new(name)?, WorkflowValue::from_json(value)?)))
            .collect::<anyhow::Result<BTreeMap<_, _>>>()?;
        let limits = super::planning_limits()?;
        let sources = self.collect_sources(file, &limits, context).await?;
        let planned =
            super::planner_worker::run_supervised_planner(&sources, supplied_inputs, &limits)
                .await?;
        let executable_identities = self
            .authorize_node_resolution_reads(&planned.graph, &catalog, context)
            .await?;
        let available_tools = AgentCapabilities::all_host_tools();
        let host = AuthorizedPlanHost::new(
            &self.cwd,
            &config,
            &catalog,
            &available_tools,
            &executable_identities,
        );
        let resolved_nodes = super::plan_host::resolve_nodes_with_host(&planned.graph, &host)?;
        super::freeze_planned_workflow(sources, planned, resolved_nodes, &limits)
    }

    async fn collect_sources(
        &self,
        entry: &Path,
        limits: &PlanningLimits,
        context: &ToolContext,
    ) -> anyhow::Result<crate::workflow::CollectedSources> {
        let collector = SourceCollector::new(&self.cwd, limits)?;
        let mut reader = ToolSourceBytes {
            service: self,
            context,
        };
        Ok(collector.collect_with(&mut reader, entry).await?)
    }
}

/// Tool path reader: authorize each module through `ToolContext`, then open a verified handle.
struct ToolSourceBytes<'a> {
    service: &'a AppWorkflowToolService,
    context: &'a ToolContext,
}

impl SourceBytes for ToolSourceBytes<'_> {
    fn read_source<'a>(
        &'a mut self,
        root_relative: &'a Path,
        budget: &'a crate::workflow::Budget,
        retained: u64,
    ) -> Pin<Box<dyn Future<Output = WorkflowResult<String>> + Send + 'a>> {
        Box::pin(async move {
            let lexical = self.service.cwd.join(root_relative);
            let opened = self
                .service
                .authorize_path(self.context, &lexical)
                .await
                .map_err(workflow_error_from_anyhow)?;
            opened.read_utf8_bounded(budget, retained)
        })
    }
}

fn workflow_error_from_anyhow(error: anyhow::Error) -> WorkflowError {
    match error.downcast::<WorkflowError>() {
        Ok(error) => error,
        Err(error) => WorkflowError::Starlark(error.to_string()),
    }
}

fn project_agent_catalogs_trusted() -> bool {
    std::env::var_os("RHO_TRUST_PROJECT_AGENTS").as_deref() == Some(std::ffi::OsStr::new("1"))
}

fn agent_catalog_roots_for(
    cwd: &Path,
    home: Option<&Path>,
    project_agents_trusted: bool,
) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = home {
        roots.push(home.join(".agents/agents"));
        roots.push(home.join(".rho/agents"));
    }
    if project_agents_trusted {
        roots.extend(
            crate::workspace::project_ancestor_dirs(cwd)
                .into_iter()
                .map(|path| path.join(".agents/agents")),
        );
    }
    roots
}

fn path_scope(cwd: &Path, path: &Path) -> PathScope {
    if path.starts_with(cwd) {
        PathScope::PrimaryWorkspace
    } else {
        PathScope::UnrestrictedFilesystem
    }
}

fn executable_candidates(program: &str) -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|paths| executable_candidates_in(program, std::env::split_paths(&paths)))
        .unwrap_or_default()
}

fn executable_candidates_in(
    program: &str,
    directories: impl IntoIterator<Item = PathBuf>,
) -> Vec<PathBuf> {
    directories
        .into_iter()
        .map(|directory| {
            let path = directory.join(program);
            #[cfg(windows)]
            {
                if !program.contains('.') {
                    return path.with_extension("exe");
                }
            }
            path
        })
        .collect()
}

fn durable_id_path(root: &Path, id: impl std::fmt::Display) -> PathBuf {
    root.join(id.to_string())
}

#[cfg(test)]
#[path = "tool_service_tests.rs"]
mod tests;

async fn confirm_exact_plan(
    context: &ToolContext,
    action: &str,
    digest: &str,
) -> Result<(), ToolError> {
    let question = HostQuestion::new(
        "confirm",
        format!("{action} workflow plan {digest}?"),
        vec![
            HostChoice::new("yes", format!("{action} {digest}")),
            HostChoice::new("no", "Do not continue"),
        ],
        SelectionMode::One,
    )
    .map_err(host_input_error)?;
    let request = HostInputRequest::questionnaire(
        format!("Confirm exact workflow plan {digest}"),
        vec![question],
    )
    .map_err(host_input_error)?;
    let response = context
        .request_host_input(request)
        .await
        .map_err(host_input_error)?;
    let confirmed = response
        .answers()
        .get("confirm")
        .is_some_and(|answers| answers.iter().any(|answer| answer == "yes"));
    if !confirmed {
        return Err(ToolError::cancelled());
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum RunResultKind {
    Run,
    Status,
    Resume,
}

fn observe_if_terminal(tracker: &WorkflowRunTracker, run: &StoredRun) {
    if matches!(
        run.state.state.lifecycle,
        RunLifecycle::Completed | RunLifecycle::NeedsRecovery
    ) {
        // Status that already saw the terminal durable state counts as delivery.
        tracker.mark_finished_from_stored(run);
        tracker.observe(&run.manifest.run_id.to_string());
    }
}

fn run_result(run: StoredRun, kind: RunResultKind) -> Result<WorkflowToolResult, ToolError> {
    let run_id = run.manifest.run_id.to_string();
    let graph_digest = run.manifest.graph_digest.0.clone();
    let state = state_summary(run.state.state.lifecycle);
    let nodes = run
        .state
        .state
        .nodes
        .iter()
        .map(|(node_id, state)| {
            let attempt = match state {
                NodeState::Running { attempt } => Some(attempt.get()),
                NodeState::Pending | NodeState::Ready => None,
                NodeState::Terminal { .. } => run
                    .state
                    .state
                    .completions
                    .get(node_id)
                    .and_then(|completion| completion.attempt)
                    .map(crate::workflow::AttemptNumber::get),
            };
            WorkflowNodeSummary {
                node_id: node_id.to_string(),
                state: node_state_summary(state),
                attempt,
                artifacts: run
                    .state
                    .state
                    .completions
                    .get(node_id)
                    .into_iter()
                    .flat_map(|completion| completion.artifacts.iter())
                    .map(|(kind, artifact)| WorkflowArtifactSummary {
                        kind,
                        artifact: artifact.clone(),
                    })
                    .collect(),
            }
        })
        .collect();
    Ok(match kind {
        RunResultKind::Run => WorkflowToolResult::Run {
            run_id,
            graph_digest,
            state,
            nodes,
        },
        RunResultKind::Status => WorkflowToolResult::Status {
            run_id,
            graph_digest,
            state,
            nodes,
        },
        RunResultKind::Resume => WorkflowToolResult::Resume {
            run_id,
            graph_digest,
            state,
            nodes,
        },
    })
}

fn state_summary(lifecycle: RunLifecycle) -> WorkflowRunStateSummary {
    match lifecycle {
        RunLifecycle::Planned => WorkflowRunStateSummary::Planned,
        RunLifecycle::Running => WorkflowRunStateSummary::Running,
        RunLifecycle::Cancelling => WorkflowRunStateSummary::Cancelling,
        RunLifecycle::Completed => WorkflowRunStateSummary::Completed,
        RunLifecycle::NeedsRecovery => WorkflowRunStateSummary::NeedsRecovery,
    }
}

fn node_state_summary(state: &NodeState) -> WorkflowNodeStateSummary {
    match state {
        NodeState::Pending => WorkflowNodeStateSummary::Pending,
        NodeState::Ready => WorkflowNodeStateSummary::Ready,
        NodeState::Running { .. } => WorkflowNodeStateSummary::Running,
        NodeState::Terminal { outcome } => match outcome {
            NodeTerminalState::Success => WorkflowNodeStateSummary::Success,
            NodeTerminalState::Failure => WorkflowNodeStateSummary::Failure,
            NodeTerminalState::Denial => WorkflowNodeStateSummary::Denial,
            NodeTerminalState::Cancellation => WorkflowNodeStateSummary::Cancellation,
            NodeTerminalState::Skipped => WorkflowNodeStateSummary::Skipped,
            NodeTerminalState::Blocked => WorkflowNodeStateSummary::Blocked,
        },
    }
}

fn diagnostic_summary(diagnostic: crate::workflow::Diagnostic) -> WorkflowDiagnosticSummary {
    WorkflowDiagnosticSummary {
        severity: "error".into(),
        code: diagnostic.code,
        message: diagnostic.message,
        source: diagnostic.span.as_ref().map(|span| span.label.clone()),
        line: diagnostic.span.as_ref().map(|span| u64::from(span.line)),
        column: diagnostic.span.as_ref().map(|span| u64::from(span.column)),
    }
}

fn model_workflow_tool_error(error: anyhow::Error) -> ToolError {
    let diagnostic = diagnostic_for_model_error(&error);
    ToolError::new(ToolErrorKind::Execution, diagnostic.message)
}

fn host_input_error(error: impl std::fmt::Display) -> ToolError {
    ToolError::new(ToolErrorKind::Execution, error.to_string())
}
