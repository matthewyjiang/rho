use super::*;

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
    app.recall_input_history_or_move_cursor(HistoryDirection::Previous, 80);
    assert_eq!(app.input_ui.text(), "/info");
}

#[test]
fn left_and_right_arrows_treat_collapsed_paste_as_one_character() {
    let mut app = test_app();
    app.insert_input_text("a");
    app.insert_pasted_input_text(&collapsible_paste());
    let segment = app.input_ui.paste_segments()[0].clone();

    app.move_input_cursor_left();
    assert_eq!(app.input_ui.cursor(), segment.start);

    app.move_input_cursor_right();
    assert_eq!(app.input_ui.cursor(), segment.end());

    app.move_input_cursor_left();
    app.move_input_cursor_left();
    assert_eq!(app.input_ui.cursor(), segment.start - 1);
}

#[test]
fn vertical_cursor_movement_focuses_a_collapsed_paste_item() {
    let mut app = test_app();
    app.insert_input_text("first line\n");
    app.insert_pasted_input_text(&collapsible_paste());
    let segment = app.input_ui.paste_segments()[0].clone();
    app.input_ui.set_cursor(5);

    app.recall_input_history_or_move_cursor(HistoryDirection::Next, 80);

    assert_eq!(app.input_ui.cursor(), segment.start);
}

#[test]
fn backspace_removes_collapsed_paste_as_one_item() {
    let mut app = test_app();
    app.insert_pasted_input_text(&collapsible_paste());

    app.backspace_input();

    assert_eq!(app.input_ui.text(), "");
    assert_eq!(app.input_ui.cursor(), 0);
    assert!(app.input_ui.paste_segments().is_empty());
}

#[test]
fn delete_removes_collapsed_paste_as_one_item() {
    let mut app = test_app();
    app.insert_input_text("before ");
    app.insert_pasted_input_text(&collapsible_paste());
    app.insert_input_text(" after");
    app.input_ui.set_cursor("before ".chars().count());

    app.delete_input();

    assert_eq!(app.input_ui.text(), "before  after");
    assert_eq!(app.input_ui.cursor(), "before ".chars().count());
    assert!(app.input_ui.paste_segments().is_empty());
}

#[test]
fn editing_from_inside_collapsed_paste_removes_the_whole_item() {
    let mut app = test_app();
    app.insert_pasted_input_text(&collapsible_paste());
    app.input_ui.set_cursor(5);

    app.backspace_input();

    assert_eq!(app.input_ui.text(), "");
    assert_eq!(app.input_ui.cursor(), 0);
    assert!(app.input_ui.paste_segments().is_empty());
}

// Covers: typing over a non-empty composer selection replaces the span.
// Owner: pure unit (composer edit ops)
#[test]
fn typing_replaces_composer_selection() {
    let mut app = test_app();
    app.insert_input_text("grab this text");
    app.input_ui.begin_selection(5);
    app.input_ui.update_selection(9); // "this"
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
    app.input_ui.begin_selection(2);
    app.input_ui.update_selection(5); // "cde"
    app.input_ui.finalize_selection();
    app.backspace_input();
    assert_eq!(app.input_ui.text(), "abf");

    app.input_ui.clear_text();
    app.input_ui.set_cursor(0);
    app.insert_input_text("abcdef");
    app.input_ui.begin_selection(2);
    app.input_ui.update_selection(5); // "cde"
    app.input_ui.finalize_selection();
    app.delete_input();
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

    app.input_ui.select_range(range.start, range.end);
    app.input_ui.update_selection(7);
    app.input_ui.finalize_selection();
    assert_eq!(app.input_ui.selection_range(), Some(5..9));

    app.input_ui.select_range(range.start, range.end);
    app.input_ui.update_selection(2);
    app.input_ui.finalize_selection();
    assert_eq!(app.input_ui.selection_range(), Some(2..9));

    app.input_ui.select_range(range.start, range.end);
    app.input_ui.update_selection(12);
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

    app.input_ui.begin_selection(segment.start - 2);
    app.input_ui.update_selection(segment.start + 3);
    app.input_ui.finalize_selection();
    app.insert_input_char('X');

    assert_eq!(app.input_ui.text(), "beforX after");
    assert!(app.input_ui.paste_segments().is_empty());

    let mut app = test_app();
    app.insert_input_text("before ");
    app.insert_pasted_input_text(&collapsible_paste());
    app.insert_input_text("XYZ");
    let segment = app.input_ui.paste_segments()[0].clone();
    app.input_ui.begin_selection(segment.end() + 1);
    app.input_ui.update_selection(segment.end() - 3);
    app.input_ui.finalize_selection();
    app.delete_input();

    assert_eq!(app.input_ui.text(), "before YZ");
    assert!(app.input_ui.paste_segments().is_empty());
}
