use crossterm::event::{KeyCode, KeyEvent};
use pretty_assertions::assert_eq;

use super::{normal_cursor, VimMode, VimOutcome, VimState};
use crate::tui::composer_buffer::{ComposerBuffer, ComposerEditKey, EditOutcome};

/// `⎋` is Esc, `⌂` is Home; every other char is typed as itself.
fn keys(spec: &str) -> impl Iterator<Item = KeyEvent> + '_ {
    spec.chars().map(|ch| match ch {
        '⎋' => KeyEvent::from(KeyCode::Esc),
        '⌂' => KeyEvent::from(KeyCode::Home),
        ch => KeyEvent::from(KeyCode::Char(ch)),
    })
}

/// Feed `spec` the way the main composer does: vim first, then the shared
/// edit path. Starts in insert mode with the caret at `cursor`.
fn run(text: &str, cursor: usize, spec: &str) -> (String, usize, VimMode) {
    let mut buffer = ComposerBuffer::default();
    buffer.replace_all(text.into(), Vec::new());
    buffer.set_cursor(cursor);
    let mut vim = VimState::default();
    for key in keys(spec) {
        let edit = match vim.handle_key(key, &mut buffer) {
            VimOutcome::Handled(_) => continue,
            VimOutcome::Unhandled => ComposerEditKey::from_key(key).expect("typed key"),
            VimOutcome::Forward(edit) => edit,
        };
        if let EditOutcome::VerticalEdge(direction) = buffer.apply_edit(edit) {
            buffer.move_vertically(direction);
        }
    }
    let cursor = if vim.mode() == VimMode::Normal {
        normal_cursor(&buffer)
    } else {
        buffer.cursor()
    };
    (buffer.text().to_owned(), cursor, vim.mode())
}

// Covers: normal-mode motions, operators, counts, text objects, registers,
// and undo grouping produce vim's text and caret.
// Owner: vim key interpreter (pure buffer logic; PTY covers routing/chrome).
#[test]
fn normal_mode_commands_edit_like_vim() {
    use VimMode::{Insert, Normal};
    let cases = [
        (
            "esc steps onto last char",
            "hello",
            5,
            "⎋",
            "hello",
            4,
            Normal,
        ),
        ("dw", "hello world", 0, "⎋dw", "world", 0, Normal),
        (
            "Home is not a count digit",
            "hello",
            5,
            "⎋2⌂x",
            "ello",
            0,
            Normal,
        ),
        (
            "dw stops at line break",
            "one two\nthree",
            5,
            "⎋dw",
            "one \nthree",
            3,
            Normal,
        ),
        (
            "cw keeps the blank",
            "foo bar",
            1,
            "⎋cwbaz⎋",
            "baz bar",
            2,
            Normal,
        ),
        (
            "ciw",
            "say hello there",
            7,
            "⎋ciwbye⎋",
            "say bye there",
            6,
            Normal,
        ),
        ("daw", "say hello there", 7, "⎋daw", "say there", 4, Normal),
        ("dd", "a\nb\nc", 3, "⎋dd", "a\nc", 2, Normal),
        ("dd last line", "a\nb", 3, "⎋dd", "a", 0, Normal),
        ("count dd", "a\nb\nc", 1, "⎋2dd", "c", 0, Normal),
        ("dG", "a\nb\nc", 3, "⎋dG", "a", 0, Normal),
        ("yy p", "a\nb", 1, "⎋yyp", "a\na\nb", 2, Normal),
        (
            "dd put supplies trailing separator",
            "a\nb",
            1,
            "⎋ddp",
            "b\na",
            2,
            Normal,
        ),
        (
            "last-line dd put supplies leading separator",
            "a\nb",
            3,
            "⎋ddp",
            "a\nb",
            2,
            Normal,
        ),
        ("x count then P", "abcd", 1, "⎋2xP", "abcd", 1, Normal),
        ("D", "foo bar", 4, "⎋D", "foo", 2, Normal),
        ("dt", "a(b)c", 1, "⎋dt)", ")c", 0, Normal),
        (
            "count motion",
            "one two three",
            1,
            "⎋2wD",
            "one two ",
            7,
            Normal,
        ),
        ("r count", "abc", 1, "⎋2rx", "xxc", 1, Normal),
        ("A", "ab", 1, "⎋Ac⎋", "abc", 2, Normal),
        ("o then one undo", "a", 1, "⎋ob⎋u", "a", 0, Normal),
        (
            "cc stays in insert",
            "a\nbc\nd",
            3,
            "⎋cc",
            "a\n\nd",
            2,
            Insert,
        ),
        (
            "u undoes a typed word",
            "",
            0,
            "hello world⎋u",
            "hello ",
            5,
            Normal,
        ),
        ("each x undoes alone", "abc", 1, "⎋xxu", "bc", 0, Normal),
        (
            "count dw crosses lines",
            "one\ntwo three",
            1,
            "⎋2dw",
            "three",
            0,
            Normal,
        ),
        ("dt with adjacent match", "abc", 1, "⎋dtb", "bc", 0, Normal),
    ];
    for (name, text, cursor, spec, want_text, want_cursor, want_mode) in cases {
        assert_eq!(
            run(text, cursor, spec),
            (want_text.to_owned(), want_cursor, want_mode),
            "{name}"
        );
    }
}
