use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use PermissionMode::{Bypass, Plan};

// Covers: blocking extensions need exact headless replies; unknown/nonblocking
// methods must remain available for method_not_found, not invented contracts.
// Owner: Cursor extension wire policy.
#[test]
fn answers_blocking_extensions_as_exact_json() {
    for (method, mode, expected) in [
        (
            "cursor/ask_question",
            Bypass,
            Some(
                json!({"outcome": {"outcome": "skipped", "reason": "no interactive user; pick the best option and continue"}}),
            ),
        ),
        (
            "cursor/ask_question",
            Plan,
            Some(
                json!({"outcome": {"outcome": "skipped", "reason": "no interactive user; pick the best option and continue"}}),
            ),
        ),
        (
            "cursor/create_plan",
            Bypass,
            Some(json!({"outcome": {"outcome": "accepted"}})),
        ),
        (
            "cursor/create_plan",
            Plan,
            Some(
                json!({"outcome": {"outcome": "rejected", "reason": "plan-only run: report the plan as your final answer"}}),
            ),
        ),
        ("cursor/update_todos", Bypass, None),
        ("cursor/task", Plan, None),
        ("cursor/generate_image", Bypass, None),
        ("cursor/future_method", Bypass, None),
        ("another/method", Bypass, None),
    ] {
        // Policy deliberately does not read presentation fields or sessionId.
        for params in [
            json!({}),
            json!({"toolCallId": "tool", "questions": [], "plan": "# A plan"}),
        ] {
            assert_eq!(
                answer(mode, method, &params),
                expected,
                "{mode:?}: {method}"
            );
        }
    }
}

// Covers: recorded Cursor new-file pseudo headers must not render as file
// contents, on either update shape. Real diffs/text/metadata remain unchanged.
// Owner: Cursor wire normalization (generic rendering must not know the quirk).
#[test]
fn normalizes_recorded_diff_shapes_without_changing_other_fields() {
    let cases = [
        // cursor_agent_tools.jsonl and cursor_permission_reject.jsonl.
        (
            Some("-- /dev/null"),
            "++ b//workspace/out.txt\nhi",
            None,
            "hi",
        ),
        (
            Some("-- /dev/null"),
            "++ b//workspace/out.txt\nfirst\nsecond\n",
            None,
            "first\nsecond\n",
        ),
        (Some("-- /dev/null"), "++ b//workspace/out.txt", None, ""),
        (
            Some("-- /dev/null"),
            "content without a pseudo header",
            None,
            "content without a pseudo header",
        ),
        // Existing-file shape from cursor_agent_tools.jsonl.
        (
            Some("hello from spike\n"),
            "hello from SPIKE\n",
            Some("hello from spike\n"),
            "hello from SPIKE\n",
        ),
        // A real new-file diff needs no normalization, even with header-like text.
        (
            None,
            "++ b/this is real file content\nhi",
            None,
            "++ b/this is real file content\nhi",
        ),
    ];
    for shape in ["tool_call", "tool_call_update"] {
        for (old, new, expected_old, expected_new) in cases {
            let mut wire = json!({
                "sessionUpdate": shape, "toolCallId": "tool_07500270-d46b-4052-af9a-c293cfb3f7e",
                "title": "Edit /workspace/out.txt", "kind": "edit", "status": "completed",
                "rawInput": {"path": "/workspace/out.txt"}, "rawOutput": {"untouched": true},
                "locations": [{"path": "/workspace/out.txt", "line": 1}], "_meta": {"keep": "call metadata"},
                "content": [
                    {"type": "content", "content": {"type": "text", "text": "keep this"}},
                    {"type": "diff", "path": "/workspace/out.txt", "oldText": old, "newText": new, "_meta": {"keep": "diff metadata"}},
                    {"type": "terminal", "terminalId": "terminal-1"}
                ]
            });
            let input: SessionUpdate = serde_json::from_value(wire.clone()).unwrap();
            wire["content"][1]["oldText"] = json!(expected_old);
            wire["content"][1]["newText"] = json!(expected_new);
            let expected: SessionUpdate = serde_json::from_value(wire).unwrap();
            assert_eq!(
                normalize_update(input),
                expected,
                "{shape}: old={old:?}, new={new:?}"
            );
        }
    }
    for wire in [
        json!({"sessionUpdate": "tool_call_update", "toolCallId": "tool", "status": "completed"}),
        json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "untouched"}}),
    ] {
        let update: SessionUpdate = serde_json::from_value(wire).unwrap();
        assert_eq!(normalize_update(update.clone()), update);
    }
}
