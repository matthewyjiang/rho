use pretty_assertions::assert_eq;

use super::*;
use crate::model::{AssistantMessage, ContentBlock, ImageContent, Message};

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

// Covers: a reader sees past exactly one rendered stamp line and nothing that
// only resembles it.
// Owner: SDK user stamp rendering.
#[test]
fn strip_rendered_prefix_sees_past_exactly_the_stamp() {
    let stamp = at(0).to_string();
    assert_eq!(
        MessageTime::strip_rendered_prefix(&format!("{stamp}\nhello\nthere")),
        Some("hello\nthere")
    );
    assert_eq!(MessageTime::strip_rendered_prefix(&stamp), Some(""));
    for text in [
        "hello",
        "Friday, Nov 12, 2026, 12:39 AM (UTC-5)\nhello",
        "Thursday, Nov 12, 2026, 12:39 AM (UTC-05)\nhello",
        "Thursday, Nov 12, 2026, 12:39 AM (UTC-5) note\nhello",
    ] {
        assert_eq!(MessageTime::strip_rendered_prefix(text), None, "{text}");
    }
}

// Covers: sessions saved while replies recorded their finish or cut-off time
// still load, and the user text stamped then is kept byte for byte so a
// resume sends what was sent before.
// Owner: SDK session history persistence.
#[test]
fn sessions_saved_with_reply_times_still_load() {
    let old_stamp = "Thursday, Nov 12, 2026, 12:39 AM (UTC-5). Previous reply finished Thursday, Nov 12, 2026, 12:38 AM (UTC-5).\nnext";
    let history = vec![
        Message::user_text("hello"),
        Message::assistant(AssistantMessage::from_content(vec![ContentBlock::Text(
            "done".into(),
        )])),
        Message::AbortedAssistant(Box::default()),
        Message::user_text(old_stamp),
    ];
    let time = serde_json::to_value(at(-1)).unwrap();
    let mut saved = serde_json::to_value(&history).unwrap();
    saved[1]["EnrichedAssistant"]["created_at"] = time.clone();
    saved[2]["AbortedAssistant"]["stopped_at"] = time;

    assert_eq!(
        serde_json::from_value::<Vec<Message>>(saved).unwrap(),
        history
    );
}

// Covers: stamping user input puts the stamp on its own line before the
// text, and in a block of its own when the input is only images.
// Owner: SDK user input stamping.
#[test]
fn stamped_input_leads_with_its_stamp() {
    let stamp = at(0);
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
