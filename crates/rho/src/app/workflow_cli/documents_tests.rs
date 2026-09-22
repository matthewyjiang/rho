use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::workflow::{
    test_support::{agent_node, id, state, workflow},
    NodeTerminalState, PlanConsent, RunId, RunManifest, RunStateRecord, WorkspaceAccess,
    RUN_MANIFEST_VERSION, RUN_STATE_VERSION,
};

// Covers: status JSON consumers written for single-graph releases still find
// graph_digest, graph, and flat root node maps next to the scoped fields.
// Owner: workflow CLI status document.
#[test]
fn status_json_keeps_single_graph_fields() {
    let frozen = workflow(vec![agent_node("inspect", &[], WorkspaceAccess::ReadOnly)]);
    let mut run_state = state(&frozen);
    run_state.root_scope_mut().nodes.insert(
        id("inspect"),
        NodeState::Terminal {
            outcome: NodeTerminalState::Success,
        },
    );
    run_state.root_scope_mut().outputs.insert(
        id("inspect"),
        WorkflowValue::from_json(json!({"summary": "ok"})).unwrap(),
    );
    let digest = frozen.program_digest.clone();
    let run = StoredRun {
        manifest: RunManifest {
            schema_version: RUN_MANIFEST_VERSION,
            run_id: RunId::new(),
            created_at_unix_nanos: 1,
            plan_id: crate::workflow::PlanId::new(),
            program_digest: digest.clone(),
            workspace_identity: "workspace-id".to_owned(),
            consent: PlanConsent {
                program_digest: digest.clone(),
                confirmed: true,
            },
            name: "test".to_owned(),
            step_count: 1,
        },
        graph: frozen.clone(),
        state: RunStateRecord {
            schema_version: RUN_STATE_VERSION,
            last_event_sequence: 0,
            state: run_state.clone(),
        },
    };

    let mut document = serde_json::to_value(&run).unwrap();
    insert_single_graph_fields(&mut document, &run.graph);
    insert_flat_state_fields(&mut document, &run_state).unwrap();

    let root = run_state.root_scope();
    assert_eq!(
        json!({
            "manifest": document["manifest"]["graph_digest"],
            "consent": document["manifest"]["consent"]["graph_digest"],
            "graph": document["graph"]["graph_digest"],
            "graph_view": document["graph"]["graph"],
            "nodes": document["state"]["state"]["nodes"],
            "outputs": document["state"]["state"]["outputs"],
            "command_exits": document["state"]["state"]["command_exits"],
            "completions": document["state"]["state"]["completions"],
            "outcome": document["state"]["state"]["outcome"],
        }),
        json!({
            "manifest": digest.0,
            "consent": digest.0,
            "graph": digest.0,
            "graph_view": {"name": "test", "nodes": frozen.program.root.nodes},
            "nodes": root.nodes,
            "outputs": {"inspect": {"summary": "ok"}},
            "command_exits": {},
            "completions": {},
            "outcome": null,
        })
    );
    assert_eq!(
        document["state"]["state"]["scopes"]["s0"]["nodes"],
        document["state"]["state"]["nodes"]
    );
}
