use std::time::Duration;

use pretty_assertions::assert_eq;

use super::*;
use crate::subagent::{RunState, RunStatus, Verdict};

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
         completion will be delivered automatically\n\
         if this is the only remaining work, end your turn now - do not call sleep or poll\n\
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
         verification: pending\n\
         completion will be delivered automatically\n\
         if this is the only remaining work, end your turn now - do not call sleep or poll\n\
         attach: rho attach abc123"
    );
}

#[test]
fn completed_run_without_a_verdict_is_not_verified() {
    assert_eq!(
        format_snapshot(&snapshot(true), SnapshotFormat::Completion),
        "agent abc123 (explorer): ok\n\
         turns: 3 · tokens: 1200 in / 300 out\n\
         verification: run completed; no review verdict — implementation done, not verified\n\
         \n\
         found it"
    );
}

#[test]
fn passing_review_is_the_only_verified_state() {
    let mut passed = snapshot(true);
    passed.status.verdict = Some(Verdict::Pass);
    assert!(format_snapshot(&passed, SnapshotFormat::Completion)
        .contains("verification: review passed"));

    let mut findings = snapshot(true);
    findings.status.verdict = Some(Verdict::Findings);
    assert!(format_snapshot(&findings, SnapshotFormat::Completion)
        .contains("verification: review reported findings — not verified"));
}

#[test]
fn failed_review_is_explicitly_unverified() {
    let mut failed = snapshot(true);
    failed.status.state = RunState::Error;
    failed.status.result = None;
    failed.status.error = Some("provider stream failed after emitting output".into());

    assert_eq!(
        format_snapshot(&failed, SnapshotFormat::Completion),
        "agent abc123 (explorer): error\n\
         turns: 3 · tokens: 1200 in / 300 out\n\
         verification: incomplete — the delegated run did not finish; nothing is verified\n\
         error: provider stream failed after emitting output"
    );
}

#[test]
fn completed_status_defers_result_to_automatic_delivery() {
    let mut snapshot = snapshot(true);
    snapshot.status.state = RunState::Error;
    snapshot.status.error = Some("provider stream failed".into());
    snapshot.status.last_text = Some("found it".into());

    assert_eq!(
        format_snapshot(&snapshot, SnapshotFormat::Status),
        "agent abc123 (explorer): error\n\
         elapsed: 1m 30s · turns: 3 · tokens: 1200 in / 300 out\n\
         verification: incomplete — the delegated run did not finish; nothing is verified\n\
         completion result uses automatic delivery\n\
         attach: rho attach abc123"
    );
}

#[test]
fn formats_completion_with_result_and_error() {
    let mut snapshot = snapshot(true);
    snapshot.status.state = RunState::Error;
    snapshot.status.error = Some("provider stream failed".into());

    assert_eq!(
        format_snapshot(&snapshot, SnapshotFormat::Completion),
        "agent abc123 (explorer): error\n\
         turns: 3 · tokens: 1200 in / 300 out\n\
         verification: incomplete — the delegated run did not finish; nothing is verified\n\
         error: provider stream failed\n\
         \n\
         found it"
    );
}
