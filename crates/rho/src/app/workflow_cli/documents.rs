//! `workflow plan` and `workflow status` documents.
//!
//! JSON documents serialize the stored records as they are: plan JSON is the
//! stored plan, and status JSON wraps the stored run with its root outcome.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use crate::{
    cli::WorkflowDocumentFormat,
    workflow::{
        NodeId, ScopeState, StoredPlan, StoredRun, TaskInstanceId, WorkflowOutcome, WorkflowState,
    },
};

use super::WorkflowOps;

pub(super) fn write_plan(plan: &StoredPlan, output: WorkflowDocumentFormat) -> anyhow::Result<()> {
    match output {
        WorkflowDocumentFormat::Json => {
            serde_json::to_writer_pretty(std::io::stdout().lock(), plan)?;
            println!();
            Ok(())
        }
        WorkflowDocumentFormat::Text => {
            println!("plan id: {}", plan.manifest.plan_id);
            println!("plan digest: {}", plan.manifest.program_digest.0);
            println!("workspace: {}", plan.manifest.workspace_identity);
            println!("workflow: {}", plan.graph.program.name);
            println!("authorities and frozen program:");
            println!("{}", serde_json::to_string_pretty(&plan.graph)?);
            Ok(())
        }
    }
}

#[derive(Serialize)]
struct StatusDocument<'a> {
    run: &'a StoredRun,
    outcome: Option<WorkflowOutcome>,
}

pub(super) fn run_status(prefix: &str, output: WorkflowDocumentFormat) -> anyhow::Result<()> {
    let ops = WorkflowOps::open(
        std::env::current_dir()?,
        /*config_path*/ None,
        &crate::session_environment::Ambient::ThisProcess,
    )?;
    let run = ops.load_run_prefix(prefix)?;
    let state = &run.state.state;
    let outcome = state.outcome();
    match output {
        WorkflowDocumentFormat::Json => {
            serde_json::to_writer_pretty(
                std::io::stdout().lock(),
                &StatusDocument { run: &run, outcome },
            )?;
            println!();
            Ok(())
        }
        WorkflowDocumentFormat::Text => {
            println!("run id: {}", run.manifest.run_id);
            println!("plan id: {}", run.manifest.plan_id);
            println!("digest: {}", run.manifest.program_digest.0);
            println!("lifecycle: {}", state.lifecycle.as_str());
            println!("revision: {}", state.revision);
            println!("cancellation requested: {}", state.cancellation_requested);
            for (node, node_state) in task_entries(state, |scope| &scope.nodes) {
                println!("node {node}: {}", serde_json::to_string(node_state)?);
            }
            for (node, exit) in task_entries(state, |scope| &scope.command_exits) {
                println!("command exit {node}: {}", serde_json::to_string(exit)?);
            }
            for (node, value) in task_entries(state, |scope| &scope.outputs) {
                println!("output {node}: {value}");
            }
            for (node, completion) in task_entries(state, |scope| &scope.completions) {
                for (kind, artifact) in completion.artifacts.iter() {
                    let artifact: Value = serde_json::to_value(artifact)?;
                    println!(
                        "artifact {node} {}: {}",
                        kind.label(),
                        serde_json::to_string(&artifact)?
                    );
                }
            }
            if let Some(outcome) = outcome {
                println!("outcome: {}", outcome.as_str());
            }
            for (scope, scope_state) in &state.scopes {
                if let Some(result) = &scope_state.result {
                    println!("scope result {scope}: {}", serde_json::to_string(result)?);
                }
            }
            Ok(())
        }
    }
}

/// Per-scope task rows keyed by the printed task instance ID.
fn task_entries<'a, T>(
    state: &'a WorkflowState,
    select: impl Fn(&'a ScopeState) -> &'a BTreeMap<NodeId, T>,
) -> Vec<(String, &'a T)> {
    state
        .scopes
        .iter()
        .flat_map(|(scope, scope_state)| {
            select(scope_state).iter().map(move |(node, value)| {
                (TaskInstanceId::new(*scope, node.clone()).to_string(), value)
            })
        })
        .collect()
}
