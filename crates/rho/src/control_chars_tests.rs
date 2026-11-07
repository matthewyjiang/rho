use pretty_assertions::assert_eq;

use super::{escape_controls, LineBreaks};

// Covers: escape sequences, carriage returns, and bidirectional overrides
// are escaped whether line breaks are kept or not; line breaks and tabs
// are kept only when asked, a CRLF line ending as a line break.
// Owner: terminal text escaping (unit).
#[test]
fn controls_are_escaped_and_line_breaks_kept_only_when_asked() {
    let text = "plan\r\n\t1 keep\x1b[8m\r\u{202e}é";
    assert_eq!(
        (
            escape_controls(text, LineBreaks::Keep),
            escape_controls(text, LineBreaks::Escape)
        ),
        (
            "plan\n\t1 keep\\u{1b}[8m\\r\\u{202e}é".to_owned(),
            "plan\\r\\n\\t1 keep\\u{1b}[8m\\r\\u{202e}é".to_owned()
        )
    );
}
