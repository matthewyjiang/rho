use pretty_assertions::assert_eq;

use super::super::{Chunk, Snapshot, State, Stream};
use super::*;

fn snapshot() -> Snapshot {
    Snapshot {
        process_id: "proc-1".into(),
        command: "sleep 300".into(),
        state: State::Running,
        runtime_seconds: 1.25,
        first_cursor: 0,
        next_cursor: 2,
        available_cursor: 2,
        truncated: false,
        output_pending: false,
        chunks: vec![
            Chunk {
                cursor: 0,
                stream: Stream::Stdout,
                text: "out".into(),
            },
            Chunk {
                cursor: 1,
                stream: Stream::Stderr,
                text: "err".into(),
            },
        ],
        exit_code: None,
        terminal_detail: None,
    }
}

// Covers: process results include command, empty-stream labels, and success exit
// Owner: process output
#[test]
fn snapshot_text_keeps_id_command_state_cursor_and_streams() {
    assert_eq!(
        format_snapshot(&snapshot()),
        "process_id: proc-1\ncommand: sleep 300\nstate: running\nnext: 2\n\nstdout:\nout\nstderr:\nerr"
    );
}

// Covers: successful exits do not repeat exit 0
// Owner: process output
#[test]
fn successful_exit_omits_exit_line() {
    let mut snapshot = snapshot();
    snapshot.state = State::Exited;
    snapshot.exit_code = Some(0);
    snapshot.chunks.clear();
    assert_eq!(
        format_snapshot(&snapshot),
        "process_id: proc-1\ncommand: sleep 300\nstate: exited\nnext: 2"
    );
}

// Covers: failed exits keep the code and drop empty streams
// Owner: process output
#[test]
fn failed_exit_includes_code() {
    let mut snapshot = snapshot();
    snapshot.state = State::Exited;
    snapshot.exit_code = Some(2);
    snapshot.chunks.clear();
    assert_eq!(
        format_snapshot(&snapshot),
        "process_id: proc-1\ncommand: sleep 300\nstate: exited\nnext: 2\nexit: 2"
    );
}

// Covers: every terminal failure reaches callers as a failed tool output;
// live snapshots remain successful observations.
// Owner: process output adapter
#[test]
fn rendered_process_outcomes_flag_failures() {
    for (state, exit_code, failed) in [
        (State::Starting, None, false),
        (State::Running, None, false),
        (State::Exited, Some(0), false),
        (State::Exited, Some(2), true),
        (State::Exited, None, true),
        (State::Terminated, None, true),
        (State::TimedOut, Some(0), true),
        (State::FailedToStart, None, true),
    ] {
        let mut snapshot = snapshot();
        snapshot.state = state;
        snapshot.exit_code = exit_code;
        let output = render_snapshot(snapshot)
            .into_tool_output(rho_sdk::tool::ToolMetadata::new())
            .unwrap();
        assert_eq!(output.is_failure(), failed, "{state:?}, {exit_code:?}");
    }
}

// Covers: JSON output caps preserve process control data and replayable cursors,
// including progress past a chunk that cannot fit alone.
// Owner: process script-output budgeting; existing tests only bound model text.
#[test]
fn process_data_budget_preserves_control_fields_and_pages_chunks() {
    let mut input = snapshot();
    // Escaping makes these chunks larger in JSON than in model-facing text.
    for chunk in &mut input.chunks {
        chunk.text = "\u{0001}".repeat(64);
    }
    let original = serde_json::json!({
        "action": "snapshot",
        "process_id": "proc-1",
        "command": "sleep 300",
        "state": "running",
        "runtime_seconds": 1.25,
        "first_cursor": 0,
        "next_cursor": 2,
        "available_cursor": 2,
        "truncated": false,
        "output_pending": false,
        "chunks": [
            {"cursor": 0, "stream": "stdout", "text": "\u{0001}".repeat(64)},
            {"cursor": 1, "stream": "stderr", "text": "\u{0001}".repeat(64)},
        ],
        "exit_code": null,
        "terminal_detail": null,
    });
    let received_bytes = serde_json::to_vec(&original).unwrap().len();
    let paging_budget = received_bytes - 1;
    let mut paged = original.clone();
    paged["chunks"] = serde_json::json!([original["chunks"][0].clone()]);
    paged["next_cursor"] = serde_json::json!(1);
    paged["output_pending"] = serde_json::json!(true);
    paged["output_budget"] = serde_json::json!({
        "max_output_bytes": paging_budget,
        "received_bytes": received_bytes,
        "deferred_chunks": 1,
        "omitted_chunks": 0,
    });
    let mut omitted = paged.clone();
    omitted["chunks"] = serde_json::json!([]);
    omitted["output_budget"]["omitted_chunks"] = serde_json::json!(1);
    // Measure a control-only receipt budget, too small for either escaped chunk.
    let control_budget = serde_json::to_vec(&omitted).unwrap().len();
    omitted["output_budget"]["max_output_bytes"] = serde_json::json!(control_budget);

    for (name, max_output_bytes, expected) in [
        ("within budget", received_bytes, original.clone()),
        ("paged", paging_budget, paged),
        ("indivisible chunk", control_budget, omitted),
    ] {
        let bounded = limit_process_data(render_snapshot(input.clone()), max_output_bytes)
            .unwrap()
            .limit_data(max_output_bytes)
            .unwrap();
        assert_eq!(bounded.text(), format_snapshot(&input), "{name}");
        let actual = serde_json::to_value(bounded.data().unwrap()).unwrap();
        assert_eq!(actual, expected, "{name}");
        assert!(serde_json::to_vec(&actual).unwrap().len() <= max_output_bytes);
        // Manager polls filter retained chunks by cursor, without consuming them.
        let cursor = actual["next_cursor"].as_u64().unwrap();
        let reachable = input
            .chunks
            .iter()
            .filter(|chunk| chunk.cursor >= cursor)
            .cloned()
            .collect::<Vec<_>>();
        let expected_remainder = if name == "within budget" {
            vec![]
        } else {
            vec![input.chunks[1].clone()]
        };
        assert_eq!(reachable, expected_remainder, "{name}");
    }
}

// Covers: stop is a two-line receipt, not JSON
// Owner: process output
#[test]
fn stop_receipt_is_plain_text() {
    assert_eq!(format_stop("proc-1"), "process_id: proc-1\nstop requested");
}

// Covers: command newlines stay in one header line and cannot spoof exit
// Owner: process output
#[test]
fn snapshot_text_escapes_multiline_command() {
    let mut snapshot = snapshot();
    snapshot.command = "echo ok\nexit: 9".into();
    snapshot.chunks.clear();
    let text = format_snapshot(&snapshot);
    assert_eq!(
        text,
        "process_id: proc-1\ncommand: echo ok\\nexit: 9\nstate: running\nnext: 2"
    );
    assert_eq!(decode_header_value("echo ok\\nexit: 9"), "echo ok\nexit: 9");
}
