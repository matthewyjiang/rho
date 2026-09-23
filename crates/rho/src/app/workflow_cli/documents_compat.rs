//! Typed single-graph compatibility views. Never edit serialized JSON by path.
use std::collections::BTreeMap;

use serde::Serialize;

use crate::workflow::{
    CommandExit, Digest, FrozenWorkflow, Node, NodeCompletion, NodeId, NodeState, PlanConsent,
    PlanId, PlanManifest, RunId, RunManifest, RunStateRecord, StoredPlan, StoredRun, WorkflowName,
    WorkflowOutcome, WorkflowState, WorkflowValue,
};

// NEXT_MAJOR(rho-coding-agent): stop emitting graph_digest and graph in workflow plan and status JSON; program_digest and program replace them.
#[derive(Serialize)]
struct WithDigest<'a, T> {
    #[serde(flatten)]
    current: &'a T,
    graph_digest: &'a Digest,
}

#[derive(Serialize)]
struct FlatGraph<'a> {
    name: &'a WorkflowName,
    nodes: &'a BTreeMap<NodeId, Node>,
}

#[derive(Serialize)]
struct GraphDocument<'a> {
    #[serde(flatten)]
    current: &'a FrozenWorkflow,
    graph_digest: &'a Digest,
    graph: FlatGraph<'a>,
}

impl<'a> From<&'a FrozenWorkflow> for GraphDocument<'a> {
    fn from(frozen: &'a FrozenWorkflow) -> Self {
        Self {
            current: frozen,
            graph_digest: &frozen.program_digest,
            graph: FlatGraph {
                name: &frozen.program.name,
                nodes: &frozen.program.root.nodes,
            },
        }
    }
}

#[derive(Serialize)]
struct PlanDocument<'a> {
    manifest: WithDigest<'a, PlanManifest>,
    graph: GraphDocument<'a>,
}

#[derive(Serialize)]
struct RunManifestDocument<'a> {
    schema_version: u32,
    run_id: RunId,
    created_at_unix_nanos: u64,
    plan_id: PlanId,
    program_digest: &'a Digest,
    graph_digest: &'a Digest,
    workspace_identity: &'a str,
    consent: WithDigest<'a, PlanConsent>,
    name: &'a str,
    step_count: usize,
}

impl<'a> From<&'a RunManifest> for RunManifestDocument<'a> {
    fn from(manifest: &'a RunManifest) -> Self {
        let RunManifest {
            schema_version,
            run_id,
            created_at_unix_nanos,
            plan_id,
            program_digest,
            workspace_identity,
            consent,
            name,
            step_count,
        } = manifest;
        Self {
            schema_version: *schema_version,
            run_id: *run_id,
            created_at_unix_nanos: *created_at_unix_nanos,
            plan_id: *plan_id,
            program_digest,
            graph_digest: program_digest,
            workspace_identity,
            consent: WithDigest {
                current: consent,
                graph_digest: &consent.program_digest,
            },
            name,
            step_count: *step_count,
        }
    }
}

// NEXT_MAJOR(rho-coding-agent): stop emitting flat node, output, command-exit, completion, and outcome fields in workflow status JSON state; scopes replace them.
#[derive(Serialize)]
struct StateDocument<'a> {
    #[serde(flatten)]
    current: &'a WorkflowState,
    outcome: Option<WorkflowOutcome>,
    nodes: &'a BTreeMap<NodeId, NodeState>,
    command_exits: &'a BTreeMap<NodeId, CommandExit>,
    outputs: &'a BTreeMap<NodeId, WorkflowValue>,
    completions: &'a BTreeMap<NodeId, NodeCompletion>,
}

#[derive(Serialize)]
struct StateRecordDocument<'a> {
    schema_version: u32,
    last_event_sequence: u64,
    state: StateDocument<'a>,
}

#[derive(Serialize)]
struct RunDocument<'a> {
    manifest: RunManifestDocument<'a>,
    graph: GraphDocument<'a>,
    state: StateRecordDocument<'a>,
}

pub(super) fn write_plan_json(
    writer: impl std::io::Write,
    plan: &StoredPlan,
) -> anyhow::Result<()> {
    let StoredPlan { manifest, graph } = plan;
    serde_json::to_writer_pretty(
        writer,
        &PlanDocument {
            manifest: WithDigest {
                current: manifest,
                graph_digest: &manifest.program_digest,
            },
            graph: graph.into(),
        },
    )?;
    Ok(())
}

pub(super) fn write_status_json(
    writer: impl std::io::Write,
    run: &StoredRun,
) -> anyhow::Result<()> {
    let StoredRun {
        manifest,
        graph,
        state,
    } = run;
    let RunStateRecord {
        schema_version,
        last_event_sequence,
        state,
    } = state;
    let root = state.root_scope();
    serde_json::to_writer_pretty(
        writer,
        &super::StatusDocument {
            run: RunDocument {
                manifest: manifest.into(),
                graph: graph.into(),
                state: StateRecordDocument {
                    schema_version: *schema_version,
                    last_event_sequence: *last_event_sequence,
                    state: StateDocument {
                        current: state,
                        outcome: state.outcome(),
                        nodes: &root.nodes,
                        command_exits: &root.command_exits,
                        outputs: &root.outputs,
                        completions: &root.completions,
                    },
                },
            },
            outcome: state.outcome(),
        },
    )?;
    Ok(())
}
