use super::*;

#[test]
fn number_input_accepts_only_ascii_digits() {
    let mut input = ConfigNumberInput::new(ConfigNumberKey::MaxOutputBytes, 42);

    input.insert_text("a1-２3");

    assert_eq!(input.value, "4213");
    assert_eq!(input.cursor, 4);
}

#[test]
fn text_input_strips_line_breaks_and_edits_at_character_cursor() {
    let mut input = ConfigTextInput::new(ConfigTextKey::Exa, Some("aé".into()));
    input.cursor = 1;

    input.insert_text("x\ny\r");
    input.delete();

    assert_eq!(input.value, "axy");
    assert_eq!(input.cursor, 3);
}

#[test]
fn editor_cursor_navigation_is_unicode_safe() {
    let mut input = ConfigTextInput::new(ConfigTextKey::Brave, Some("aéz".into()));

    input.move_cursor_left();
    input.backspace();
    input.move_cursor_home();
    input.move_cursor_right();
    input.insert_char('x');
    input.move_cursor_end();

    assert_eq!(input.value, "axz");
    assert_eq!(input.cursor, 3);
}
