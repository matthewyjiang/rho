use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pretty_assertions::assert_eq;

use super::{ComposerBuffer, ComposerEditKey, Fragment};

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

#[derive(Clone, Copy, Debug)]
enum Op {
    Type(&'static str),
    Editor(&'static str),
    BeginGroup,
    EndGroup,
    Backspace,
    Paste,
    Undo,
    Redo,
}

// Covers: undo steps coalesce typing by word and deletions by run, revert
// collapsed paste markers with their content, isolate editor edits, preserve
// nested insert-session grouping across undo, and lose redo on a new edit.
// Owner: composer undo history (pure buffer logic).
#[test]
fn undo_steps_follow_words_runs_and_paste_markers() {
    let pasted = "1\n2\n3\n4\n5";
    let marker = crate::tui::paste_burst::collapsed_paste_for(pasted)
        .expect("collapses")
        .marker();
    let cases: [(&str, &[Op], String, String); 9] = [
        (
            "editor output does not absorb subsequent typing",
            &[Op::Editor("editor prompt"), Op::Type("!"), Op::Undo],
            "editor prompt".into(),
            "editor prompt".into(),
        ),
        (
            "undo seals a step without closing its insert-session group",
            &[
                Op::BeginGroup,
                Op::Type("first"),
                Op::Undo,
                Op::Type("two words"),
                Op::EndGroup,
                Op::Undo,
            ],
            "".into(),
            "".into(),
        ),
        (
            "nested owner groups stay within the insert session",
            &[
                Op::BeginGroup,
                Op::Type("one"),
                Op::BeginGroup,
                Op::Type(" two"),
                Op::EndGroup,
                Op::Type(" three"),
                Op::EndGroup,
                Op::Undo,
            ],
            "".into(),
            "".into(),
        ),
        (
            "typing undoes a word at a time",
            &[Op::Type("foo bar"), Op::Undo],
            "foo ".into(),
            "foo ".into(),
        ),
        (
            "redo reapplies",
            &[Op::Type("foo bar"), Op::Undo, Op::Undo, Op::Redo],
            "foo ".into(),
            "foo ".into(),
        ),
        (
            "a backspace run is one step",
            &[Op::Type("abc"), Op::Backspace, Op::Backspace, Op::Undo],
            "abc".into(),
            "abc".into(),
        ),
        (
            "a new edit drops redo",
            &[Op::Type("ab"), Op::Undo, Op::Type("x"), Op::Redo],
            "x".into(),
            "x".into(),
        ),
        (
            "deleting a marker undoes with its content",
            &[Op::Type("a"), Op::Paste, Op::Backspace, Op::Undo],
            format!("a{marker}"),
            format!("a{pasted}"),
        ),
        (
            "undoing a paste removes the marker",
            &[Op::Type("a"), Op::Paste, Op::Undo],
            "a".into(),
            "a".into(),
        ),
    ];
    for (name, ops, text, expanded) in cases {
        let mut buffer = ComposerBuffer::default();
        for op in ops {
            match *op {
                Op::Editor(text) => {
                    buffer.replace_range(0..buffer.char_len(), Fragment::plain(text));
                }
                Op::BeginGroup => buffer.begin_undo_group(),
                Op::EndGroup => buffer.end_undo_group(),
                Op::Type(text) => {
                    for ch in text.chars() {
                        buffer.apply_edit(ComposerEditKey::Char(ch));
                    }
                }
                Op::Backspace => {
                    buffer.apply_edit(ComposerEditKey::Backspace);
                }
                Op::Paste => {
                    let paste = crate::tui::paste_burst::collapsed_paste_for(pasted).unwrap();
                    buffer.insert_collapsed_paste(&paste, pasted);
                }
                Op::Undo => {
                    buffer.undo();
                }
                Op::Redo => {
                    buffer.redo();
                }
            }
        }
        assert_eq!(
            (buffer.text(), buffer.expanded_text()),
            (text.as_str(), expanded),
            "{name}"
        );
    }
}
