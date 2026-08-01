use pretty_assertions::assert_eq;
use rho_sdk::hooks::HookPayloadBounds;

use super::*;

fn artifact(path: &str) -> crate::workflow::DurableArtifactReference {
    crate::workflow::DurableArtifactReference {
        kind: crate::workflow::ArtifactKind::Stdout,
        artifact: crate::workflow::ArtifactRef {
            relative_path: path.to_owned(),
            retained_bytes: path.len() as u64,
            observed: crate::workflow::ArtifactObservation::Complete {
                observed_bytes: path.len() as u64,
            },
            digest: crate::workflow::Digest("sha256:test".into()),
        },
    }
}

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
            artifacts: &[artifact("artifact-long")],
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
                "payload.artifact_references.0.relative_path",
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
            "artifact_references": [{
                "kind": "stdout",
                "relative_path": "arti",
                "retained_bytes": 13,
                "observed": {"kind": "complete", "observed_bytes": 13},
                "digest": "sha256:test"
            }]
        })
    );
}
