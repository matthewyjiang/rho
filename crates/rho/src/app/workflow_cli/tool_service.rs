use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    str::FromStr,
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
    tools::workflow::{
        WorkflowArtifactSummary, WorkflowCancellationStateSummary, WorkflowDiagnosticSummary,
        WorkflowNodeStateSummary, WorkflowNodeSummary, WorkflowRunStateSummary,
        WorkflowToolRequest, WorkflowToolResult, WorkflowToolService,
    },
    workflow::{
        CollectedSources, Digest, FreezePlan, InputName, NodeState, NodeTerminalState, PlanConsent,
        PlanningLimits, RunLifecycle, SourceFile, SourceManifest, StoredPlan, StoredRun,
        WorkflowError, WorkflowStore, WorkflowValue,
    },
};

use super::{diagnostic_for_error, recheck_plan, runtime};

mod capabilities;

pub(in crate::app) fn workflow_tool_service(
    cwd: PathBuf,
    config_path: Option<PathBuf>,
) -> Arc<dyn WorkflowToolService> {
    Arc::new(AppWorkflowToolService { cwd, config_path })
}

struct AppWorkflowToolService {
    cwd: PathBuf,
    config_path: Option<PathBuf>,
}

impl WorkflowToolService for AppWorkflowToolService {
    fn prepare(&self, request: &WorkflowToolRequest) -> Result<Vec<CapabilityRequest>, ToolError> {
        self.capabilities_for(request).map_err(tool_error)
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
        )
    }

    fn capabilities_for_paths(
        &self,
        request: &WorkflowToolRequest,
        rho_home: &Path,
        home: Option<&Path>,
        executable: &Path,
    ) -> anyhow::Result<Vec<CapabilityRequest>> {
        let source = || CapabilitySource::built_in_tool("workflow");
        let plans = rho_home.join("workflows/plans");
        let runs = rho_home.join("workflows/runs");
        let capabilities = match request {
            WorkflowToolRequest::Validate { file, .. } | WorkflowToolRequest::Plan { file, .. } => {
                let mut requests = self.planning_read_capabilities(file, rho_home, home)?;
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
                    durable_id_path(&plans, plan_id)?,
                    PathScope::UnrestrictedFilesystem,
                    source(),
                ),
                CapabilityRequest::write_path(runs, PathScope::UnrestrictedFilesystem, source()),
            ],
            WorkflowToolRequest::Status { run_id } => vec![CapabilityRequest::read_path(
                durable_id_path(&runs, run_id)?,
                PathScope::UnrestrictedFilesystem,
                source(),
            )],
            WorkflowToolRequest::Cancel { run_id } => {
                let run = durable_id_path(&runs, run_id)?;
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
                let run = durable_id_path(&runs, run_id)?;
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
        for path in agent_catalog_roots(&self.cwd, home) {
            let scope = if path.starts_with(&self.cwd) {
                PathScope::PrimaryWorkspace
            } else {
                PathScope::UnrestrictedFilesystem
            };
            requests.push(CapabilityRequest::read_path(path, scope, source()));
        }
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
                vec!["workflow".into(), "validate".into(), "worker.star".into()],
            ),
            ProcessEnvironment::InheritAll,
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
                        diagnostics: vec![diagnostic_summary(diagnostic_for_error(&error))],
                    }),
                }
            }
            WorkflowToolRequest::Plan { file, inputs } => {
                let prepared = self
                    .prepare(Path::new(&file), inputs, context)
                    .await
                    .map_err(tool_error)?;
                let stored = super::workflow_service()
                    .map_err(tool_error)?
                    .freeze_and_store(FreezePlan {
                        planner: prepared.workflow.planner,
                        sources: prepared.sources.manifest,
                        source_bytes: &prepared.sources.sources,
                        inputs: prepared.workflow.inputs,
                        graph: prepared.workflow.graph,
                        resolved_nodes: prepared.workflow.resolved_nodes,
                        scheduler: prepared.workflow.scheduler,
                        runtime_limits: prepared.workflow.runtime_limits,
                        workspace_identity: super::workspace_identity(&self.cwd)
                            .map_err(tool_error)?,
                    })
                    .map_err(tool_error)?;
                Ok(WorkflowToolResult::Plan {
                    plan_id: stored.manifest.plan_id.to_string(),
                    graph_digest: stored.manifest.graph_digest.0.clone(),
                    workflow_name: stored.graph.graph.name.as_str().to_owned(),
                    node_count: stored.graph.graph.nodes.len() as u64,
                })
            }
            WorkflowToolRequest::Run { plan_id } => {
                let plan = self.load_plan(&plan_id)?;
                recheck_plan(&plan, self.config_path.clone()).map_err(tool_error)?;
                confirm_exact_plan(context, "Run", &plan.manifest.graph_digest.0).await?;
                let run = super::workflow_service()
                    .map_err(tool_error)?
                    .create_run(
                        &plan,
                        PlanConsent {
                            graph_digest: plan.manifest.graph_digest.clone(),
                            confirmed: true,
                        },
                    )
                    .map_err(tool_error)?;
                let completed = runtime::execute_tool_run(
                    run,
                    RecoveryDecision::NormalResume,
                    self.config_path.clone(),
                    context,
                )
                .await
                .map_err(tool_error)?;
                run_result(completed, RunResultKind::Run)
            }
            WorkflowToolRequest::Status { run_id } => {
                run_result(self.load_run(&run_id)?, RunResultKind::Status)
            }
            WorkflowToolRequest::Cancel { run_id } => {
                let run = self.load_run(&run_id)?;
                let outcome = super::request_cancellation(
                    &crate::paths::rho_dir().map_err(tool_error)?,
                    run.manifest.run_id,
                    run.state.state.lifecycle,
                )
                .await
                .map_err(tool_error)?;
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
                let run = self.load_run(&run_id)?;
                super::recheck_run(&run, self.config_path.clone()).map_err(tool_error)?;
                if run.state.state.lifecycle == RunLifecycle::NeedsRecovery && !recover_uncertain {
                    return Err(ToolError::new(
                        ToolErrorKind::InvalidArguments,
                        "the run has uncertain attempts; confirm that no prior process remains and set recover_uncertain to true",
                    ));
                }
                confirm_exact_plan(context, "Resume", &run.manifest.graph_digest.0).await?;
                let recovery = if recover_uncertain {
                    RecoveryDecision::ConfirmNoProcess
                } else {
                    RecoveryDecision::NormalResume
                };
                let completed =
                    runtime::execute_tool_run(run, recovery, self.config_path.clone(), context)
                        .await
                        .map_err(tool_error)?;
                run_result(completed, RunResultKind::Resume)
            }
        }
    }

    async fn prepare(
        &self,
        file: &Path,
        inputs: BTreeMap<String, serde_json::Value>,
        context: &ToolContext,
    ) -> anyhow::Result<super::PreparedPlan> {
        let catalog = self.authorized_agent_catalog(context).await?;
        let rho_home = crate::paths::rho_dir()?;
        let config_path = self.config_request_path(&rho_home);
        let config = self.authorized_config(context, &config_path).await?;
        let supplied_inputs = inputs
            .into_iter()
            .map(|(name, value)| Ok((InputName::new(name)?, WorkflowValue::from_json(value)?)))
            .collect::<anyhow::Result<BTreeMap<_, _>>>()?;
        let limits = super::planning_limits()?;
        let sources = self.collect_sources(file, &limits, context).await?;
        let planned = super::run_supervised_planner(&sources, supplied_inputs, &limits).await?;
        let executable_identities = self
            .authorize_node_resolution_reads(&planned.graph, &catalog, context)
            .await?;
        let resolved_nodes = super::resolve_nodes_with_authorized_executables(
            &planned.graph,
            &config,
            &self.cwd,
            &AgentCapabilities::all_host_tools(),
            &catalog,
            &executable_identities,
        )?;
        super::freeze_planned_workflow(sources, planned, resolved_nodes, &limits)
    }

    async fn collect_sources(
        &self,
        entry: &Path,
        limits: &PlanningLimits,
        context: &ToolContext,
    ) -> anyhow::Result<CollectedSources> {
        use sha2::Digest as _;
        use starlark::syntax::{AstModule, Dialect};

        let entry = if entry.is_absolute() {
            entry.to_path_buf()
        } else {
            self.cwd.join(entry)
        };
        let entry = context
            .workspace()
            .ok_or_else(|| anyhow::anyhow!("workflow tool requires a workspace"))?
            .resolve(&entry)?;
        let relative =
            entry
                .strip_prefix(&self.cwd)
                .map_err(|_| WorkflowError::SourceOutsideRoot {
                    path: entry.clone(),
                })?;
        let entry_label = format!("//{}", crate::paths::display(relative));
        source_relative_path(&entry_label)?;

        let mut pending = vec![(entry_label.clone(), 1_u64, Vec::<String>::new())];
        let mut seen = BTreeSet::new();
        let mut sources = BTreeMap::new();
        let mut total_bytes = 0_u64;
        while let Some((label, depth, ancestors)) = pending.pop() {
            limits.module_depth.check(depth)?;
            if let Some(index) = ancestors.iter().position(|ancestor| ancestor == &label) {
                let mut cycle = ancestors[index..].to_vec();
                cycle.push(label);
                return Err(WorkflowError::ImportCycle {
                    chain: cycle.join(" -> "),
                }
                .into());
            }
            if seen.contains(&label) {
                continue;
            }
            limits.module_count.check((seen.len() + 1) as u64)?;
            let relative = source_relative_path(&label)?;
            let lexical = self.cwd.join(&relative);
            let opened = self.authorize_path(context, &lexical).await?;
            let source = crate::workflow::read_opened_utf8_bounded(
                opened,
                &limits.total_source_bytes,
                total_bytes,
            )?;
            total_bytes = total_bytes.checked_add(source.len() as u64).ok_or(
                WorkflowError::BudgetExceeded {
                    budget: limits.total_source_bytes.name,
                    limit: limits.total_source_bytes.limit,
                    actual: u64::MAX,
                },
            )?;
            limits.total_source_bytes.check(total_bytes)?;
            let ast = AstModule::parse(&label, source.clone(), &Dialect::Standard)
                .map_err(|error| WorkflowError::Starlark(error.to_string()))?;
            let mut next_ancestors = ancestors;
            next_ancestors.push(label.clone());
            for load in ast.loads().iter().rev() {
                source_relative_path(load.module_id)?;
                pending.push((load.module_id.to_owned(), depth + 1, next_ancestors.clone()));
            }
            seen.insert(label.clone());
            sources.insert(label, source);
        }
        let modules = sources
            .iter()
            .map(|(label, source)| {
                let digest = sha2::Sha256::digest(source.as_bytes());
                (
                    label.clone(),
                    SourceFile {
                        digest: Digest(format!("sha256:{digest:x}")),
                        bytes: source.len() as u64,
                    },
                )
            })
            .collect();
        Ok(CollectedSources {
            entry_label: entry_label.clone(),
            sources,
            manifest: SourceManifest {
                entry_label,
                modules,
            },
        })
    }

    fn load_plan(&self, plan_id: &str) -> Result<StoredPlan, ToolError> {
        let rho_home = crate::paths::rho_dir().map_err(tool_error)?;
        let store = WorkflowStore::new(&rho_home).map_err(tool_error)?;
        let parsed = crate::workflow::PlanId::from_str(plan_id)
            .map_err(|_| invalid_request("plan_id must be a canonical full UUID"))?;
        if parsed.to_string() != plan_id {
            return Err(invalid_request("plan_id must be a canonical full UUID"));
        }
        store.load_plan(parsed).map_err(tool_error)
    }

    fn load_run(&self, run_id: &str) -> Result<StoredRun, ToolError> {
        let rho_home = crate::paths::rho_dir().map_err(tool_error)?;
        let store = WorkflowStore::new(&rho_home).map_err(tool_error)?;
        let parsed = crate::workflow::RunId::from_str(run_id)
            .map_err(|_| invalid_request("run_id must be a canonical full UUID"))?;
        if parsed.to_string() != run_id {
            return Err(invalid_request("run_id must be a canonical full UUID"));
        }
        store.load_run(parsed).map_err(tool_error)
    }
}

fn agent_catalog_roots(cwd: &Path, home: Option<&Path>) -> Vec<PathBuf> {
    agent_catalog_roots_for(cwd, home, project_agent_catalogs_trusted())
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

fn durable_id_path(root: &Path, value: &str) -> anyhow::Result<PathBuf> {
    let parsed = uuid::Uuid::parse_str(value)
        .map_err(|_| anyhow::anyhow!("workflow durable ID must be a canonical full UUID"))?;
    if parsed.to_string() != value {
        anyhow::bail!("workflow durable ID must be a canonical full UUID");
    }
    Ok(root.join(value))
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

fn source_relative_path(label: &str) -> Result<PathBuf, WorkflowError> {
    let invalid = || WorkflowError::InvalidModuleLabel {
        label: label.to_owned(),
        reason: "expected // followed by non-empty '/'-separated components and a .star suffix"
            .into(),
    };
    if !label.starts_with("//") || label.contains('\\') || !label.ends_with(".star") {
        return Err(invalid());
    }
    let path = PathBuf::from(&label[2..]);
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(invalid());
    }
    Ok(path)
}

fn tool_error(error: impl std::fmt::Display) -> ToolError {
    ToolError::new(ToolErrorKind::Execution, error.to_string())
}

fn invalid_request(message: impl Into<String>) -> ToolError {
    ToolError::new(ToolErrorKind::InvalidArguments, message)
}

fn host_input_error(error: impl std::fmt::Display) -> ToolError {
    ToolError::new(ToolErrorKind::Execution, error.to_string())
}
