use std::time::Duration;

use pretty_assertions::assert_eq;

use super::*;
use crate::subagent::{RunState, RunStatus};

fn snapshot(done: bool) -> SubagentSnapshot {
    SubagentSnapshot {
        id: "abc123".into(),
        agent_id: "explorer".into(),
        background: true,
        elapsed: Duration::from_secs(90),
        status: RunStatus {
            state: if done {
                RunState::Ok
            } else {
                RunState::Running
            },
            turns: 3,
            input_tokens: 1_200,
            output_tokens: 300,
            last_activity: Some("searching files".into()),
            result: done.then(|| "found it".into()),
            ..RunStatus::default()
        },
        done,
    }
}

#[test]
fn formats_agent_start_output() {
    assert_eq!(
        format_background_start("abc123", "explorer"),
        "agent abc123 (explorer) started in background\n\
         attach: rho attach abc123"
    );
    assert_eq!(
        format_running("abc123"),
        "agent abc123 running\nattach: rho attach abc123"
    );
}

#[test]
fn formats_list_entries_as_single_lines() {
    assert_eq!(
        format_list_entry(&snapshot(false)),
        "abc123  explorer  running  1m 30s  searching files"
    );
}

#[test]
fn formats_status_with_runtime_details() {
    assert_eq!(
        format_snapshot(&snapshot(false), SnapshotFormat::Status),
        "agent abc123 (explorer): running\n\
         elapsed: 1m 30s · turns: 3 · tokens: 1200 in / 300 out\n\
         activity: searching files\n\
         attach: rho attach abc123"
    );
}

#[test]
fn formats_completion_with_result() {
    assert_eq!(
        format_snapshot(&snapshot(true), SnapshotFormat::Completion),
        "agent abc123 (explorer): ok\n\
         turns: 3 · tokens: 1200 in / 300 out\n\
         \n\
         found it"
    );
}
