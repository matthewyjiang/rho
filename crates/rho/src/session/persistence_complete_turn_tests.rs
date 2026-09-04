use pretty_assertions::assert_eq;
use serde_json::json;

use super::complete_turn_tail_len;
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

// Covers: a complete prefix keeps late and out-of-order results; a missing
// result truncates at the unfinished assistant.
// Owner: session persistence
#[test]
fn complete_turn_tail_len_is_id_set_based() {
    struct Case {
        name: &'static str,
        messages: Vec<Message>,
        expected: usize,
    }
    let cases = [
        Case {
            name: "late result after user steer",
            messages: vec![
                Message::user_text("go"),
                call("a"),
                Message::user_text("steer"),
                result("a"),
            ],
            expected: 4,
        },
        Case {
            name: "out-of-order results",
            messages: vec![calls(&["a", "b"]), result("b"), result("a")],
            expected: 3,
        },
        Case {
            name: "missing result truncates",
            messages: vec![
                Message::user_text("go"),
                call("a"),
                Message::user_text("next"),
            ],
            expected: 1,
        },
    ];
    for case in cases {
        assert_eq!(
            complete_turn_tail_len(&case.messages, |message| message),
            case.expected,
            "{}",
            case.name
        );
    }
}
