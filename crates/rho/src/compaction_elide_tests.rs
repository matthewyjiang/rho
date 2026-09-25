use pretty_assertions::assert_eq;
use rho_sdk::model::{ImageContent, ToolCall};
use serde_json::json;

use super::*;

fn call(id: &str, path: &str) -> Message {
    Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
        id: id.into(),
        name: "read_file".into(),
        arguments: json!({"path": path}),
    })])
}

fn result(id: &str, ok: bool, bytes: usize) -> ToolResult {
    ToolResult {
        id: id.into(),
        ok,
        content: "x".repeat(bytes),
    }
}

fn stub(result: &ToolResult, path: &str, status: &str, images: &str) -> Message {
    Message::ToolResult(ToolResult {
        id: result.id.clone(),
        ok: result.ok,
        content: format!(
            "[elided tool result: read_file path={path} · {status} · {} bytes{images} · recall_id={}; fetch the text with the sessions tool, action=recall]",
            result.content.len(),
            recall_id(result),
        ),
    })
}

// Covers: oldest-first selection outside the recent tail, the minimum-size
// skip, stopping once the target is reached, and call/result pairing.
// Owner: elision tier selection policy.
#[test]
fn elides_oldest_large_results_outside_the_tail_until_target() {
    let old = result("a", true, 8_000);
    let small = result("b", false, 200);
    let middle = result("c", false, 8_000);
    let recent = result("d", true, 8_000);
    let messages = vec![
        Message::System("system".into()),
        Message::user_text("go"),
        call("a", "src/a.rs"),
        Message::ToolResult(old.clone()),
        call("b", "src/b.rs"),
        Message::ToolResult(small.clone()),
        call("c", "src/c.rs"),
        Message::ToolResult(middle.clone()),
        call("d", "src/d.rs"),
        Message::ToolResult(recent.clone()),
    ];
    let full = estimate_context_tokens(&messages, &[]);
    let cases = [
        // Eliding `a` alone reaches this target, so `c` stays verbatim.
        ("one", full - 1_500, vec![3]),
        // `b` is too small to elide; `d` is in the recent tail.
        ("all eligible", 3_000, vec![3, 7]),
    ];

    for (case, target, elided_indexes) in cases {
        let elision = elide_tool_results(&messages, &[], target).unwrap();

        let mut expected = messages.clone();
        for index in &elided_indexes {
            let (original, path, status) = match index {
                3 => (&old, "src/a.rs", "ok"),
                _ => (&middle, "src/c.rs", "error"),
            };
            expected[*index] = stub(original, path, status, "");
        }
        assert_eq!(
            elision,
            Elision {
                messages: expected,
                elided: elided_indexes.len(),
            },
            "{case}"
        );
    }
}

// Covers: a tool-image supplement is removed with its elided result, so no
// orphaned image message remains, and the stub records the image count.
// Owner: elision tier supplement pairing.
#[test]
fn elided_result_drops_its_image_supplement() {
    let shot = result("shot", true, 10);
    let image = ImageContent {
        mime_type: "image/png".into(),
        data: "a".repeat(20_000),
    };
    let messages = vec![
        Message::System("system".into()),
        Message::user_text("look"),
        call("shot", "shot.png"),
        Message::ToolResult(shot.clone()),
        Message::tool_image_supplement("read_file", "shot", vec![image]).unwrap(),
        Message::user_text("recent"),
        Message::assistant_text("y".repeat(4_000)),
    ];

    let elision = elide_tool_results(&messages, &[], 1_200).unwrap();

    assert_eq!(
        elision.messages,
        vec![
            messages[0].clone(),
            messages[1].clone(),
            messages[2].clone(),
            stub(&shot, "shot.png", "ok", " + 1 image"),
            messages[5].clone(),
            messages[6].clone(),
        ]
    );
}

#[test]
fn returns_none_when_nothing_is_eligible() {
    let messages = vec![
        Message::System("system".into()),
        Message::user_text("x".repeat(8_000)),
        call("a", "a"),
        Message::ToolResult(result("a", true, 100)),
        Message::user_text("recent"),
    ];

    assert_eq!(elide_tool_results(&messages, &[], 100), None);
}
