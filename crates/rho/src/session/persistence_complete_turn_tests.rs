use pretty_assertions::assert_eq;
use serde_json::json;

use super::insert_interrupted_tool_placeholders;
use rho_providers::model::{ContentBlock, Message, ToolCall, ToolResult};

fn call(id: &str) -> Message {
    Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
        id: id.into(),
        name: "agent".into(),
        arguments: json!({}),
    })])
}

fn result(id: &str) -> Message {
    Message::ToolResult(ToolResult {
        id: id.into(),
        ok: true,
        content: "ok".into(),
    })
}

// Covers: an undelivered async call gets an interrupted placeholder so later
// completed turns survive resume.
// Owner: session persistence
#[test]
fn uncovered_async_call_inserts_placeholder_and_keeps_later_turn() {
    let recovered = insert_interrupted_tool_placeholders(vec![
        Message::user_text("go"),
        call("a"),
        Message::user_text("later"),
        call("b"),
        result("b"),
    ]);
    assert_eq!(
        recovered,
        vec![
            Message::user_text("go"),
            call("a"),
            Message::ToolResult(ToolResult {
                id: "a".into(),
                ok: false,
                content: "tool call interrupted before completion".into(),
            }),
            Message::user_text("later"),
            call("b"),
            result("b"),
        ]
    );
}
