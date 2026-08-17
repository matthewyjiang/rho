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

// Covers: process results omit command, empty-stream labels, and success exit
// Owner: process output
#[test]
fn snapshot_text_keeps_id_state_cursor_and_streams() {
    assert_eq!(
        format_snapshot(&snapshot()),
        "process_id: proc-1\nstate: running\nnext: 2\nstdout:\nout\nstderr:\nerr"
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
        "process_id: proc-1\nstate: exited\nnext: 2"
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
        "process_id: proc-1\nstate: exited\nnext: 2\nexit: 2"
    );
}

// Covers: stop is a two-line receipt, not JSON
// Owner: process output
#[test]
fn stop_receipt_is_plain_text() {
    assert_eq!(format_stop("proc-1"), "process_id: proc-1\nstop requested");
}
