use std::time::Duration;

use pretty_assertions::assert_eq;

use super::{format_snapshot, SnapshotFormat};
use crate::{
    agent::AgentRuntime,
    subagent::{RunState, RunStatus},
    tools::agent::SubagentSnapshot,
};

// Covers: a Cursor ACP session id is reported without a resume command; it is
// not a `cursor-agent --resume` id, so a hint would send the model astray.
// Owner: agent output
#[test]
fn cursor_session_line_has_no_resume_command() {
    let snapshot = SubagentSnapshot {
        prior_notices: Vec::new(),
        id: "abc123".into(),
        agent_id: "cursor-test".into(),
        title: None,
        elapsed: Duration::from_secs(1),
        status: RunStatus {
            state: RunState::Ok,
            runtime: Some(AgentRuntime::Cursor),
            claude_session_id: Some("sess-cursor".into()),
            ..RunStatus::default()
        },
        done: true,
    };
    let text = format_snapshot(&snapshot, SnapshotFormat::Completion);
    assert_eq!(
        text.lines()
            .find(|line| line.starts_with("cursor session:") || line.starts_with("claude session:"))
            .unwrap(),
        "cursor session: sess-cursor"
    );
}

// Covers: a script sees a delegated run's result only once it is done (the
// same privacy rule as the text), with state and error typed.
// Owner: agent output structured view.
#[test]
fn structured_run_exposes_result_only_when_done() {
    let snapshot = |state: RunState, done: bool| SubagentSnapshot {
        prior_notices: Vec::new(),
        id: "run1".into(),
        agent_id: "explorer".into(),
        title: None,
        elapsed: Duration::from_secs(1),
        status: RunStatus {
            state,
            result: Some("found it".into()),
            ..RunStatus::default()
        },
        done,
    };
    assert_eq!(
        [
            serde_json::to_value(super::AgentRunView::from(&snapshot(
                RunState::Running,
                false
            )))
            .unwrap(),
            serde_json::to_value(super::AgentRunView::from(&snapshot(RunState::Ok, true))).unwrap(),
        ],
        [
            serde_json::json!({"id": "run1", "agent_id": "explorer", "state": "running",
                "done": false, "result": null, "error": null}),
            serde_json::json!({"id": "run1", "agent_id": "explorer", "state": "ok",
                "done": true, "result": "found it", "error": null}),
        ]
    );
}
