use std::collections::BTreeSet;

use pretty_assertions::assert_eq;
use serde_json::json;

use super::PendingAsyncCalls;
use crate::model::{ContentBlock, ImageContent, Message, ToolCall, ToolResult};

fn call(id: &str) -> Message {
    Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
        id: id.into(),
        name: "tool".into(),
        arguments: json!({}),
    })])
}

fn result(id: &str) -> Message {
    Message::ToolResult(ToolResult {
        id: id.into(),
        ok: true,
        content: "done".into(),
    })
}

fn image(id: &str) -> Message {
    Message::tool_image_supplement(
        "tool",
        id,
        vec![ImageContent {
            mime_type: "image/png".into(),
            data: "AA==".into(),
        }],
    )
    .unwrap()
}

// Covers: compacting while async jobs run must keep every running call and any
// result or image after the cut paired with its call, and must not compact a
// prefix that holds no model reply.
// Owner: sdk orchestration compaction.
#[test]
fn locate_cuts_before_running_calls_without_splitting_pairs() {
    let cases = [
        (
            "no running jobs",
            vec![Message::user_text("go"), call("a")],
            vec![],
            PendingAsyncCalls::None,
        ),
        (
            "cut at the earliest running call",
            vec![
                Message::System("rules".into()),
                Message::user_text("go"),
                call("done"),
                result("done"),
                call("late"),
                Message::assistant_text("working"),
                call("later"),
            ],
            vec!["later", "late"],
            PendingAsyncCalls::CompactBefore(4),
        ),
        (
            "late result moves the cut to its call",
            vec![
                Message::user_text("go"),
                Message::assistant_text("hi"),
                Message::user_text("more"),
                call("finished"),
                call("running"),
                result("finished"),
                image("finished"),
            ],
            vec!["running"],
            PendingAsyncCalls::CompactBefore(3),
        ),
        (
            "late image alone moves the cut to its call",
            vec![
                Message::user_text("go"),
                Message::assistant_text("hi"),
                call("finished"),
                result("finished"),
                call("running"),
                image("finished"),
            ],
            vec!["running"],
            PendingAsyncCalls::CompactBefore(2),
        ),
        (
            "running call in the first reply",
            vec![
                Message::System("rules".into()),
                Message::user_text("go"),
                call("running"),
            ],
            vec!["running"],
            PendingAsyncCalls::Blocking,
        ),
        (
            "running call missing from history",
            vec![Message::user_text("go"), call("done"), result("done")],
            vec!["missing"],
            PendingAsyncCalls::Blocking,
        ),
    ];
    for (name, history, running, expected) in cases {
        let running = running.into_iter().collect::<BTreeSet<_>>();
        assert_eq!(
            PendingAsyncCalls::locate(&history, &running),
            expected,
            "{name}"
        );
    }
}
