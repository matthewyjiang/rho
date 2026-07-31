use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, IsTerminal, Read, Write},
    path::Path,
    process::Stdio,
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::{
    agent::{AgentOrigin, PromptPolicy, ToolCapability, BUILTIN_TOOL_CAPABILITIES},
    cli::{Cli, WorkflowCommand, WorkflowDocumentFormat, WorkflowRunFormat},
    workflow::{
        derive_workflow_outcome, normalize_workflow, validate_workflow, CollectedSources,
        Diagnostic, Digest, FreezePlan, FrozenSchedulerSettings, FrozenWorkflow, InputName,
        NodeExecution, PlanConsent, PlannerIdentity, PlanningLimits, PlanningMeasurements,
        ResolvedAgent, ResolvedCommand, ResolvedNode, RunLifecycle, SourceCollector,
        SourceManifest, StarlarkPlanner, StoredPlan, StoredRun, WorkflowError, WorkflowResult,
        WorkflowService, WorkflowStore, WorkflowValue, FROZEN_WORKFLOW_SCHEMA_VERSION,
    },
};

use super::{
    agent_binding::{AgentBinder, AgentInvocation, AgentRole, BoundRuntime},
    automation,
    automation_protocol::TerminalReason,
    bootstrap::host_capabilities,
    cli_config,
    config_repository::ConfigRepository,
    sdk_config,
    workflow_runtime::WorkflowRunner,
};

#[path = "workflow_cli/runtime.rs"]
mod runtime;
#[path = "workflow_cli/tool_service.rs"]
mod tool_service;

pub(super) use tool_service::workflow_tool_service;

const PLANNER_WORKER_ENV: &str = "RHO_WORKFLOW_PLANNER_WORKER";
const PLANNER_FORMAT_VERSION: u32 = 1;
const WORKFLOW_WIRE_VERSION: u32 = 1;
// Receipt: matches agent_executor::DEFAULT_TOTAL_CONCURRENCY. Kind limits
// use the same ceiling and cannot raise total parallel work.
const DEFAULT_PARALLEL_NODES: u32 = 4;
const DEFAULT_PARALLEL_AGENTS: u32 = 4;
const DEFAULT_PARALLEL_COMMANDS: u32 = 4;

pub(super) fn planner_worker_requested() -> bool {
    std::env::var_os(PLANNER_WORKER_ENV).is_some()
}

pub(super) async fn run(command: &WorkflowCommand, cli: &Cli) -> anyhow::Result<()> {
    match command {
        WorkflowCommand::Validate { file, input } => run_validate(file, input, cli).await,
        WorkflowCommand::Plan {
            file,
            input,
            output,
        } => run_plan(file, input, *output, cli).await,
        WorkflowCommand::Run {
            plan_id,
            yes,
            output,
        } => run_frozen_plan(plan_id, *yes, *output, cli.config.clone()).await,
        WorkflowCommand::Status { run_id, output } => run_status(run_id, *output),
        WorkflowCommand::Cancel { run_id } => run_cancel(run_id),
        WorkflowCommand::Resume {
            run_id,
            yes,
            recover_uncertain,
            output,
        } => {
            run_resume(
                run_id,
                *yes,
                *recover_uncertain,
                *output,
                cli.config.clone(),
            )
            .await
        }
    }
}

#[derive(Serialize)]
struct ValidationDocument {
    valid: bool,
    diagnostics: Vec<Diagnostic>,
    source_manifest: Option<SourceManifest>,
    workflow_name: Option<String>,
    node_count: Option<usize>,
}

async fn run_validate(file: &Path, inputs: &[String], cli: &Cli) -> anyhow::Result<()> {
    match prepare_plan(file, inputs, cli).await {
        Ok(prepared) => {
            let document = ValidationDocument {
                valid: true,
                diagnostics: Vec::new(),
                source_manifest: Some(prepared.sources.manifest),
                workflow_name: Some(prepared.workflow.graph.name.to_string()),
                node_count: Some(prepared.workflow.graph.nodes.len()),
            };
            write_validation_document(&document)?;
            Ok(())
        }
        Err(error) => {
            let document = ValidationDocument {
                valid: false,
                diagnostics: vec![diagnostic_for_error(&error)],
                source_manifest: None,
                workflow_name: None,
                node_count: None,
            };
            write_validation_document(&document)?;
            Err(workflow_exit("workflow validation failed"))
        }
    }
}

fn write_validation_document(document: &ValidationDocument) -> anyhow::Result<()> {
    if document.valid {
        println!("valid");
        if let (Some(name), Some(nodes)) = (&document.workflow_name, document.node_count) {
            println!("workflow: {name}");
            println!("nodes: {nodes}");
        }
        return Ok(());
    }
    for diagnostic in &document.diagnostics {
        match &diagnostic.span {
            Some(span) => eprintln!("{}: {}: {}", span, diagnostic.code, diagnostic.message),
            None => eprintln!("{}: {}", diagnostic.code, diagnostic.message),
        }
    }
    Ok(())
}

async fn run_plan(
    file: &Path,
    inputs: &[String],
    output: WorkflowDocumentFormat,
    cli: &Cli,
) -> anyhow::Result<()> {
    let prepared = prepare_plan(file, inputs, cli).await?;
    let service = workflow_service()?;
    let stored = service.freeze_and_store(FreezePlan {
        planner: prepared.workflow.planner,
        sources: prepared.sources.manifest,
        source_bytes: &prepared.sources.sources,
        inputs: prepared.workflow.inputs,
        graph: prepared.workflow.graph,
        resolved_nodes: prepared.workflow.resolved_nodes,
        scheduler: prepared.workflow.scheduler,
        workspace_identity: workspace_identity(&std::env::current_dir()?)?,
    })?;
    write_plan(&stored, output)
}

fn write_plan(plan: &StoredPlan, output: WorkflowDocumentFormat) -> anyhow::Result<()> {
    match output {
        WorkflowDocumentFormat::Json => write_json_document(plan),
        WorkflowDocumentFormat::Text => {
            println!("plan id: {}", plan.manifest.plan_id);
            println!("plan digest: {}", plan.manifest.graph_digest.0);
            println!("workspace: {}", plan.manifest.workspace_identity);
            println!("workflow: {}", plan.graph.graph.name);
            println!("authorities and frozen graph:");
            println!("{}", serde_json::to_string_pretty(&plan.graph)?);
            Ok(())
        }
    }
}

struct PreparedPlan {
    sources: CollectedSources,
    workflow: FrozenWorkflow,
}

async fn prepare_plan(file: &Path, inputs: &[String], cli: &Cli) -> anyhow::Result<PreparedPlan> {
    let workspace = std::env::current_dir()?.canonicalize()?;
    let supplied_inputs = parse_inputs(inputs)?;
    let config_repository = ConfigRepository::new(cli.config.clone());
    let mut config = config_repository.load()?;
    cli_config::apply_overrides(&mut config, cli)?;
    cli_config::normalize_reasoning_for_cli(
        &mut config,
        if cli.reasoning.is_some() {
            rho_providers::model::ReasoningRequestSource::Explicit
        } else {
            rho_providers::model::ReasoningRequestSource::PersistedOrDefault
        },
    )?;
    let available_tools = host_capabilities(cli, &config, AgentRole::Workflow);
    prepare_plan_with_config(file, supplied_inputs, &config, &workspace, &available_tools).await
}

async fn prepare_plan_with_config(
    file: &Path,
    supplied_inputs: BTreeMap<InputName, WorkflowValue>,
    config: &crate::config::Config,
    workspace: &Path,
    available_tools: &crate::agent::AgentCapabilities,
) -> anyhow::Result<PreparedPlan> {
    let limits = planning_limits()?;
    let sources = SourceCollector::new(workspace, &limits)?.collect(file)?;
    prepare_plan_from_sources(
        sources,
        supplied_inputs,
        config,
        workspace,
        available_tools,
        &limits,
    )
    .await
}

async fn prepare_plan_from_sources(
    sources: CollectedSources,
    supplied_inputs: BTreeMap<InputName, WorkflowValue>,
    config: &crate::config::Config,
    workspace: &Path,
    available_tools: &crate::agent::AgentCapabilities,
    limits: &PlanningLimits,
) -> anyhow::Result<PreparedPlan> {
    let planned = run_supervised_planner(&sources, supplied_inputs, limits).await?;
    let resolved_nodes = resolve_nodes(&planned.graph, config, workspace, available_tools)?;
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
    })?;
    validate_workflow(&workflow)?;
    Ok(PreparedPlan { sources, workflow })
}

fn parse_inputs(values: &[String]) -> anyhow::Result<BTreeMap<InputName, WorkflowValue>> {
    let mut inputs = BTreeMap::new();
    for value in values {
        let (name, json) = value
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("invalid --input '{value}': expected KEY=JSON"))?;
        let name = InputName::new(name)?;
        let parsed = serde_json::from_str(json).map_err(|error| {
            anyhow::anyhow!("invalid JSON for workflow input '{name}': {error}")
        })?;
        let parsed = WorkflowValue::from_json(parsed)?;
        if inputs.insert(name.clone(), parsed).is_some() {
            anyhow::bail!("workflow input '{name}' was supplied more than once");
        }
    }
    Ok(inputs)
}

fn planning_limits() -> WorkflowResult<PlanningLimits> {
    // These are the measured acceptance values recorded by the workflow foundation
    // prototype. Keep the receipt with every limit until a config-backed profile exists.
    PlanningLimits::from_measurements(planning_measurements())
}

#[derive(Serialize, Deserialize)]
struct PlannerWorkerRequest {
    entry_label: String,
    sources: BTreeMap<String, String>,
    manifest: SourceManifest,
    inputs: BTreeMap<InputName, WorkflowValue>,
    measurements: PlanningMeasurements,
}

#[derive(Serialize, Deserialize)]
struct PlannerWorkerPlan {
    graph: crate::workflow::WorkflowGraph,
    inputs: BTreeMap<InputName, WorkflowValue>,
}

#[derive(Serialize, Deserialize)]
struct PlannerWorkerResponse {
    plan: Option<PlannerWorkerPlan>,
    error: Option<String>,
}

fn planning_measurements() -> PlanningMeasurements {
    PlanningMeasurements {
        receipt: "workflow foundation prototype acceptance profile".to_owned(),
        total_source_bytes: 1_000_000,
        module_count: 100,
        module_depth: 20,
        evaluator_ticks: 1_000_000,
        evaluator_heap_bytes: 64_000_000,
        call_stack_depth: 100,
        string_bytes: 1_000_000,
        list_items: 10_000,
        dict_items: 10_000,
        input_depth: 20,
        input_bytes: 1_000_000,
        node_count: 1_000,
        edge_count: 10_000,
        condition_depth: crate::workflow::CONDITION_DEPTH_LIMIT as u64,
        schema_depth: 20,
        schema_bytes: 1_000_000,
        graph_bytes: 10_000_000,
        worker_wall_millis: 10_000,
    }
}

async fn run_supervised_planner(
    sources: &CollectedSources,
    inputs: BTreeMap<InputName, WorkflowValue>,
    limits: &PlanningLimits,
) -> anyhow::Result<PlannerWorkerPlan> {
    let request = PlannerWorkerRequest {
        entry_label: sources.entry_label.clone(),
        sources: sources.sources.clone(),
        manifest: sources.manifest.clone(),
        inputs,
        measurements: planning_measurements(),
    };
    let mut child = tokio::process::Command::new(std::env::current_exe()?)
        .args(["workflow", "validate", "worker.star"])
        .env(PLANNER_WORKER_ENV, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    child
        .stdin
        .take()
        .expect("planner worker stdin is piped")
        .write_all(&serde_json::to_vec(&request)?)
        .await?;
    let output = tokio::time::timeout(
        Duration::from_millis(limits.worker_wall_millis.limit),
        child.wait_with_output(),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "{} budget exceeded: accepted limit {}, requested or measured {}",
            limits.worker_wall_millis.name,
            limits.worker_wall_millis.limit,
            limits.worker_wall_millis.limit.saturating_add(1)
        )
    })??;
    if !output.status.success() {
        anyhow::bail!(
            "workflow planner worker failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let response: PlannerWorkerResponse = serde_json::from_slice(&output.stdout)?;
    match (response.plan, response.error) {
        (Some(plan), None) => Ok(plan),
        (None, Some(error)) => anyhow::bail!(error),
        _ => anyhow::bail!("workflow planner worker returned an invalid response"),
    }
}

pub(super) async fn run_planner_worker() -> anyhow::Result<()> {
    let mut bytes = Vec::new();
    io::stdin().read_to_end(&mut bytes)?;
    let request: PlannerWorkerRequest = serde_json::from_slice(&bytes)?;
    let limits = PlanningLimits::from_measurements(request.measurements)?;
    let collected = CollectedSources {
        entry_label: request.entry_label,
        sources: request.sources,
        manifest: request.manifest,
    };
    let response = match StarlarkPlanner::new(&limits).plan_in_process_prototype(
        &collected,
        &request.inputs,
        Arc::new(AtomicBool::new(false)),
    ) {
        Ok(planned) => PlannerWorkerResponse {
            plan: Some(PlannerWorkerPlan {
                graph: planned.graph,
                inputs: planned.inputs,
            }),
            error: None,
        },
        Err(error) => PlannerWorkerResponse {
            plan: None,
            error: Some(error.to_string()),
        },
    };
    serde_json::to_writer(io::stdout().lock(), &response)?;
    Ok(())
}

fn resolve_nodes(
    graph: &crate::workflow::WorkflowGraph,
    config: &crate::config::Config,
    workspace: &Path,
    available_tools: &crate::agent::AgentCapabilities,
) -> anyhow::Result<BTreeMap<crate::workflow::NodeId, ResolvedNode>> {
    let catalog = crate::agent::AgentCatalog::discover(workspace)?;
    graph
        .nodes
        .iter()
        .map(|(id, node)| {
            let resolved = match &node.execution {
                NodeExecution::Agent(agent) => {
                    let entry = catalog.find(&agent.agent)?;
                    let bound = AgentBinder::bind(
                        Arc::new(entry.definition.clone()),
                        AgentInvocation {
                            role: AgentRole::Workflow,
                            available_tools: available_tools.clone(),
                        },
                        config,
                    )?;
                    ResolvedNode::Agent(Box::new(resolve_agent(entry, bound, workspace)?))
                }
                NodeExecution::Command(command) => {
                    let (executable, cwd) = match command {
                        crate::workflow::CommandNode::Direct {
                            executable, cwd, ..
                        }
                        | crate::workflow::CommandNode::Shell {
                            executable, cwd, ..
                        } => (executable, cwd),
                    };
                    let cwd_path = workspace.join(cwd).canonicalize()?;
                    if !cwd_path.starts_with(workspace) {
                        anyhow::bail!("command node '{id}' cwd is outside the workspace");
                    }
                    ResolvedNode::Command(ResolvedCommand {
                        executable: resolve_executable(executable, workspace)?,
                        exact_path: true,
                        cwd: crate::paths::display(&cwd_path),
                        environment_policy: "inherit-current-process".to_owned(),
                    })
                }
            };
            Ok((id.clone(), resolved))
        })
        .collect()
}

fn resolve_agent(
    entry: &crate::agent::AgentCatalogEntry,
    bound: super::agent_binding::BoundAgent,
    workspace: &Path,
) -> anyhow::Result<ResolvedAgent> {
    let source_origin = match entry.metadata.origin {
        AgentOrigin::Internal => "internal",
        AgentOrigin::BuiltIn => "built_in",
        AgentOrigin::AgentsHome => "agents_home",
        AgentOrigin::RhoHome => "rho_home",
        AgentOrigin::Project => "project",
    };
    let source_origin = match &entry.metadata.path {
        Some(path) => format!("{source_origin}:{}", crate::paths::display(path)),
        None => source_origin.to_owned(),
    };
    let prompt_policy = match &entry.definition.prompt {
        PromptPolicy::Extend(text) => format!("extend:{text}"),
        PromptPolicy::Replace(text) => format!("replace:{text}"),
    };
    let permission_ceiling = match bound.runtime() {
        BoundRuntime::Rho { config, .. } => config.permission_mode.to_string(),
        BoundRuntime::ClaudeCli {
            permission_mode, ..
        } => permission_mode.to_string(),
    };
    let common = ResolvedAgent {
        agent_id: entry.definition.id.to_string(),
        fingerprint: entry.fingerprint.to_string(),
        runtime: match bound.runtime() {
            BoundRuntime::Rho { .. } => crate::workflow::AgentRuntime::Rho,
            BoundRuntime::ClaudeCli { .. } => crate::workflow::AgentRuntime::ClaudeCli,
        },
        source_origin,
        trust_required: entry.metadata.origin == AgentOrigin::Project,
        prompt_policy,
        provider: None,
        model: None,
        reasoning: None,
        step_limit: sdk_config::run_step_limit().get() as u64,
        capabilities: BTreeSet::new(),
        permission_ceiling,
        auth_profile: None,
        executable: None,
        arguments: Vec::new(),
    };
    Ok(match bound.runtime() {
        BoundRuntime::Rho {
            config,
            capabilities,
        } => ResolvedAgent {
            provider: Some(config.provider.clone()),
            model: Some(config.model.clone()),
            reasoning: Some(config.reasoning.to_string()),
            capabilities: frozen_capabilities(capabilities),
            auth_profile: Some(config.auth.clone()),
            ..common
        },
        BoundRuntime::ClaudeCli {
            model,
            tools,
            inherit_claude_config,
            permission_mode,
            max_turns,
            effort,
        } => {
            let plan = crate::claude_runtime::spawn::build_spawn_plan(
                &crate::claude_runtime::spawn::ClaudeSpawnRequest {
                    system_prompt: entry.definition.prompt.clone(),
                    model: model.clone(),
                    tools: tools.clone(),
                    inherit_claude_config: *inherit_claude_config,
                    permission_mode: *permission_mode,
                    cwd: workspace.to_path_buf(),
                    max_turns: *max_turns,
                    effort: *effort,
                },
            )?;
            ResolvedAgent {
                model: model.clone(),
                reasoning: effort.map(str::to_owned),
                step_limit: *max_turns,
                capabilities: tools.iter().cloned().collect(),
                executable: Some(resolve_executable("claude", workspace)?),
                arguments: plan.args,
                ..common
            }
        }
    })
}

fn frozen_capabilities(capabilities: &crate::agent::AgentCapabilities) -> BTreeSet<String> {
    BUILTIN_TOOL_CAPABILITIES
        .iter()
        .filter(|capability| capabilities.contains(capability))
        .filter(|capability| {
            !matches!(
                capability,
                ToolCapability::Agent
                    | ToolCapability::Agents
                    | ToolCapability::Questionnaire
                    | ToolCapability::Rho
                    | ToolCapability::Workflow
            )
        })
        .map(|capability| capability.as_str().to_owned())
        .collect()
}

fn resolve_executable(executable: &str, workspace: &Path) -> anyhow::Result<String> {
    let path = Path::new(executable);
    let resolved = if path.components().count() == 1 {
        crate::executable::find_on_path(executable)
            .ok_or_else(|| anyhow::anyhow!("executable '{executable}' was not found on PATH"))?
    } else if path.is_absolute() {
        path.canonicalize()?
    } else {
        workspace.join(path).canonicalize()?
    };
    Ok(crate::paths::display(&resolved))
}

async fn run_frozen_plan(
    prefix: &str,
    yes: bool,
    output: Option<WorkflowRunFormat>,
    config_path: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    #[cfg(debug_assertions)]
    if run_matrix_tui(crate::tui::workflow::MatrixWorkflowStart::Run, output).await? {
        return Ok(());
    }
    let service = workflow_service()?;
    let plan_id = service.store().resolve_plan(prefix)?;
    let plan = service.store().load_plan(plan_id)?;
    recheck_plan(&plan, config_path.clone())?;
    confirm_exact_plan(
        yes,
        &format!(
            "run workflow plan {} ({})",
            plan.manifest.plan_id, plan.manifest.graph_digest.0
        ),
    )?;
    let run = service.create_run(
        &plan,
        PlanConsent {
            graph_digest: plan.manifest.graph_digest.clone(),
            confirmed: true,
        },
    )?;
    runtime::execute_run(
        run,
        super::workflow_runtime::RecoveryDecision::NormalResume,
        output,
        config_path,
    )
    .await
}

fn recheck_plan(plan: &StoredPlan, config_path: Option<std::path::PathBuf>) -> anyhow::Result<()> {
    let current_directory = std::env::current_dir()?;
    let current_workspace = workspace_identity(&current_directory)?;
    if current_workspace != plan.manifest.workspace_identity {
        anyhow::bail!(
            "workflow plan workspace is '{}', but the current workspace is '{}'",
            plan.manifest.workspace_identity,
            current_workspace
        );
    }
    let entry = plan
        .graph
        .sources
        .entry_label
        .strip_prefix("//")
        .ok_or_else(|| anyhow::anyhow!("frozen workflow entry label is invalid"))?;
    let measured_sources = SourceCollector::new(&current_directory, &planning_limits()?)?
        .collect(&current_directory.join(entry))?;
    if measured_sources.manifest != plan.graph.sources {
        anyhow::bail!(
            "workflow source drift was detected for plan {}",
            plan.manifest.plan_id
        );
    }
    let config = ConfigRepository::new(config_path).load()?;
    for agent in plan
        .graph
        .resolved_nodes
        .values()
        .filter_map(|node| match node {
            ResolvedNode::Agent(agent) => Some(agent.as_ref()),
            ResolvedNode::Command(_) => None,
        })
    {
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

fn recheck_run(run: &StoredRun, config_path: Option<std::path::PathBuf>) -> anyhow::Result<()> {
    let service = workflow_service()?;
    let plan = service.store().load_plan(run.manifest.plan_id)?;
    if plan.manifest.graph_digest != run.manifest.graph_digest {
        anyhow::bail!("workflow run plan digest does not match its frozen graph");
    }
    recheck_plan(&plan, config_path)
}

fn permission_rank(value: &str) -> Option<u8> {
    match value {
        "plan" => Some(0),
        "supervised" => Some(1),
        "auto" => Some(2),
        _ => None,
    }
}

fn workspace_identity(path: &Path) -> anyhow::Result<String> {
    Ok(crate::paths::display(&path.canonicalize()?))
}

fn confirm_exact_plan(yes: bool, action: &str) -> anyhow::Result<()> {
    let terminal = io::stdin().is_terminal() && io::stderr().is_terminal();
    match confirmation_requirement(yes, terminal) {
        ConfirmationRequirement::Confirmed => Ok(()),
        ConfirmationRequirement::FlagRequired => {
            anyhow::bail!("workflow confirmation requires --yes outside an interactive terminal")
        }
        ConfirmationRequirement::Prompt => {
            eprint!("Confirm {action}? [y/N] ");
            io::stderr().flush()?;
            let mut response = String::new();
            io::stdin().read_line(&mut response)?;
            if matches!(response.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                Ok(())
            } else {
                anyhow::bail!("workflow was not confirmed")
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConfirmationRequirement {
    Confirmed,
    Prompt,
    FlagRequired,
}

fn confirmation_requirement(yes: bool, terminal: bool) -> ConfirmationRequirement {
    if yes {
        ConfirmationRequirement::Confirmed
    } else if terminal {
        ConfirmationRequirement::Prompt
    } else {
        ConfirmationRequirement::FlagRequired
    }
}

#[derive(Serialize)]
struct StatusDocument<'a> {
    run: &'a StoredRun,
    outcome: Option<crate::workflow::WorkflowOutcome>,
}

fn run_status(prefix: &str, output: WorkflowDocumentFormat) -> anyhow::Result<()> {
    let service = workflow_service()?;
    let run_id = service.store().resolve_run(prefix)?;
    let run = service.store().load_run(run_id)?;
    let document = StatusDocument {
        outcome: derive_workflow_outcome(&run.graph, &run.state.state),
        run: &run,
    };
    match output {
        WorkflowDocumentFormat::Json => write_json_document(&document),
        WorkflowDocumentFormat::Text => {
            println!("run id: {}", run.manifest.run_id);
            println!("plan id: {}", run.manifest.plan_id);
            println!("digest: {}", run.manifest.graph_digest.0);
            println!("lifecycle: {:?}", run.state.state.lifecycle);
            println!("revision: {}", run.state.state.revision);
            println!(
                "cancellation requested: {}",
                run.state.state.cancellation_requested
            );
            for (node, state) in &run.state.state.nodes {
                println!("node {node}: {}", serde_json::to_string(state)?);
            }
            for (node, exit) in &run.state.state.command_exits {
                println!("command exit {node}: {}", serde_json::to_string(exit)?);
            }
            for (node, value) in &run.state.state.outputs {
                println!("output {node}: {value}");
            }
            if let Some(outcome) = document.outcome {
                println!("outcome: {:?}", outcome);
            }
            Ok(())
        }
    }
}

#[derive(Serialize)]
struct CancelDocument {
    run_id: String,
    cancellation_requested: bool,
    owner_acknowledged: bool,
    lifecycle: RunLifecycle,
}

fn run_cancel(prefix: &str) -> anyhow::Result<()> {
    let service = workflow_service()?;
    let run_id = service.store().resolve_run(prefix)?;
    let run = service.store().load_run(run_id)?;
    if run.state.state.lifecycle == RunLifecycle::Completed {
        return write_json_document(&CancelDocument {
            run_id: run_id.to_string(),
            cancellation_requested: run.state.state.cancellation_requested,
            owner_acknowledged: true,
            lifecycle: RunLifecycle::Completed,
        });
    }
    WorkflowRunner::request_cross_process_cancel(&crate::paths::rho_dir()?, run_id)?;
    write_json_document(&CancelDocument {
        run_id: run_id.to_string(),
        cancellation_requested: true,
        owner_acknowledged: run.state.state.cancellation_requested,
        lifecycle: run.state.state.lifecycle,
    })
}

async fn run_resume(
    prefix: &str,
    yes: bool,
    recover_uncertain: bool,
    output: Option<WorkflowRunFormat>,
    config_path: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    #[cfg(debug_assertions)]
    if run_matrix_tui(crate::tui::workflow::MatrixWorkflowStart::Resume, output).await? {
        return Ok(());
    }
    let service = workflow_service()?;
    let run_id = service.store().resolve_run(prefix)?;
    let run = service.store().load_run(run_id)?;
    recheck_run(&run, config_path.clone())?;
    confirm_exact_plan(
        yes,
        &format!(
            "resume workflow run {} ({})",
            run.manifest.run_id, run.manifest.graph_digest.0
        ),
    )?;
    if run.state.state.lifecycle == RunLifecycle::NeedsRecovery && !recover_uncertain {
        anyhow::bail!(
            "workflow run {} has uncertain attempts; confirm that no prior process remains, then use --recover-uncertain",
            run.manifest.run_id
        );
    }
    let recovery = if recover_uncertain {
        super::workflow_runtime::RecoveryDecision::ConfirmNoProcess
    } else {
        super::workflow_runtime::RecoveryDecision::NormalResume
    };
    runtime::execute_run(run, recovery, output, config_path).await
}

#[cfg(debug_assertions)]
async fn run_matrix_tui(
    start: crate::tui::workflow::MatrixWorkflowStart,
    output: Option<WorkflowRunFormat>,
) -> anyhow::Result<bool> {
    if output.is_none()
        && std::env::var_os("RHO_TUI_TEST_MODE").as_deref() == Some(std::ffi::OsStr::new("matrix"))
    {
        crate::tui::workflow::run(crate::tui::workflow::matrix_adapter(start)).await?;
        return Ok(true);
    }
    Ok(false)
}

fn workflow_service() -> anyhow::Result<WorkflowService> {
    Ok(WorkflowService::new(WorkflowStore::new(
        &crate::paths::rho_dir()?,
    )?))
}

fn write_json_document(value: &impl Serialize) -> anyhow::Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer(&mut output, value)?;
    output.write_all(b"\n")?;
    Ok(())
}

fn diagnostic_for_error(error: &anyhow::Error) -> Diagnostic {
    if let Some(error) = error.downcast_ref::<WorkflowError>() {
        let code = match error {
            WorkflowError::InvalidId { .. } => "invalid_id",
            WorkflowError::BudgetExceeded { .. } => "budget_exceeded",
            WorkflowError::Cycle { .. } => "cycle",
            WorkflowError::MissingDependency { .. } => "missing_dependency",
            WorkflowError::NonAncestorReference { .. } => "non_ancestor_reference",
            WorkflowError::InvalidAccess { .. } => "invalid_access",
            WorkflowError::Schema { .. } => "schema",
            WorkflowError::Condition(_) => "condition",
            WorkflowError::IllegalTransition { .. } => "illegal_transition",
            WorkflowError::Scheduler(_) => "scheduler",
            WorkflowError::InvalidModuleLabel { .. } => "invalid_module_label",
            WorkflowError::SourceOutsideRoot { .. } => "source_outside_root",
            WorkflowError::SourceSymlink { .. } => "source_symlink",
            WorkflowError::ImportCycle { .. } => "import_cycle",
            WorkflowError::MissingWorkflow => "missing_workflow",
            WorkflowError::UnsupportedValue { .. } => "unsupported_value",
            WorkflowError::MissingInput(_) => "missing_input",
            WorkflowError::UnknownInput(_) => "unknown_input",
            WorkflowError::InvalidInput { .. } => "invalid_input",
            WorkflowError::Corrupt { .. } => "corrupt",
            WorkflowError::UnsupportedVersion { .. } => "unsupported_version",
            WorkflowError::AmbiguousId { .. } => "ambiguous_id",
            WorkflowError::UnknownId(_) => "unknown_id",
            WorkflowError::UntrustedDirectory(_) => "untrusted_directory",
            WorkflowError::Io(_) => "io",
            WorkflowError::Json(_) => "json",
            WorkflowError::Starlark(_) => "starlark",
        };
        let span = None;
        Diagnostic {
            code: code.to_owned(),
            message: error.to_string(),
            span,
        }
    } else {
        Diagnostic {
            code: "workflow_cli".to_owned(),
            message: error.to_string(),
            span: None,
        }
    }
}

fn workflow_exit(message: &str) -> anyhow::Error {
    automation::AutomationExit::new(2, TerminalReason::ConfigurationError, message.to_owned())
        .into()
}

#[cfg(test)]
#[path = "workflow_cli_tests.rs"]
mod tests;
