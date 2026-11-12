use std::collections::HashSet;

use pretty_assertions::assert_eq;
use rho_sdk::model::{ContentBlock, Message, ToolCall, ToolResult};
use serde_json::json;

use super::*;

fn call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall::new(id.into(), name.into(), arguments)
}

fn read(id: &str, path: &str) -> ToolCall {
    call(id, "read_file", json!({ "path": path }))
}

fn turn(call: ToolCall, content: &str) -> [Message; 2] {
    let id = call.id.clone();
    [
        Message::Assistant(vec![ContentBlock::ToolCall(call)]),
        Message::ToolResult(ToolResult::succeeded(id, content)),
    ]
}

fn fingerprints(calls: &[ToolCall]) -> HashSet<ToolFingerprint> {
    calls.iter().filter_map(ToolFingerprint::of).collect()
}

// Covers: the proxy watches exactly the calls whose results the agent lost.
// A kept result is not a loss; an elision stub (same id, new content) and a
// summarized-away result are. Tools other than read_file and shells, and
// calls missing the identifying argument, are never watched.
// Owner: compaction metrics.
#[test]
fn removed_tool_calls_are_the_ones_whose_results_left_history() {
    let kept = read("kept", "src/kept.rs");
    let elided = read("elided", "src/elided.rs");
    let summarized = call("summarized", "bash", json!({ "command": "cargo test" }));
    let unwatched = call("grep", "grep", json!({ "pattern": "x" }));
    let no_path = call("no_path", "read_file", json!({}));
    let before = [
        turn(kept.clone(), "kept text"),
        turn(elided.clone(), "elided text"),
        turn(summarized.clone(), "test output"),
        turn(unwatched, "matches"),
        turn(no_path, "error"),
    ]
    .concat();
    let after = [
        turn(kept, "kept text"),
        turn(elided.clone(), "[elided tool result]"),
    ]
    .concat();

    assert_eq!(
        removed_tool_calls(&before, &after),
        fingerprints(&[elided, summarized])
    );
}
