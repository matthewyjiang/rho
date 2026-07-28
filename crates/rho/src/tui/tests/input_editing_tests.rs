use super::*;

#[test]
fn idle_composer_shows_prompt_marker_and_placeholder() {
    let app = test_app();

    let lines = app.composer_lines(80);
    let text = lines[0]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(text.starts_with("> "));
    assert_ne!(text, "> ");
    assert!(lines[0].spans.iter().all(|span| span.style == Theme::dim()));
    assert_eq!(
        app.composer_cursor_position(80),
        ratatui::layout::Position { x: 2, y: 0 }
    );
}

#[test]
fn composer_placeholder_disappears_when_text_is_entered() {
    let mut app = test_app();
    app.insert_input_text("hello");

    let lines = app.composer_lines(80);
    let text = lines[0]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert_eq!(text, "> hello");
    assert_eq!(lines[0].spans[0].style, Theme::dim());
    assert_eq!(lines[0].spans[1].style, ratatui::style::Style::default());
}

#[test]
fn composer_wraps_and_moves_cursor_with_prompt_indent() {
    let mut app = test_app();
    app.insert_input_text("123456789");

    let rendered = app
        .composer_lines(10)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(rendered, ["> 12345678", "  9"]);
    assert_eq!(
        app.composer_cursor_position(10),
        ratatui::layout::Position { x: 3, y: 1 }
    );
}

#[test]
fn composer_moves_cursor_to_continuation_at_exact_wrap_boundary() {
    let mut app = test_app();
    app.insert_input_text("12345678");

    let rendered = app
        .composer_lines(10)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(rendered, ["> 12345678", "  "]);
    assert_eq!(
        app.composer_cursor_position(10),
        ratatui::layout::Position { x: 2, y: 1 }
    );
}

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
    app.insert_pasted_input_text("alpha\nbeta\ngamma");
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
    let segment = app.input_ui.paste_segments()[0].clone();
    app.input_ui.set_cursor(5);

    app.recall_input_history_or_move_cursor(HistoryDirection::Next, 80);

    assert_eq!(app.input_ui.cursor(), segment.start);
}

#[test]
fn backspace_removes_collapsed_paste_as_one_item() {
    let mut app = test_app();
    app.insert_pasted_input_text("alpha\nbeta\ngamma");

    app.backspace_input();

    assert_eq!(app.input_ui.text(), "");
    assert_eq!(app.input_ui.cursor(), 0);
    assert!(app.input_ui.paste_segments().is_empty());
}

#[test]
fn delete_removes_collapsed_paste_as_one_item() {
    let mut app = test_app();
    app.insert_input_text("before ");
    app.insert_pasted_input_text("alpha\nbeta\ngamma");
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
    app.insert_pasted_input_text("alpha\nbeta\ngamma");
    app.input_ui.set_cursor(5);

    app.backspace_input();

    assert_eq!(app.input_ui.text(), "");
    assert_eq!(app.input_ui.cursor(), 0);
    assert!(app.input_ui.paste_segments().is_empty());
}
