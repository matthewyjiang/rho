use pretty_assertions::assert_eq;

use super::*;

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

// Covers: stamping user input puts the time on its own line before the
// text, and in a block of its own when the input is only images.
// Owner: SDK user input stamping.
#[test]
fn stamped_input_leads_with_its_time() {
    let time = MessageTime::from_parts(INSTANT, -5 * 3600).unwrap();
    let image = ContentBlock::Image(crate::model::ImageContent {
        data: "AA==".into(),
        mime_type: "image/png".into(),
    });
    let stamp = "Thursday, Nov 12, 2026, 12:39 AM (UTC-5)";
    let cases = [
        (
            vec![ContentBlock::Text("look".into()), image.clone()],
            vec![ContentBlock::Text(format!("{stamp}\nlook")), image.clone()],
        ),
        (
            vec![image.clone()],
            vec![ContentBlock::Text(stamp.into()), image.clone()],
        ),
    ];
    for (content, expected) in cases {
        let input = crate::UserInput::content(content).unwrap().stamped(time);
        assert_eq!(input.blocks(), expected.as_slice());
    }
}

// Covers: a reader can see past exactly the line a time renders as, for every
// offset form, and nothing that only resembles one.
// Owner: SDK message time rendering.
#[test]
fn strips_only_a_rendered_time_line() {
    for offset in [-5 * 3600, 0, 5 * 3600 + 1800, -(9 * 3600 + 1800)] {
        let time = MessageTime::from_parts(INSTANT, offset).unwrap();
        let text = format!("{time}\nhello\nthere");
        assert_eq!(
            MessageTime::strip_rendered_prefix(&text),
            Some("hello\nthere"),
            "{text}"
        );
        assert_eq!(
            MessageTime::strip_rendered_prefix(&time.to_string()),
            Some("")
        );
    }
    for text in [
        "hello",
        "Thursday, Nov 12, 2026, 12:39 AM\nhello",
        "Friday, Nov 12, 2026, 12:39 AM (UTC-5)\nhello",
        "Thursday, Nov 12, 2026, 12:39 AM (UTC-05)\nhello",
        "Thursday, Nov 12, 2026, 12:39 AM (UTC-5) note\nhello",
    ] {
        assert_eq!(MessageTime::strip_rendered_prefix(text), None, "{text}");
    }
}
