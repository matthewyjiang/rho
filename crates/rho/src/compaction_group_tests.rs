use pretty_assertions::assert_eq;
use serde_json::json;

use super::completed_tool_group_end;
use rho_providers::model::{ContentBlock, Message, ToolCall, ToolResult};

fn call(id: &str) -> Message {
    Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
        id: id.into(),
        name: "agent".into(),
        arguments: json!({}),
    })])
}

fn calls(ids: &[&str]) -> Message {
    Message::Assistant(
        ids.iter()
            .map(|id| {
                ContentBlock::ToolCall(ToolCall {
                    id: (*id).into(),
                    name: "agent".into(),
                    arguments: json!({}),
                })
            })
            .collect(),
    )
}

fn result(id: &str) -> Message {
    Message::ToolResult(ToolResult {
        id: id.into(),
        ok: true,
        content: "ok".into(),
    })
}

// Covers: compaction groups an assistant with later/out-of-order results, and
// an unresolved call ends the group at the last result it covers.
// Owner: compaction grouping
#[test]
fn completed_tool_group_end_is_id_set_based() {
    struct Case {
        name: &'static str,
        messages: Vec<Message>,
        expected: Option<usize>,
    }
    let cases = [
        Case {
            name: "late result after user steer",
            messages: vec![call("a"), Message::user_text("steer"), result("a")],
            expected: Some(3),
        },
        Case {
            name: "out-of-order results",
            messages: vec![calls(&["a", "b"]), result("b"), result("a")],
            expected: Some(3),
        },
        Case {
            name: "interleaved assistant calls stay paired",
            messages: vec![call("a"), call("b"), result("a"), result("b")],
            expected: Some(4),
        },
        Case {
            name: "partial multi-call assistant keeps the covered result",
            messages: vec![calls(&["a", "b"]), result("a")],
            expected: Some(2),
        },
        Case {
            name: "covered initial call stays paired when nested call is unresolved",
            messages: vec![call("a"), call("b"), result("a")],
            expected: Some(3),
        },
        Case {
            name: "nested call keeps its own result inside the group",
            messages: vec![calls(&["a", "b"]), call("c"), result("a"), result("c")],
            expected: Some(4),
        },
        Case {
            name: "missing result ends at the assistant",
            messages: vec![call("a"), Message::user_text("next")],
            expected: Some(1),
        },
        Case {
            name: "uncovered async call does not swallow later completed turn",
            messages: vec![
                call("a"),
                Message::user_text("later"),
                call("b"),
                result("b"),
            ],
            expected: Some(1),
        },
    ];
    for case in cases {
        assert_eq!(
            completed_tool_group_end(&case.messages, 0),
            case.expected,
            "{}",
            case.name
        );
    }
}
