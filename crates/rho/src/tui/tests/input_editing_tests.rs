use super::*;

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
