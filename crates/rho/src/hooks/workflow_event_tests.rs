use pretty_assertions::assert_eq;
use rho_sdk::hooks::HookPayloadBounds;

use super::*;

// Covers: every workflow payload text field must be bounded and named when shortened.
// Owner: app hook wire envelope.
#[test]
fn workflow_envelopes_report_all_shortened_fields() {
    let envelope = AppHookEnvelope::new(
        WorkflowHookEventKind::NodeFinished,
        WorkflowPayload::Node {
            workflow_run_id: "workflow-run",
            plan_digest: "plan-digest",
            node_id: "node-long",
            attempt: 3,
            outcome: Some("success"),
            duration_ms: Some(7),
            artifacts: &["artifact-long".to_owned()],
        },
        HookPayloadBounds::new(4, 4096),
    )
    .unwrap();
    let encoded: serde_json::Value =
        serde_json::from_str(&envelope.to_bounded_json().unwrap()).unwrap();

    assert_eq!(
        encoded["bounds"],
        serde_json::json!({
            "truncated": true,
            "fields": [
                "payload.artifact_references.0",
                "payload.node_id",
                "payload.outcome",
                "payload.plan_digest",
                "payload.workflow_run_id"
            ]
        })
    );
    assert_eq!(
        encoded["payload"],
        serde_json::json!({
            "workflow_run_id": "work",
            "plan_digest": "plan",
            "node_id": "node",
            "attempt": 3,
            "outcome": "succ",
            "duration_ms": 7,
            "artifact_references": ["arti"]
        })
    );
}
