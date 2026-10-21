use pretty_assertions::assert_eq;

use super::*;

// Covers: head truncation leaves short values alone and never splits a
// character.
// Owner: hook payload bounds
#[test]
fn truncation_never_splits_a_character() {
    let cases = [
        ("short value", "keep", 16, "keep", false),
        ("long value", "0123456789", 4, "0123", true),
        // Four bytes of one multi-byte character; a three-byte bound drops it.
        ("multi-byte cut", "\u{1F600}tail", 3, "", true),
    ];
    for (case, value, max_field_bytes, expected, cut) in cases {
        let mut value = value.to_owned();
        let truncated = truncate_field(&mut value, HookPayloadBounds::new(max_field_bytes, 64));
        assert_eq!((value.as_str(), truncated), (expected, cut), "{case}");
    }
}

// Covers: process output keeps its last bytes, where errors are, and the cut
// never splits a character or goes unreported.
// Owner: hook payload bounds
#[test]
fn tail_bounds_keep_the_end_on_a_character_boundary() {
    let bounds = HookPayloadBounds::new(5, 64);
    let cases = [
        ("short", "short", false),
        ("0123456789", "56789", true),
        // The cut lands inside the four-byte emoji, so the emoji is dropped.
        ("ab\u{1F600}xyz", "xyz", true),
    ];
    for (value, expected, cut) in cases {
        let mut truncation = HookTruncation::default();
        let kept = bounded_tail_string(value, "payload.process.stderr", bounds, &mut truncation);
        assert_eq!(
            (kept.as_str(), truncation.is_truncated()),
            (expected, cut),
            "{value}"
        );
    }
}

#[test]
fn bounds_never_go_below_one_byte() {
    let bounds = HookPayloadBounds::new(0, 0);
    assert_eq!(bounds.max_field_bytes(), 1);
    assert_eq!(bounds.max_envelope_bytes(), 1);
}
