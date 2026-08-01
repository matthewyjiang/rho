use std::{
    collections::BTreeMap,
    io::{self, IsTerminal, Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{
    cli::{Cli, WorkflowCommand, WorkflowDocumentFormat, WorkflowRunFormat},
    workflow::{
        derive_workflow_outcome, CollectedSources, Diagnostic, InputName, PlanningLimits,
        PlanningMeasurements, SourceManifest, StarlarkPlanner, StoredPlan, StoredRun,
        WorkflowError, WorkflowResult, WorkflowService, WorkflowStore, WorkflowValue,
    },
};

#[cfg(test)]
use crate::workflow::{PlanConsent, RunLifecycle};

use super::{
    automation, automation_protocol::TerminalReason, bootstrap::host_capabilities, cli_config,
    config_repository::ConfigRepository,
};

#[cfg(test)]
use super::workflow_runtime::WorkflowRunner;

#[path = "workflow_cli/cancel.rs"]
mod cancel;
#[path = "workflow_cli/ops.rs"]
mod ops;
#[path = "workflow_cli/plan_host.rs"]
mod plan_host;
#[path = "workflow_cli/runtime.rs"]
mod runtime;
#[path = "workflow_cli/runtime_present.rs"]
mod runtime_present;
#[path = "workflow_cli/runtime_tui.rs"]
mod runtime_tui;
#[path = "workflow_cli/tool_service.rs"]
mod tool_service;

use cancel::run_cancel;
#[cfg(test)]
use cancel::{cancellation_state, wait_for_cancellation_ack};
pub(super) use cancel::{request_cancellation, CancellationState};
pub(crate) use ops::{freeze_planned_workflow, PreparedPlan, WorkflowOps};
#[cfg(test)]
pub(super) use plan_host::{resolve_nodes_with_host, AuthorizedPlanHost};
pub(crate) use runtime::spawn_background_run;
pub(crate) use runtime_tui::watch_run;
pub(super) use tool_service::workflow_tool_service;

const PLANNER_WORKER_ENV: &str = "RHO_WORKFLOW_PLANNER_WORKER";
// Receipt: a 256-bit bearer token gives the internal one-shot channel 256 bits of entropy.
const PLANNER_TOKEN_BYTES: usize = 32;
// Receipts: limit_receipt.json planner_process.request_frame_bytes,
// response_frame_bytes, stderr_bytes, and address_space_bytes. Reproduce them
// with scripts/measure_workflow_limits.py.
const PLANNER_REQUEST_FRAME_BYTES: u64 = 16 * 1024 * 1024;
pub(super) const PLANNER_RESPONSE_FRAME_BYTES: u64 = 16 * 1024 * 1024;
const PLANNER_STDERR_BYTES: usize = 64 * 1024;
#[cfg(any(unix, windows))]
const PLANNER_ADDRESS_SPACE_BYTES: u64 = 16 * 64 * 1024 * 1024;
const WORKFLOW_WIRE_VERSION: u32 = 1;

pub(super) fn planner_worker_requested(cli: &Cli) -> bool {
    matches!(
        &cli.command,
        Some(crate::cli::Command::WorkflowPlannerWorker)
    ) && std::env::var(PLANNER_WORKER_ENV).is_ok_and(|token| valid_planner_token(&token))
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
        WorkflowCommand::Cancel { run_id } => run_cancel(run_id).await,
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
    let ops = WorkflowOps::open(std::env::current_dir()?, cli.config.clone())?;
    let stored = ops.store_plan(&prepared)?;
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

async fn prepare_plan(
    file: &Path,
    inputs: &[String],
    cli: &Cli,
) -> anyhow::Result<ops::PreparedPlan> {
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
    let available_tools =
        host_capabilities(cli, &config, super::agent_binding::AgentRole::Workflow);
    let limits = planning_limits()?;
    let ops = WorkflowOps::open(workspace, cli.config.clone())?;
    ops.prepare_local(file, supplied_inputs, &config, &available_tools, &limits)
        .await
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

mod planner_worker;

pub(crate) use planner_worker::planning_limits;
#[cfg(test)]
use planner_worker::read_frame_sync;
pub(crate) use planner_worker::run_planner_worker;
use planner_worker::valid_planner_token;

async fn run_frozen_plan(
    prefix: &str,
    yes: bool,
    output: Option<WorkflowRunFormat>,
    config_path: Option<PathBuf>,
) -> anyhow::Result<()> {
    #[cfg(debug_assertions)]
    if run_matrix_tui(crate::tui::workflow::MatrixWorkflowStart::Run, output).await? {
        return Ok(());
    }
    let ops = WorkflowOps::open(std::env::current_dir()?, config_path.clone())?;
    let plan = ops.prepare_run(prefix)?;
    confirm_exact_plan(
        yes,
        &format!(
            "run workflow plan {} ({})",
            plan.manifest.plan_id, plan.manifest.graph_digest.0
        ),
    )?;
    let run = ops.create_confirmed_run(&plan)?;
    runtime::execute_run(
        run,
        super::workflow_runtime::RecoveryDecision::NormalResume,
        output,
        config_path,
    )
    .await
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
    let ops = WorkflowOps::open(std::env::current_dir()?, None)?;
    let run = ops.load_run_prefix(prefix)?;
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
            for (node, completion) in &run.state.state.completions {
                for (kind, artifact) in completion.artifacts.iter() {
                    println!(
                        "artifact {node} {kind:?}: {}",
                        serde_json::to_string(artifact)?
                    );
                }
            }
            if let Some(outcome) = document.outcome {
                println!("outcome: {:?}", outcome);
            }
            Ok(())
        }
    }
}

async fn run_resume(
    prefix: &str,
    yes: bool,
    recover_uncertain: bool,
    output: Option<WorkflowRunFormat>,
    config_path: Option<PathBuf>,
) -> anyhow::Result<()> {
    #[cfg(debug_assertions)]
    if run_matrix_tui(crate::tui::workflow::MatrixWorkflowStart::Resume, output).await? {
        return Ok(());
    }
    let ops = WorkflowOps::open(std::env::current_dir()?, config_path.clone())?;
    let run = ops.load_run_prefix(prefix)?;
    let recovery = ops.prepare_resume(&run, recover_uncertain)?;
    confirm_exact_plan(
        yes,
        &format!(
            "resume workflow run {} ({})",
            run.manifest.run_id, run.manifest.graph_digest.0
        ),
    )?;
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

pub(super) fn workflow_service() -> anyhow::Result<WorkflowService> {
    Ok(WorkflowService::new(WorkflowStore::new(
        &crate::paths::rho_dir()?,
    )?))
}

pub(super) fn write_json_document(value: &impl Serialize) -> anyhow::Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer(&mut output, value)?;
    output.write_all(b"\n")?;
    Ok(())
}

pub(super) fn diagnostic_for_error(error: &anyhow::Error) -> Diagnostic {
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
