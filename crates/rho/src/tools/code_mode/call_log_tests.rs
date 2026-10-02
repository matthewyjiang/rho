use pretty_assertions::assert_eq;
use serde_json::json;

use super::{NestedCallRecord, NestedCallStatus};

// Covers: rows carry status, the primary argument as plain text (compact JSON
// when no argument names the work), timing, and the latest detail line; long
// args are cut on a char boundary and empty args are omitted. Compact summaries
// prefer progress over arguments and normalize whitespace within the same budget.
// Owner: codemode nested-call display row.
#[test]
fn rows_summarize_each_call() {
    let mut running = NestedCallRecord::running("bash", &json!({"command": "cargo test"}));
    running.set_progress("compiling\n\n   running   3 tests  \n");
    let mut finished = NestedCallRecord::running("list_tools", &json!({}));
    finished.status = NestedCallStatus::Ok;
    finished.duration_ms = Some(1340);
    let long = NestedCallRecord::running("read_file", &json!({"path": "é".repeat(100)}));
    let unnamed = NestedCallRecord::running("tally", &json!({"n": 3}));
    assert_eq!(
        [running, finished, long, unnamed].map(|record| (record.row(), record.summary())),
        [
            (
                "● bash cargo test · running   3 tests".to_owned(),
                "● bash · running 3 tests".to_owned(),
            ),
            ("✓ list_tools 1.3s".to_owned(), "✓ list_tools".to_owned()),
            (
                format!("● read_file {}…", "é".repeat(79)),
                format!("● read_file · {}…", "é".repeat(65)),
            ),
            (
                "● tally {\"n\":3}".to_owned(),
                "● tally · {\"n\":3}".to_owned(),
            ),
        ]
    );
}
