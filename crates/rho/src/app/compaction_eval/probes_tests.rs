use std::collections::BTreeMap;

use pretty_assertions::assert_eq;
use rho_sdk::model::{ContentBlock, Message, ToolCall, ToolResult};
use serde_json::json;

use super::{path_recall, references, Probe, Reference};

fn call(id: &str, name: &str, arguments: serde_json::Value) -> Message {
    Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
        id: id.into(),
        name: name.into(),
        arguments,
    })])
}

fn result(id: &str, ok: bool, content: &str) -> Message {
    Message::ToolResult(ToolResult {
        id: id.into(),
        ok,
        content: content.into(),
    })
}

// Covers: references cover every edit format, pick the latest check command
// rather than any shell call, and list failed calls.
// Owner: compaction eval probe references
#[test]
fn references_come_from_the_history() {
    let history = vec![
        Message::System("system".into()),
        Message::user_text("fix the parser, keep the public API"),
        // Host notifications are not user requests.
        Message::user_text("[process notification]\n\nProcess status:\nexited"),
        // A failed exploration call is not an error to remember.
        call("0", "bash", json!({"command": "grep -n missing src"})),
        result("0", false, "exit 1"),
        call("1", "write", json!({"path": "src/new.rs", "content": "x"})),
        result("1", true, "wrote"),
        call(
            "2",
            "edit",
            json!({"input": "[src/lib.rs#AB12]\nPUT 3:\n+x"}),
        ),
        result("2", true, "edited"),
        call(
            "3",
            "apply_patch",
            json!({"input": "*** Begin Patch\n*** Update File: a.rs\n*** Move to: b.rs\n*** End Patch"}),
        ),
        result("3", true, "patched"),
        call("4", "bash", json!({"command": "cargo test -p parser"})),
        result("4", false, "1 failed"),
        // Later, but not a check: mentions "check" only as a flag.
        call("5", "bash", json!({"command": "git diff --check"})),
        result("5", true, ""),
        call("6", "str_replace", json!({"path": "src/kept.rs"})),
        result("6", true, "replaced"),
    ];

    assert_eq!(
        references(&history),
        BTreeMap::from([
            (
                Probe::FilesChanged,
                Reference::Paths(vec![
                    "a.rs".into(),
                    "b.rs".into(),
                    "src/kept.rs".into(),
                    "src/lib.rs".into(),
                    "src/new.rs".into(),
                ]),
            ),
            (
                Probe::TestResult,
                Reference::Facts("command: cargo test -p parser\nresult: failed".into()),
            ),
            (
                Probe::UserRequests,
                Reference::Facts("1. fix the parser, keep the public API".into()),
            ),
            (
                Probe::Errors,
                Reference::Facts("1. bash {\"command\":\"cargo test -p parser\"}: 1 failed".into()),
            ),
        ])
    );
}

// Covers: exact-match scoring accepts absolute or relative spellings of a
// path and gives partial credit.
// Owner: compaction eval scoring
#[test]
fn path_recall_matches_path_suffixes() {
    let paths = ["/repo/src/lib.rs".to_string(), "src/new.rs".to_string()];
    let cases = [
        ("both relative", "src/lib.rs\nsrc/new.rs", 1.0),
        ("absolute", "/home/me/repo/src/new.rs", 0.5),
        ("bare file name", "lib.rs", 0.0),
        ("unknown", "I do not know", 0.0),
    ];

    for (name, answer, expected) in cases {
        assert_eq!(path_recall(&paths, answer), expected, "{name}");
    }
}

// Covers: only real test, build, and lint invocations count as the latest
// check, including after `cd`, env assignments, and `timeout`.
// Owner: compaction eval probe references
#[test]
fn check_commands_are_invocations_not_words() {
    let cases = [
        ("cargo test -p rho", true),
        (
            "cd crates && RUST_LOG=off timeout 600 cargo clippy --all-targets",
            true,
        ),
        ("python3 scripts/validate.py full > /tmp/log 2>&1", true),
        ("git diff --check && git diff", false),
        ("git commit -m 'fix: make tests pass'", false),
        ("gh pr checks 12", false),
        (
            "git commit -F - <<'EOF'\nran python3 scripts/validate.py full\nEOF",
            false,
        ),
        ("mise current rust", false),
    ];

    for (command, expected) in cases {
        assert_eq!(super::is_check_command(command), expected, "{command}");
    }
}
