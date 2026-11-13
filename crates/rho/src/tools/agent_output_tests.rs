use std::time::Duration;

use pretty_assertions::assert_eq;

use super::{format_snapshot, SnapshotFormat};
use crate::{
    agent::AgentRuntime,
    session::SessionPersistence,
    subagent::{RunState, RunStatus},
    tools::agent::SubagentSnapshot,
};

// Covers: a session line offers a resume command only for a session that can
// be resumed: a Cursor ACP id is not a `cursor-agent --resume` id, and a
// Claude session a `--no-save` run discarded is gone, so a hint would send
// the model astray.
// Owner: agent output
#[test]
fn session_line_offers_resume_only_for_a_resumable_session() {
    let session_line = |runtime: AgentRuntime, persistence: SessionPersistence| {
        let snapshot = SubagentSnapshot {
            prior_notices: Vec::new(),
            persistence,
            id: "abc123".into(),
            agent_id: "test".into(),
            title: None,
            elapsed: Duration::from_secs(1),
            status: RunStatus {
                state: RunState::Ok,
                runtime: Some(runtime),
                runtime_session_id: Some("sess-1".into()),
                ..RunStatus::default()
            },
            done: true,
        };
        format_snapshot(&snapshot, SnapshotFormat::Completion)
            .lines()
            .find(|line| line.contains(" session: "))
            .map(str::to_owned)
    };
    assert_eq!(
        [
            session_line(AgentRuntime::Cursor, SessionPersistence::Saved),
            session_line(AgentRuntime::ClaudeCli, SessionPersistence::Saved),
            session_line(AgentRuntime::ClaudeCli, SessionPersistence::Ephemeral),
        ],
        [
            Some("cursor session: sess-1".to_owned()),
            Some(
                "claude session: sess-1 (resume with `claude-agent-acp --cli --resume sess-1`)"
                    .to_owned()
            ),
            Some("claude session: sess-1".to_owned()),
        ]
    );
}

// Covers: a script sees a delegated run's result only once it is done (the
// same privacy rule as the text), with state and error typed.
// Owner: agent output structured view.
#[test]
fn structured_run_exposes_result_only_when_done() {
    let snapshot = |state: RunState, done: bool| SubagentSnapshot {
        prior_notices: Vec::new(),
        persistence: SessionPersistence::Saved,
        id: "run1".into(),
        agent_id: "explorer".into(),
        title: None,
        elapsed: Duration::from_secs(1),
        status: RunStatus {
            state,
            provider: Some("xai".into()),
            model: Some("grok-4.7".into()),
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
            serde_json::to_value(super::AgentRunView::started_with_identity(&snapshot(
                RunState::Ok,
                true
            )))
            .unwrap(),
        ],
        [
            serde_json::json!({"id": "run1", "agent_id": "explorer", "state": "running",
                "provider": "xai", "model": "grok-4.7",
                "done": false, "result": null, "error": null}),
            serde_json::json!({"id": "run1", "agent_id": "explorer", "state": "ok",
                "provider": "xai", "model": "grok-4.7",
                "done": true, "result": "found it", "error": null}),
            serde_json::json!({"id": "run1", "agent_id": "explorer", "state": "starting",
                "provider": "xai", "model": "grok-4.7",
                "done": false, "result": null, "error": null}),
        ]
    );
}
