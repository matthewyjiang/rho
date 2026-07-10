use pretty_assertions::assert_eq;

use super::utf16_with_nul;

#[test]
fn encodes_clipboard_text_as_null_terminated_utf16() {
    assert_eq!(
        utf16_with_nul("hello 🙂"),
        vec![104, 101, 108, 108, 111, 32, 0xD83D, 0xDE42, 0]
    );
}
