use super::*;
use crate::tui::composer_buffer::ComposerEditKey;

#[test]
fn valid_slash_commands_are_added_to_input_history() {
    let mut app = test_app();
    app.input_ui.set_text("/info  ".to_string());
    app.input_ui.set_cursor(app.input_char_len());

    let invocation = app.parse_input_command().unwrap().unwrap();

    assert_eq!(invocation.id, CommandId::Info);
    assert_eq!(app.input_ui.history(), ["/info"]);
    app.input_ui.clear_text();
    app.input_ui.set_cursor(0);
    app.apply_input_edit_key(ComposerEditKey::Up);
    assert_eq!(app.input_ui.text(), "/info");
}

#[test]
fn left_and_right_arrows_treat_collapsed_paste_as_one_character() {
    let mut app = test_app();
    app.insert_input_text("a");
    app.insert_pasted_input_text(&collapsible_paste());
    let segment = app.input_ui.paste_segments()[0].clone();

    app.apply_input_edit_key(ComposerEditKey::Left);
    assert_eq!(app.input_ui.cursor(), segment.start);

    app.apply_input_edit_key(ComposerEditKey::Right);
    assert_eq!(app.input_ui.cursor(), segment.end());

    app.apply_input_edit_key(ComposerEditKey::Left);
    app.apply_input_edit_key(ComposerEditKey::Left);
    assert_eq!(app.input_ui.cursor(), segment.start - 1);
}

#[test]
fn vertical_cursor_movement_focuses_a_collapsed_paste_item() {
    let mut app = test_app();
    app.insert_input_text("first line\n");
    app.insert_pasted_input_text(&collapsible_paste());
    let segment = app.input_ui.paste_segments()[0].clone();
    app.input_ui.set_cursor(5);

    app.apply_input_edit_key(ComposerEditKey::Down);

    assert_eq!(app.input_ui.cursor(), segment.start);
}

// Covers: Backspace after, Backspace from inside, and Delete before a
// collapsed paste each remove the whole item and its expansion metadata.
// Owner: pure unit (composer edit ops)
#[test]
fn backspace_removes_collapsed_paste_as_one_item() {
    // (case, text before the paste, text after it, cursor, key, text, cursor)
    let cases = [
        (
            "backspace after",
            "",
            "",
            None,
            ComposerEditKey::Backspace,
            "",
            0,
        ),
        (
            "backspace inside",
            "",
            "",
            Some(5),
            ComposerEditKey::Backspace,
            "",
            0,
        ),
        (
            "delete before",
            "before ",
            " after",
            Some(7),
            ComposerEditKey::Delete,
            "before  after",
            7,
        ),
    ];
    for (case, before, after, cursor, key, expected_text, expected_cursor) in cases {
        let mut app = test_app();
        app.insert_input_text(before);
        app.insert_pasted_input_text(&collapsible_paste());
        app.insert_input_text(after);
        if let Some(cursor) = cursor {
            app.input_ui.set_cursor(cursor);
        }

        app.apply_input_edit_key(key);

        assert_eq!(app.input_ui.text(), expected_text, "{case}");
        assert_eq!(app.input_ui.cursor(), expected_cursor, "{case}");
        assert!(app.input_ui.paste_segments().is_empty(), "{case}");
    }
}

// Covers: typing over a non-empty composer selection replaces the span.
// Owner: pure unit (composer edit ops)
#[test]
fn typing_replaces_composer_selection() {
    let mut app = test_app();
    app.insert_input_text("grab this text");
    app.input_ui.buffer_mut().begin_selection(5);
    app.input_ui.buffer_mut().update_selection(9); // "this"
    app.input_ui.finalize_selection();

    app.insert_input_char('X');

    assert_eq!(app.input_ui.text(), "grab X text");
    assert_eq!(app.input_ui.cursor(), 6);
    assert!(app.input_ui.selection_range().is_none());
}

// Covers: backspace/delete over a selection remove the span once.
// Owner: pure unit (composer edit ops)
#[test]
fn delete_and_backspace_remove_composer_selection() {
    let mut app = test_app();
    app.insert_input_text("abcdef");
    app.input_ui.buffer_mut().begin_selection(2);
    app.input_ui.buffer_mut().update_selection(5); // "cde"
    app.input_ui.finalize_selection();
    app.apply_input_edit_key(ComposerEditKey::Backspace);
    assert_eq!(app.input_ui.text(), "abf");

    app.input_ui.clear_text();
    app.input_ui.set_cursor(0);
    app.insert_input_text("abcdef");
    app.input_ui.buffer_mut().begin_selection(2);
    app.input_ui.buffer_mut().update_selection(5); // "cde"
    app.input_ui.finalize_selection();
    app.apply_input_edit_key(ComposerEditKey::Delete);
    assert_eq!(app.input_ui.text(), "abf");
}

// Covers: double-click word selection survives release and extends from the
// original token edge in either drag direction.
// Owner: pure unit (composer selection ops)
#[test]
fn range_selection_preserves_and_extends_its_base_range() {
    let mut app = test_app();
    app.insert_input_text("grab this text");
    let range = super::super::paste_burst::word_range_at(app.input_ui.text(), 7);

    app.input_ui
        .buffer_mut()
        .select_range(range.start, range.end);
    app.input_ui.buffer_mut().update_selection(7);
    app.input_ui.finalize_selection();
    assert_eq!(app.input_ui.selection_range(), Some(5..9));

    app.input_ui
        .buffer_mut()
        .select_range(range.start, range.end);
    app.input_ui.buffer_mut().update_selection(2);
    app.input_ui.finalize_selection();
    assert_eq!(app.input_ui.selection_range(), Some(2..9));

    app.input_ui
        .buffer_mut()
        .select_range(range.start, range.end);
    app.input_ui.buffer_mut().update_selection(12);
    app.input_ui.finalize_selection();
    assert_eq!(app.input_ui.selection_range(), Some(5..12));

    app.insert_input_char('X');
    assert_eq!(app.input_ui.text(), "grab Xxt");
}

// Covers: an edit that touches any part of a collapsed paste marker must
// consume the whole marker and its expansion metadata.
// Owner: pure unit (composer edit ops)
#[test]
fn partial_selection_consumes_whole_collapsed_paste() {
    let mut app = test_app();
    app.insert_input_text("before ");
    app.insert_pasted_input_text(&collapsible_paste());
    app.insert_input_text(" after");
    let segment = app.input_ui.paste_segments()[0].clone();

    app.input_ui.buffer_mut().begin_selection(segment.start - 2);
    app.input_ui
        .buffer_mut()
        .update_selection(segment.start + 3);
    app.input_ui.finalize_selection();
    app.insert_input_char('X');

    assert_eq!(app.input_ui.text(), "beforX after");
    assert!(app.input_ui.paste_segments().is_empty());

    let mut app = test_app();
    app.insert_input_text("before ");
    app.insert_pasted_input_text(&collapsible_paste());
    app.insert_input_text("XYZ");
    let segment = app.input_ui.paste_segments()[0].clone();
    app.input_ui.buffer_mut().begin_selection(segment.end() + 1);
    app.input_ui
        .buffer_mut()
        .update_selection(segment.end() - 3);
    app.input_ui.finalize_selection();
    app.apply_input_edit_key(ComposerEditKey::Delete);

    assert_eq!(app.input_ui.text(), "before YZ");
    assert!(app.input_ui.paste_segments().is_empty());
}
