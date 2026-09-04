use pretty_assertions::assert_eq;

use super::safe_message_text;

// Covers: terminal/bidi controls cannot hide or reorder message content.
// Owner: pure text sanitization, independent of card layout.
#[test]
fn message_controls_are_visible_without_changing_normal_unicode() {
    for (input, expected) in [
        ("first\n第二行 👩‍💻", "first\n第二行 👩‍💻"),
        ("a\r\tb\x1b[2J\0", "a\\r\\tb\\u{1b}[2J\\u{0}"),
        ("a\u{202e}b\u{2069}", "a\\u{202e}b\\u{2069}"),
    ] {
        assert_eq!(safe_message_text(input), expected);
    }
}
