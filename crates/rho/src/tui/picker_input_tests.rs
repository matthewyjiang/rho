use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use pretty_assertions::assert_eq;

use super::{
    super::{PickerAction, PickerItem, PickerKeyHints, UiPicker},
    apply_picker_key, PickerKeyEffect,
};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn item(label: &str) -> PickerItem {
    PickerItem {
        section: None,
        label: label.into(),
        detail: None,
        preview: None,
        badge: None,
        value: label.into(),
        selection_verb: None,
    }
}

// Covers: Tab must not complete the filter when tab_complete is disabled.
// Owner: tui picker key dispatch
#[test]
fn tab_is_ignored_when_tab_complete_disabled() {
    let mut picker = UiPicker::new("Config", vec![item("mode")], PickerAction::Config);
    picker.filter = "mo".into();

    let effect = apply_picker_key(
        &mut picker,
        key(KeyCode::Tab),
        None,
        /*space_confirms*/ true,
    );

    assert_eq!(effect, PickerKeyEffect::None);
    assert_eq!(picker.filter, "mo");
}

// Covers: Tab completes the selected row only when tab_complete is enabled.
// Owner: tui picker key dispatch
#[test]
fn tab_completes_filter_when_tab_complete_enabled() {
    let mut picker = UiPicker::new(
        "select model",
        vec![item("openai/gpt-5.5")],
        PickerAction::SelectModel,
    )
    .with_key_hints(PickerKeyHints {
        pin_toggle: false,
        tab_complete: true,
        row_delete: false,
    });
    picker.filter = "gpt".into();

    let effect = apply_picker_key(
        &mut picker,
        key(KeyCode::Tab),
        None,
        /*space_confirms*/ false,
    );

    assert_eq!(effect, PickerKeyEffect::Handled);
    assert_eq!(picker.filter, "openai/gpt-5.5");
}
