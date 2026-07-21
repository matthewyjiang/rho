use super::*;

#[test]
fn valid_slash_commands_are_added_to_input_history() {
    let mut app = test_app();
    app.input = "/info  ".into();
    app.input_cursor = app.input_char_len();

    let invocation = app.parse_input_command().unwrap().unwrap();

    assert_eq!(invocation.id, CommandId::Info);
    assert_eq!(app.input_history, ["/info"]);
    app.input.clear();
    app.input_cursor = 0;
    app.recall_input_history_or_move_cursor(HistoryDirection::Previous, 80);
    assert_eq!(app.input, "/info");
}

#[test]
fn left_and_right_arrows_treat_collapsed_paste_as_one_character() {
    let mut app = test_app();
    app.insert_input_text("a");
    app.insert_pasted_input_text("alpha\nbeta\ngamma");
    let segment = app.paste_segments[0].clone();

    app.move_input_cursor_left();
    assert_eq!(app.input_cursor, segment.start);

    app.move_input_cursor_right();
    assert_eq!(app.input_cursor, segment.end());

    app.move_input_cursor_left();
    app.move_input_cursor_left();
    assert_eq!(app.input_cursor, segment.start - 1);
}

#[test]
fn focused_collapsed_paste_highlights_the_whole_marker() {
    let mut app = test_app();
    app.insert_pasted_input_text("alpha\nbeta\ngamma");
    app.move_input_cursor_left();

    let highlighted = app
        .composer_lines(10)
        .iter()
        .flat_map(|line| &line.spans)
        .filter(|span| span.style.add_modifier.contains(Modifier::REVERSED))
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert_eq!(highlighted, "[ pasted: 3 lines ]");
}

#[test]
fn vertical_cursor_movement_focuses_a_collapsed_paste_item() {
    let mut app = test_app();
    app.insert_input_text("first line\n");
    app.insert_pasted_input_text("alpha\nbeta\ngamma");
    let segment = app.paste_segments[0].clone();
    app.input_cursor = 5;

    app.recall_input_history_or_move_cursor(HistoryDirection::Next, 80);

    assert_eq!(app.input_cursor, segment.start);
}

#[test]
fn backspace_removes_collapsed_paste_as_one_item() {
    let mut app = test_app();
    app.insert_pasted_input_text("alpha\nbeta\ngamma");

    app.backspace_input();

    assert_eq!(app.input, "");
    assert_eq!(app.input_cursor, 0);
    assert!(app.paste_segments.is_empty());
}

#[test]
fn delete_removes_collapsed_paste_as_one_item() {
    let mut app = test_app();
    app.insert_input_text("before ");
    app.insert_pasted_input_text("alpha\nbeta\ngamma");
    app.insert_input_text(" after");
    app.input_cursor = "before ".chars().count();

    app.delete_input();

    assert_eq!(app.input, "before  after");
    assert_eq!(app.input_cursor, "before ".chars().count());
    assert!(app.paste_segments.is_empty());
}

#[test]
fn editing_from_inside_collapsed_paste_removes_the_whole_item() {
    let mut app = test_app();
    app.insert_pasted_input_text("alpha\nbeta\ngamma");
    app.input_cursor = 5;

    app.backspace_input();

    assert_eq!(app.input, "");
    assert_eq!(app.input_cursor, 0);
    assert!(app.paste_segments.is_empty());
}
