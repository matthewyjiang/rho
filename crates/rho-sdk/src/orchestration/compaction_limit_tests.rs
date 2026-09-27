use std::collections::BTreeSet;

use pretty_assertions::assert_eq;
use serde_json::json;

use super::CompactionLimit;
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

// Covers: compacting while async jobs run must keep every running call, any
// result or image after the cut, and fresh completion input verbatim; must not
// compact a prefix with no model reply; and must not recompact a prefix already
// compacted while the same jobs run.
// Owner: sdk orchestration compaction.
#[test]
fn step_limit_cuts_before_running_calls_without_splitting_pairs() {
    use CompactionLimit::{BeforePendingCalls, BlockedByPendingCalls, History};

    let two_replies = vec![
        Message::System("rules".into()),
        Message::user_text("go"),
        call("done"),
        result("done"),
        call("late"),
        Message::assistant_text("working"),
        call("later"),
    ];
    // (case, history, running, preserve_from, compacted_end, expected)
    let cases = [
        (
            "no running jobs keeps completion input",
            vec![Message::user_text("go"), call("a")],
            vec![],
            Some(1),
            Some(1),
            History {
                preserve_from: Some(1),
            },
        ),
        (
            "cut at the earliest running call",
            two_replies.clone(),
            vec!["later", "late"],
            None,
            None,
            BeforePendingCalls(4),
        ),
        (
            "completion input before the call wins",
            two_replies.clone(),
            vec!["late"],
            Some(3),
            None,
            BeforePendingCalls(3),
        ),
        (
            "completion input after the call",
            two_replies.clone(),
            vec!["late"],
            Some(6),
            None,
            BeforePendingCalls(4),
        ),
        (
            "prefix already compacted for these jobs",
            two_replies.clone(),
            vec!["late"],
            None,
            Some(4),
            BlockedByPendingCalls,
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
            None,
            None,
            BeforePendingCalls(3),
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
            None,
            None,
            BeforePendingCalls(2),
        ),
        (
            "running call in the first reply",
            vec![
                Message::System("rules".into()),
                Message::user_text("go"),
                call("running"),
            ],
            vec!["running"],
            None,
            None,
            BlockedByPendingCalls,
        ),
        (
            "running call missing from history",
            vec![Message::user_text("go"), call("done"), result("done")],
            vec!["missing"],
            None,
            None,
            BlockedByPendingCalls,
        ),
    ];
    for (name, history, running, preserve_from, compacted_end, expected) in cases {
        let running = running.into_iter().collect::<BTreeSet<_>>();
        assert_eq!(
            CompactionLimit::for_step(&history, &running, preserve_from, compacted_end),
            expected,
            "{name}"
        );
    }
}
