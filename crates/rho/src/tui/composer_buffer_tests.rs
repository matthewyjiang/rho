use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pretty_assertions::assert_eq;

use super::{ComposerBuffer, ComposerEditKey};

// Covers: word keys convert char cursors to byte ranges around multibyte text.
// Owner: composer buffer; PTY scenarios only edit ASCII words.
#[test]
fn word_keys_edit_multibyte_words() {
    let alt = |code| KeyEvent::new(code, KeyModifiers::ALT);
    let cases = [
        (vec![alt(KeyCode::Backspace)], "héllo ", 6),
        (
            vec![alt(KeyCode::Left), alt(KeyCode::Backspace)],
            "wörld",
            0,
        ),
        (
            vec![KeyEvent::from(KeyCode::Home), alt(KeyCode::Right)],
            "héllo wörld",
            5,
        ),
    ];
    for (keys, value, cursor) in cases {
        let mut buffer = ComposerBuffer::default();
        buffer.replace_all("héllo wörld".into(), Vec::new());
        for key in keys {
            let edit = ComposerEditKey::from_key(key).expect("edit key");
            buffer.apply_edit(edit);
        }
        assert_eq!((buffer.text(), buffer.cursor()), (value, cursor));
    }
}
