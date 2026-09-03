use std::time::Duration;

use pretty_assertions::assert_eq;

use super::{format_snapshot, SnapshotFormat};
use crate::{
    agent::AgentRuntime,
    subagent::{RunState, RunStatus},
    tools::agent::SubagentSnapshot,
};

// Covers: Cursor session ids render a cursor-agent resume line, not Claude's.
// Owner: agent output
#[test]
fn cursor_session_line_uses_cursor_agent_resume() {
    let snapshot = SubagentSnapshot {
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
        "cursor session: sess-cursor (resume with `cursor-agent --resume sess-cursor`)"
    );
}
