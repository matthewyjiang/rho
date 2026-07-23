use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pretty_assertions::assert_eq;

use super::{InlineChoice, InlineChoiceKeyOutcome, InlineChoiceOption};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn navigation_skips_unavailable_options() {
    let mut choice = InlineChoice::new(
        "choose",
        "details",
        vec![
            InlineChoiceOption::unavailable("first", '1', "first", "unavailable"),
            InlineChoiceOption::available("second", '2', "second", "available"),
            InlineChoiceOption::available("third", '3', "third", "available"),
        ],
    )
    .unwrap();

    assert_eq!(choice.selected_value(), "second");
    assert_eq!(
        choice.handle_key(key(KeyCode::Down)),
        InlineChoiceKeyOutcome::Handled
    );
    assert_eq!(choice.selected_value(), "third");
    assert_eq!(
        choice.handle_key(key(KeyCode::Up)),
        InlineChoiceKeyOutcome::Handled
    );
    assert_eq!(choice.selected_value(), "second");
}

#[test]
fn shortcut_selects_and_submits_option() {
    let mut choice = InlineChoice::new(
        "choose",
        "details",
        vec![
            InlineChoiceOption::available("compact", '1', "compact", "first"),
            InlineChoiceOption::available("direct", '2', "direct", "second"),
        ],
    )
    .unwrap();

    assert_eq!(
        choice.handle_key(key(KeyCode::Char('2'))),
        InlineChoiceKeyOutcome::Selected("direct".into())
    );
    assert_eq!(choice.selected_value(), "direct");
    assert_eq!(
        choice.handle_key(key(KeyCode::Esc)),
        InlineChoiceKeyOutcome::Cancelled
    );
}
