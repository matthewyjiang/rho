use pretty_assertions::assert_eq;
use serde_json::json;

use super::super::WORKFLOW_WIRE_VERSION;
use super::runtime_event_json;
use crate::{
    app::workflow_runtime::RuntimeEvent,
    workflow::{AttemptNumber, NodeId, NodeTerminalState, RunId},
};

// Covers: RuntimeEvent wire body comes from Serialize, with envelope fields layered on.
// Owner: workflow CLI runtime event presentation.
#[test]
fn runtime_event_json_matches_wire_shape() {
    let run_id = "00000000-0000-4000-8000-000000000001"
        .parse::<RunId>()
        .unwrap();
    let node = NodeId::new("build").unwrap();
    let attempt = AttemptNumber::new(2).unwrap();

    assert_eq!(
        runtime_event_json(1, run_id, &RuntimeEvent::StateChanged { revision: 7 }),
        json!({
            "type": "state_changed",
            "revision": 7,
            "version": WORKFLOW_WIRE_VERSION,
            "sequence": 1,
            "run_id": run_id.to_string(),
        })
    );
    assert_eq!(
        runtime_event_json(
            2,
            run_id,
            &RuntimeEvent::NodeStarted {
                node: node.clone(),
                attempt
            }
        ),
        json!({
            "type": "node_started",
            "node": "build",
            "attempt": 2,
            "version": WORKFLOW_WIRE_VERSION,
            "sequence": 2,
            "run_id": run_id.to_string(),
        })
    );
    assert_eq!(
        runtime_event_json(
            3,
            run_id,
            &RuntimeEvent::NodeFinished {
                node: node.clone(),
                outcome: NodeTerminalState::Success
            }
        ),
        json!({
            "type": "node_finished",
            "node": "build",
            "outcome": "success",
            "version": WORKFLOW_WIRE_VERSION,
            "sequence": 3,
            "run_id": run_id.to_string(),
        })
    );
    assert_eq!(
        runtime_event_json(
            4,
            run_id,
            &RuntimeEvent::NeedsRecovery {
                nodes: vec![node.clone()]
            }
        ),
        json!({
            "type": "needs_recovery",
            "nodes": ["build"],
            "version": WORKFLOW_WIRE_VERSION,
            "sequence": 4,
            "run_id": run_id.to_string(),
        })
    );
    assert_eq!(
        runtime_event_json(5, run_id, &RuntimeEvent::Completed),
        json!({
            "type": "completed",
            "version": WORKFLOW_WIRE_VERSION,
            "sequence": 5,
            "run_id": run_id.to_string(),
        })
    );
    assert_eq!(
        runtime_event_json(
            6,
            run_id,
            &RuntimeEvent::NodeProgress {
                node: node.clone(),
                attempt,
                message: "tool: Bash".into(),
                detail: Some("git status".into()),
                completed: Some(2),
                total: None,
            }
        ),
        json!({
            "type": "node_progress",
            "node": "build",
            "attempt": 2,
            "message": "tool: Bash",
            "detail": "git status",
            "completed": 2,
            "version": WORKFLOW_WIRE_VERSION,
            "sequence": 6,
            "run_id": run_id.to_string(),
        })
    );
}

// Covers: tools and CLI text share one RuntimeEvent message path.
// Owner: workflow CLI runtime event presentation.
#[test]
fn runtime_event_message_is_canonical() {
    let node = NodeId::new("build").unwrap();
    let attempt = AttemptNumber::new(2).unwrap();
    assert_eq!(
        RuntimeEvent::StateChanged { revision: 3 }.message(),
        "workflow state revision 3"
    );
    assert_eq!(
        RuntimeEvent::NodeStarted {
            node: node.clone(),
            attempt
        }
        .message(),
        "workflow node build started attempt 2"
    );
    assert_eq!(
        RuntimeEvent::NodeProgress {
            node: node.clone(),
            attempt,
            message: "tool: Read".into(),
            detail: Some("src/lib.rs".into()),
            completed: None,
            total: None,
        }
        .message(),
        "workflow node build: tool: Read · src/lib.rs"
    );
    assert_eq!(
        RuntimeEvent::NodeFinished {
            node: node.clone(),
            outcome: NodeTerminalState::Failure
        }
        .message(),
        "workflow node build finished: Failure"
    );
    assert_eq!(
        RuntimeEvent::NeedsRecovery {
            nodes: vec![node, NodeId::new("test").unwrap()]
        }
        .message(),
        "workflow needs recovery: build, test"
    );
    assert_eq!(RuntimeEvent::Completed.message(), "workflow completed");
}
