use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::workflow::{
    test_support::{agent_node, id, state, workflow},
    NodeState, NodeTerminalState, PlanConsent, RunId, RunManifest, RunStateRecord, WorkflowValue,
    WorkspaceAccess, RUN_MANIFEST_VERSION, RUN_STATE_VERSION,
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

    let mut bytes = Vec::new();
    compat::write_status_json(&mut bytes, &run).unwrap();
    let status: Value = serde_json::from_slice(&bytes).unwrap();
    let root = run_state.root_scope();
    let mut expected = serde_json::to_value(&run).unwrap();
    expected["manifest"]["graph_digest"] = json!(digest);
    expected["manifest"]["consent"]["graph_digest"] = json!(digest);
    expected["graph"]["graph_digest"] = json!(digest);
    expected["graph"]["graph"] = json!({"name": "test", "nodes": frozen.program.root.nodes});
    expected["state"]["state"]["nodes"] = json!(root.nodes);
    expected["state"]["state"]["outputs"] = json!(root.outputs);
    expected["state"]["state"]["command_exits"] = json!(root.command_exits);
    expected["state"]["state"]["completions"] = json!(root.completions);
    expected["state"]["state"]["outcome"] = Value::Null;
    assert_eq!(status, json!({"run": expected, "outcome": null}));

    let plan = StoredPlan {
        manifest: crate::workflow::PlanManifest {
            schema_version: crate::workflow::PLAN_MANIFEST_VERSION,
            plan_id: run.manifest.plan_id,
            created_at_unix_nanos: 1,
            program_digest: digest.clone(),
            workspace_identity: "workspace-id".into(),
            source_digests: BTreeMap::new(),
            name: "test".into(),
            step_count: 1,
        },
        graph: frozen,
    };
    bytes.clear();
    compat::write_plan_json(&mut bytes, &plan).unwrap();
    let actual: Value = serde_json::from_slice(&bytes).unwrap();
    let mut expected_plan = serde_json::to_value(&plan).unwrap();
    expected_plan["manifest"]["graph_digest"] = json!(digest);
    expected_plan["graph"] = expected["graph"].clone();
    assert_eq!(actual, expected_plan);
}

// Covers: status retains old completion/output payloads without current runtime decoding.
// Owner: CLI document writer; the fixture follows wire.rs at 2ea7ad26.
#[test]
fn legacy_status_preserves_completed_snapshot() {
    let state: Value = serde_json::from_str(include_str!(
        "../../workflow/fixtures/legacy_run_state.json"
    ))
    .unwrap();
    let manifest = json!({
        "schema_version": 1, "run_id": "018f0000-0000-7000-8000-000000000002",
        "plan_id": "018f0000-0000-7000-8000-000000000001", "created_at_unix_nanos": 8,
        "graph_digest": "sha256:legacy", "workspace_identity": "workspace-id",
        "consent": {"graph_digest": "sha256:legacy", "confirmed": true},
        "name": "review", "step_count": 1,
    });
    let graph = json!({"schema_version": 2, "graph_digest": "sha256:legacy"});
    let run = LegacyRun {
        manifest: serde_json::from_value(manifest.clone()).unwrap(),
        graph: graph.clone(),
        state: serde_json::from_value(state.clone()).unwrap(),
    };
    let mut bytes = Vec::new();
    write_legacy_status_json(&mut bytes, &run).unwrap();
    let actual: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        actual,
        json!({"run": {"manifest": manifest, "graph": graph, "state": state}, "outcome": "success"})
    );
}
