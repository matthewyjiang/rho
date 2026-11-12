use pretty_assertions::assert_eq;

use super::*;
use crate::model::{AbortedAssistant, AssistantMessage, ContentBlock, ImageContent};

// 2026-11-12 05:39:00 UTC.
const INSTANT: i64 = 1_794_461_940;

// Covers: the rendered line spells out weekday, date, 12-hour time, and the
// zone's UTC offset, including zero and fractional-hour offsets.
// Owner: SDK message time rendering.
#[test]
fn renders_local_time_with_utc_offset() {
    let cases = [
        (-5 * 3600, "Thursday, Nov 12, 2026, 12:39 AM (UTC-5)"),
        (0, "Thursday, Nov 12, 2026, 5:39 AM (UTC)"),
        (
            5 * 3600 + 1800,
            "Thursday, Nov 12, 2026, 11:09 AM (UTC+5:30)",
        ),
        (
            -(9 * 3600 + 1800),
            "Wednesday, Nov 11, 2026, 8:09 PM (UTC-9:30)",
        ),
    ];
    for (offset, expected) in cases {
        let time = MessageTime::from_parts(INSTANT, offset).unwrap();
        assert_eq!(time.to_string(), expected, "offset {offset}");
    }
}

// Covers: a stored time round-trips through session JSON, and an offset no
// zone can have is rejected at the boundary instead of rendering garbage.
// Owner: SDK message time persistence.
#[test]
fn serde_round_trips_and_rejects_impossible_offsets() {
    let time = MessageTime::from_parts(INSTANT, -5 * 3600).unwrap();
    let json = serde_json::to_value(time).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"unix_seconds": INSTANT, "utc_offset_seconds": -18000})
    );
    assert_eq!(serde_json::from_value::<MessageTime>(json).unwrap(), time);
    assert!(serde_json::from_value::<MessageTime>(
        serde_json::json!({"unix_seconds": INSTANT, "utc_offset_seconds": 90_000})
    )
    .is_err());
}

fn at(offset_minutes: i64) -> MessageTime {
    MessageTime::from_parts(INSTANT + offset_minutes * 60, -5 * 3600).unwrap()
}

// Covers: a user stamp spells out when the message was sent and how the reply
// before it ended, and a reader sees past exactly that line and nothing that
// only resembles it.
// Owner: SDK user stamp rendering.
#[test]
fn user_stamps_render_and_strip_exactly() {
    let cases = [
        (None, "Thursday, Nov 12, 2026, 12:39 AM (UTC-5)"),
        (
            Some(ReplyEnd::Finished(at(-1))),
            "Thursday, Nov 12, 2026, 12:39 AM (UTC-5). Previous reply finished Thursday, Nov 12, 2026, 12:38 AM (UTC-5).",
        ),
        (
            Some(ReplyEnd::Interrupted(at(-2))),
            "Thursday, Nov 12, 2026, 12:39 AM (UTC-5). Previous reply interrupted at Thursday, Nov 12, 2026, 12:37 AM (UTC-5).",
        ),
    ];
    for (previous_reply, expected) in cases {
        let stamp = UserStamp {
            sent: at(0),
            previous_reply,
        };
        assert_eq!(stamp.to_string(), expected);
        assert_eq!(
            UserStamp::strip_rendered_prefix(&format!("{stamp}\nhello\nthere")),
            Some("hello\nthere")
        );
        assert_eq!(UserStamp::strip_rendered_prefix(expected), Some(""));
    }
    for text in [
        "hello",
        "Friday, Nov 12, 2026, 12:39 AM (UTC-5)\nhello",
        "Thursday, Nov 12, 2026, 12:39 AM (UTC-05)\nhello",
        "Thursday, Nov 12, 2026, 12:39 AM (UTC-5) note\nhello",
        "Thursday, Nov 12, 2026, 12:39 AM (UTC-5). Previous reply ended Thursday, Nov 12, 2026, 12:38 AM (UTC-5).\nhello",
    ] {
        assert_eq!(UserStamp::strip_rendered_prefix(text), None, "{text}");
    }
}

// Covers: the previous reply is the one answering the latest user message,
// finished or cut off, and only when it recorded its time.
// Owner: SDK user stamp history reading.
#[test]
fn previous_reply_is_the_latest_users_timed_reply() {
    let finished = |time| {
        Message::assistant(AssistantMessage {
            created_at: time,
            ..AssistantMessage::from_content(vec![ContentBlock::Text("done".into())])
        })
    };
    let cut_off = Message::AbortedAssistant(Box::new(AbortedAssistant {
        stopped_at: Some(at(-2)),
        ..AbortedAssistant::default()
    }));
    let tool_result = Message::ToolResult(crate::model::ToolResult::succeeded("call", "ok"));
    let user = Message::user_text("hi");
    let cases = [
        ("no reply yet", vec![], None),
        (
            "finished after tools",
            vec![
                user.clone(),
                finished(Some(at(-3))),
                tool_result.clone(),
                finished(Some(at(-1))),
            ],
            Some(ReplyEnd::Finished(at(-1))),
        ),
        (
            "cut off",
            vec![
                user.clone(),
                finished(Some(at(-3))),
                tool_result.clone(),
                cut_off,
            ],
            Some(ReplyEnd::Interrupted(at(-2))),
        ),
        (
            "unanswered user",
            vec![finished(Some(at(-3))), user.clone()],
            None,
        ),
        (
            "untimed reply",
            vec![user, finished(None), tool_result],
            None,
        ),
    ];
    for (name, history, expected) in cases {
        assert_eq!(ReplyEnd::last_in(&history), expected, "{name}");
    }
}

// Covers: stamping user input puts the stamp on its own line before the
// text, and in a block of its own when the input is only images.
// Owner: SDK user input stamping.
#[test]
fn stamped_input_leads_with_its_stamp() {
    let stamp = UserStamp {
        sent: at(0),
        previous_reply: None,
    };
    let image = ContentBlock::Image(ImageContent {
        data: "AA==".into(),
        mime_type: "image/png".into(),
    });
    let cases = [
        (
            vec![ContentBlock::Text("look".into()), image.clone()],
            vec![ContentBlock::Text(format!("{stamp}\nlook")), image.clone()],
        ),
        (
            vec![image.clone()],
            vec![ContentBlock::Text(stamp.to_string()), image.clone()],
        ),
    ];
    for (content, expected) in cases {
        let input = crate::UserInput::content(content).unwrap().stamped(stamp);
        assert_eq!(input.blocks(), expected.as_slice());
    }
}
