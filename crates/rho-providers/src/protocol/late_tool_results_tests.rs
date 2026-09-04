use std::borrow::Cow;

use pretty_assertions::assert_eq;
use serde_json::json;

use super::{normalize_late_tool_results, LATE_PLACEHOLDER};
use crate::model::{ContentBlock, Message, ToolCall, ToolResult};

fn call(id: &str, name: &str) -> Message {
    Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: json!({}),
    })])
}

fn calls(entries: &[(&str, &str)]) -> Message {
    Message::Assistant(
        entries
            .iter()
            .map(|(id, name)| {
                ContentBlock::ToolCall(ToolCall {
                    id: (*id).into(),
                    name: (*name).into(),
                    arguments: json!({}),
                })
            })
            .collect(),
    )
}

fn result(id: &str, content: &str) -> Message {
    Message::ToolResult(ToolResult {
        id: id.into(),
        ok: true,
        content: content.into(),
    })
}

fn placeholder(id: &str) -> Message {
    Message::ToolResult(ToolResult {
        id: id.into(),
        ok: true,
        content: LATE_PLACEHOLDER.into(),
    })
}

fn late_user(id: &str, name: &str, content: &str) -> Message {
    Message::user_text(format!("Result for tool call {id} ({name}): {content}"))
}

// Covers: adjacency providers must see a tool result beside each call, with
// delayed results rewritten as user text.
// Owner: protocol late-tool-result pairing
#[test]
fn normalize_late_tool_results_pairs_or_borrows() {
    struct Case {
        name: &'static str,
        input: Vec<Message>,
        expected: Option<Vec<Message>>,
    }
    let cases = [
        Case {
            name: "already paired",
            input: vec![
                Message::user_text("go"),
                call("a", "bash"),
                result("a", "ok"),
            ],
            expected: None,
        },
        Case {
            name: "late after a user steer",
            input: vec![
                call("a", "one_agent"),
                Message::user_text("steer"),
                result("a", "done"),
            ],
            expected: Some(vec![
                call("a", "one_agent"),
                placeholder("a"),
                Message::user_text("steer"),
                late_user("a", "one_agent", "done"),
            ]),
        },
        Case {
            name: "two calls one late",
            input: vec![
                calls(&[("a", "bash"), ("b", "one_agent")]),
                result("a", "files"),
                Message::user_text("steer"),
                result("b", "agent done"),
            ],
            expected: Some(vec![
                calls(&[("a", "bash"), ("b", "one_agent")]),
                result("a", "files"),
                placeholder("b"),
                Message::user_text("steer"),
                late_user("b", "one_agent", "agent done"),
            ]),
        },
        Case {
            name: "never delivered",
            input: vec![call("a", "one_agent"), Message::user_text("next")],
            expected: Some(vec![
                call("a", "one_agent"),
                placeholder("a"),
                Message::user_text("next"),
            ]),
        },
    ];

    for case in cases {
        let normalized = normalize_late_tool_results(&case.input);
        match case.expected {
            None => {
                assert!(
                    matches!(normalized, Cow::Borrowed(_)),
                    "{} should borrow",
                    case.name
                );
                assert_eq!(normalized.as_ref(), case.input.as_slice(), "{}", case.name);
            }
            Some(expected) => {
                assert!(
                    matches!(normalized, Cow::Owned(_)),
                    "{} should own a rewrite",
                    case.name
                );
                assert_eq!(normalized.as_ref(), expected.as_slice(), "{}", case.name);
            }
        }
    }
}
