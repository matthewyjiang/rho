//! `workflow plan` and `workflow status` documents.
//!
//! JSON documents serialize the stored records and also carry the single-graph
//! field names that earlier releases emitted (`graph_digest`, `graph`, and flat
//! node maps in run state), so existing consumers keep working.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use crate::{
    cli::WorkflowDocumentFormat,
    workflow::{
        LegacyRun, LegacyWorkflowState, NodeId, ScopeState, StoredPlan, StoredRun, TaskInstanceId,
        WorkflowOutcome, WorkflowState,
    },
};

use super::{ops::RunRecord, WorkflowOps};

#[path = "documents_compat.rs"]
mod compat;

pub(super) fn write_plan(plan: &StoredPlan, output: WorkflowDocumentFormat) -> anyhow::Result<()> {
    match output {
        WorkflowDocumentFormat::Json => {
            compat::write_plan_json(std::io::stdout().lock(), plan)?;
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
            compat::write_status_json(std::io::stdout().lock(), run)?;
            println!();
            Ok(())
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
                    nodes: task_entries(state, |scope| &scope.nodes),
                    command_exits: task_entries(state, |scope| &scope.command_exits),
                    outputs: task_entries(state, |scope| &scope.outputs),
                    artifacts: task_entries(state, |scope| &scope.completions)
                        .into_iter()
                        .flat_map(|(node, completion)| {
                            completion
                                .artifacts
                                .iter()
                                .map(move |(kind, artifact)| (node.clone(), kind.label(), artifact))
                        })
                        .map(|(node, kind, artifact)| {
                            Ok((node, kind.to_owned(), serde_json::to_value(artifact)?))
                        })
                        .collect::<anyhow::Result<_>>()?,
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
        WorkflowDocumentFormat::Json => {
            write_legacy_status_json(std::io::stdout().lock(), run)?;
            println!();
            Ok(())
        }
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
                    artifacts: state
                        .completions
                        .iter()
                        .flat_map(|(node, completion)| {
                            [
                                ("stdout", "stdout"),
                                ("stderr", "stderr"),
                                ("answer", "answer"),
                                ("structured_output", "structured output"),
                                ("command_outcome", "command outcome"),
                            ]
                            .into_iter()
                            .filter_map(move |(key, label)| {
                                completion
                                    .get("artifacts")?
                                    .get(key)
                                    .filter(|value| !value.is_null())
                                    .map(|artifact| {
                                        (node.to_string(), label.to_owned(), artifact.clone())
                                    })
                            })
                        })
                        .collect(),
                },
                state.outcome,
            )?;
            println!("read-only: saved by an older Rho release");
            Ok(())
        }
    }
}

fn write_legacy_status_json(writer: impl std::io::Write, run: &LegacyRun) -> anyhow::Result<()> {
    serde_json::to_writer_pretty(
        writer,
        &StatusDocument {
            run,
            outcome: run.state.state.outcome,
        },
    )?;
    Ok(())
}

/// Text status rows keyed by the printed task ID.
struct StatusLines<'a, N, E, O> {
    lifecycle: &'a str,
    revision: u64,
    cancellation_requested: bool,
    nodes: Vec<(String, &'a N)>,
    command_exits: Vec<(String, &'a E)>,
    outputs: Vec<(String, &'a O)>,
    artifacts: Vec<(String, String, Value)>,
}

fn print_status_header(run_id: &str, plan_id: &str, digest: &str) {
    println!("run id: {run_id}");
    println!("plan id: {plan_id}");
    println!("digest: {digest}");
}

fn print_status_body<N: Serialize, E: Serialize, O: std::fmt::Display>(
    lines: &StatusLines<'_, N, E, O>,
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
    for (node, kind, artifact) in &lines.artifacts {
        println!(
            "artifact {node} {}: {}",
            kind,
            serde_json::to_string(artifact)?
        );
    }
    if let Some(outcome) = outcome {
        println!("outcome: {}", outcome.as_str());
    }
    Ok(())
}

fn named<T>(entries: &BTreeMap<String, T>) -> Vec<(String, &T)> {
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

#[cfg(test)]
#[path = "documents_tests.rs"]
mod tests;
