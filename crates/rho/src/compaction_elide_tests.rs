use pretty_assertions::assert_eq;
use rho_sdk::model::{ImageContent, ToolCall, ToolResultStatus};
use serde_json::json;

use super::*;

fn call(id: &str, path: &str) -> Message {
    Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
        id: id.into(),
        name: "read_file".into(),
        arguments: json!({"path": path}),
    })])
}

fn result(id: &str, status: ToolResultStatus, bytes: usize) -> ToolResult {
    ToolResult::new(id, status, "x".repeat(bytes))
}

fn stub(result: &ToolResult, path: &str, status: &str, images: &str) -> Message {
    Message::ToolResult(ToolResult::new(
        result.id.clone(),
        result.status,
        format!(
            "[elided tool result: read_file path={path} · {status} · {} bytes{images} · recall_id={}; fetch the text with the sessions tool, action=recall]",
            result.content.len(),
            recall_id(result),
        ),
    ))
}

// Covers: oldest-first selection outside the recent tail, the minimum-size
// skip, stopping once the target is reached, and call/result pairing.
// Owner: elision tier selection policy.
#[test]
fn elides_oldest_large_results_outside_the_tail_until_target() {
    let old = result("a", ToolResultStatus::Succeeded, 8_000);
    let small = result("b", ToolResultStatus::Failed, 200);
    let middle = result("c", ToolResultStatus::Failed, 8_000);
    let recent = result("d", ToolResultStatus::Succeeded, 8_000);
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
        let mut originals = Vec::new();
        for index in &elided_indexes {
            let (original, path, status) = match index {
                3 => (&old, "src/a.rs", "ok"),
                _ => (&middle, "src/c.rs", "error"),
            };
            expected[*index] = stub(original, path, status, "");
            originals.push(original.clone());
        }
        assert_eq!(
            elision,
            Elision {
                messages: expected,
                originals,
            },
            "{case}"
        );
    }
}

// Covers: an elided result drops its images even when its text is small, the
// stub records the image count, and the original keeps the images.
// Owner: elision tier image handling.
#[test]
fn elided_result_drops_its_images() {
    let shot = result("shot", ToolResultStatus::Succeeded, 10).with_images(vec![ImageContent {
        mime_type: "image/png".into(),
        data: "a".repeat(20_000),
    }]);
    let messages = vec![
        Message::System("system".into()),
        Message::user_text("look"),
        call("shot", "shot.png"),
        Message::ToolResult(shot.clone()),
        Message::user_text("recent"),
        Message::assistant_text("y".repeat(4_000)),
    ];

    let elision = elide_tool_results(&messages, &[], 1_000).unwrap();

    assert_eq!(
        elision,
        Elision {
            messages: vec![
                messages[0].clone(),
                messages[1].clone(),
                messages[2].clone(),
                stub(&shot, "shot.png", "ok", " + 1 image"),
                messages[4].clone(),
                messages[5].clone(),
            ],
            originals: vec![shot],
        }
    );
}

// Covers: nothing below the size floor or outside the tail means no elision,
// so the compactor escalates without saving recall files.
// Owner: elision tier selection policy.
#[test]
fn returns_none_when_nothing_is_eligible() {
    let messages = vec![
        Message::System("system".into()),
        Message::user_text("x".repeat(8_000)),
        call("a", "a"),
        Message::ToolResult(result("a", ToolResultStatus::Succeeded, 100)),
        Message::user_text("recent"),
    ];

    assert_eq!(elide_tool_results(&messages, &[], 100), None);
}

// Covers: providers that reuse call ids (`call_0` every turn). Each result
// keeps its own call summary, so eliding only the first result neither drops
// the second's image nor borrows its tool name.
// Owner: elision tier ownership.
#[test]
fn reused_call_ids_pair_by_position() {
    let first = result("call_0", ToolResultStatus::Succeeded, 8_000);
    let second = result("call_0", ToolResultStatus::Succeeded, 10);
    let image = ImageContent {
        mime_type: "image/png".into(),
        data: "a".repeat(8_000),
    };
    let messages = vec![
        Message::System("system".into()),
        Message::user_text("go"),
        call("call_0", "first.rs"),
        Message::ToolResult(first.clone()),
        call("call_0", "second.png"),
        Message::ToolResult(second.with_images(vec![image])),
        Message::user_text("recent"),
        Message::assistant_text("y".repeat(4_000)),
    ];
    let full = estimate_context_tokens(&messages, &[]);

    let elision = elide_tool_results(&messages, &[], full - 1_000).unwrap();

    let mut expected = messages.clone();
    expected[3] = stub(&first, "first.rs", "ok", "");
    assert_eq!(
        elision,
        Elision {
            messages: expected,
            originals: vec![first],
        }
    );
}
