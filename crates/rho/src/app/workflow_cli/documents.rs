//! `workflow plan` and `workflow status` documents.
//!
//! JSON documents serialize the stored records and also carry the single-graph
//! field names that earlier releases emitted (`graph_digest`, `graph`, and flat
//! node maps in run state), so existing consumers keep working.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::{
    cli::WorkflowDocumentFormat,
    workflow::{
        CommandExit, FrozenWorkflow, LegacyRun, LegacyWorkflowState, NodeCompletion, NodeId,
        NodeState, ScopeState, StoredPlan, StoredRun, TaskInstanceId, WorkflowOutcome,
        WorkflowState, WorkflowValue,
    },
};

use super::{ops::RunRecord, write_json_document, WorkflowOps};

pub(super) fn write_plan(plan: &StoredPlan, output: WorkflowDocumentFormat) -> anyhow::Result<()> {
    match output {
        WorkflowDocumentFormat::Json => {
            let mut document = serde_json::to_value(plan)?;
            insert_single_graph_fields(&mut document, &plan.graph);
            write_json_document(&document)
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
struct StatusDocument<T> {
    run: T,
    outcome: Option<WorkflowOutcome>,
}

pub(super) fn run_status(prefix: &str, output: WorkflowDocumentFormat) -> anyhow::Result<()> {
    let ops = WorkflowOps::open(std::env::current_dir()?, None)?;
    match ops.load_run_record(prefix)? {
        RunRecord::Current(run) => write_current_status(&run, output),
        RunRecord::Legacy(run) => write_legacy_status(&run, output),
    }
}

fn write_current_status(run: &StoredRun, output: WorkflowDocumentFormat) -> anyhow::Result<()> {
    let state = &run.state.state;
    let outcome = state.outcome();
    match output {
        WorkflowDocumentFormat::Json => {
            let mut document = serde_json::to_value(run)?;
            insert_single_graph_fields(&mut document, &run.graph);
            insert_flat_state_fields(&mut document, state)?;
            write_json_document(&StatusDocument {
                run: document,
                outcome,
            })
        }
        WorkflowDocumentFormat::Text => {
            print_status_header(
                &run.manifest.run_id.to_string(),
                &run.manifest.plan_id.to_string(),
                &run.manifest.program_digest.0,
            );
            print_status_body(
                &StatusLines {
                    lifecycle: state.lifecycle.as_str(),
                    revision: state.revision,
                    cancellation_requested: state.cancellation_requested,
                    nodes: state
                        .tasks()
                        .map(|(id, node)| (id.to_string(), node))
                        .collect(),
                    command_exits: task_entries(state, |scope| &scope.command_exits),
                    outputs: task_entries(state, |scope| &scope.outputs),
                    completions: task_entries(state, |scope| &scope.completions),
                },
                outcome,
            )?;
            for (scope, scope_state) in &state.scopes {
                if let Some(result) = &scope_state.result {
                    println!("scope result {scope}: {}", serde_json::to_string(result)?);
                }
            }
            Ok(())
        }
    }
}

fn write_legacy_status(run: &LegacyRun, output: WorkflowDocumentFormat) -> anyhow::Result<()> {
    let state: &LegacyWorkflowState = &run.state.state;
    match output {
        WorkflowDocumentFormat::Json => write_json_document(&StatusDocument {
            run,
            outcome: state.outcome,
        }),
        WorkflowDocumentFormat::Text => {
            print_status_header(
                &run.manifest.run_id.to_string(),
                &run.manifest.plan_id.to_string(),
                &run.manifest.graph_digest.0,
            );
            print_status_body(
                &StatusLines {
                    lifecycle: state.lifecycle.as_str(),
                    revision: state.revision,
                    cancellation_requested: state.cancellation_requested,
                    nodes: named(&state.nodes),
                    command_exits: named(&state.command_exits),
                    outputs: named(&state.outputs),
                    completions: named(&state.completions),
                },
                state.outcome,
            )?;
            println!("read-only: saved by an older Rho release");
            Ok(())
        }
    }
}

/// Text status rows keyed by the printed task ID.
struct StatusLines<'a> {
    lifecycle: &'a str,
    revision: u64,
    cancellation_requested: bool,
    nodes: Vec<(String, &'a NodeState)>,
    command_exits: Vec<(String, &'a CommandExit)>,
    outputs: Vec<(String, &'a WorkflowValue)>,
    completions: Vec<(String, &'a NodeCompletion)>,
}

fn print_status_header(run_id: &str, plan_id: &str, digest: &str) {
    println!("run id: {run_id}");
    println!("plan id: {plan_id}");
    println!("digest: {digest}");
}

fn print_status_body(
    lines: &StatusLines<'_>,
    outcome: Option<WorkflowOutcome>,
) -> anyhow::Result<()> {
    println!("lifecycle: {}", lines.lifecycle);
    println!("revision: {}", lines.revision);
    println!("cancellation requested: {}", lines.cancellation_requested);
    for (node, state) in &lines.nodes {
        println!("node {node}: {}", serde_json::to_string(state)?);
    }
    for (node, exit) in &lines.command_exits {
        println!("command exit {node}: {}", serde_json::to_string(exit)?);
    }
    for (node, value) in &lines.outputs {
        println!("output {node}: {value}");
    }
    for (node, completion) in &lines.completions {
        for (kind, artifact) in completion.artifacts.iter() {
            println!(
                "artifact {node} {}: {}",
                kind.label(),
                serde_json::to_string(artifact)?
            );
        }
    }
    if let Some(outcome) = outcome {
        println!("outcome: {}", outcome.as_str());
    }
    Ok(())
}

fn named<T>(entries: &BTreeMap<NodeId, T>) -> Vec<(String, &T)> {
    entries
        .iter()
        .map(|(node, value)| (node.to_string(), value))
        .collect()
}

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

/// Adds `graph_digest` next to `program_digest` and the flat `graph` view of
/// the root scope, matching the single-graph document shape.
///
// NEXT_MAJOR(rho-coding-agent): stop emitting graph_digest and graph in workflow plan and status JSON; program_digest and program replace them.
fn insert_single_graph_fields(document: &mut Value, frozen: &FrozenWorkflow) {
    let digest = Value::String(frozen.program_digest.0.clone());
    if let Some(manifest) = document.get_mut("manifest").and_then(Value::as_object_mut) {
        manifest.insert("graph_digest".into(), digest.clone());
        if let Some(consent) = manifest.get_mut("consent").and_then(Value::as_object_mut) {
            consent.insert("graph_digest".into(), digest.clone());
        }
    }
    if let Some(graph) = document.get_mut("graph").and_then(Value::as_object_mut) {
        graph.insert("graph_digest".into(), digest);
        graph.insert(
            "graph".into(),
            json!({
                "name": frozen.program.name,
                "nodes": frozen.program.root.nodes,
            }),
        );
    }
}

/// Adds the root scope's node maps and run outcome at the top of run state,
/// where single-graph releases stored them.
///
// NEXT_MAJOR(rho-coding-agent): stop emitting flat node, output, command-exit, completion, and outcome fields in workflow status JSON state; scopes replace them.
fn insert_flat_state_fields(document: &mut Value, state: &WorkflowState) -> anyhow::Result<()> {
    let Some(target) = document
        .pointer_mut("/state/state")
        .and_then(Value::as_object_mut)
    else {
        return Ok(());
    };
    let root = state.root_scope();
    let mut flat = Map::new();
    flat.insert("outcome".into(), serde_json::to_value(state.outcome())?);
    flat.insert("nodes".into(), serde_json::to_value(&root.nodes)?);
    flat.insert(
        "command_exits".into(),
        serde_json::to_value(&root.command_exits)?,
    );
    flat.insert("outputs".into(), serde_json::to_value(&root.outputs)?);
    flat.insert(
        "completions".into(),
        serde_json::to_value(&root.completions)?,
    );
    target.extend(flat);
    Ok(())
}

#[cfg(test)]
#[path = "documents_tests.rs"]
mod tests;
