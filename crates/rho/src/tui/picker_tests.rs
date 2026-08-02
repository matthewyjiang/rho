use pretty_assertions::assert_eq;

use super::{PickerAction, PickerItem, PickerKeyHints, UiPicker};

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

// Covers: structured key hints must appear in footer parts so d-delete and
// Space confirm stay discoverable on both list and overlay chrome.
// Owner: tui picker policy
#[test]
fn action_footer_parts_include_structured_key_hints() {
    let picker = UiPicker::new(
        "resume session",
        vec![item("a")],
        PickerAction::ResumeSession,
    )
    .with_key_hints(PickerKeyHints {
        pin_toggle: false,
        tab_complete: true,
        row_delete: true,
    })
    .with_confirm_verb("resume");

    assert_eq!(
        picker.action_footer_parts(),
        vec![
            "Enter resume".to_string(),
            "Tab complete".to_string(),
            "d delete".to_string(),
            "Esc cancel".to_string(),
        ]
    );

    let config = UiPicker::new("Config", vec![item("mode")], PickerAction::Config);
    assert_eq!(
        config.action_footer_parts(),
        vec![
            "Enter change".to_string(),
            "Space confirm".to_string(),
            "Esc cancel".to_string(),
        ]
    );

    let model = UiPicker::new("select model", vec![item("m")], PickerAction::SelectModel)
        .with_key_hints(PickerKeyHints {
            pin_toggle: true,
            tab_complete: true,
            row_delete: false,
        });
    assert_eq!(
        model.action_footer_parts(),
        vec![
            "Enter select".to_string(),
            "Ctrl-P pin/unpin".to_string(),
            "Tab complete".to_string(),
            "Esc cancel".to_string(),
        ]
    );
}

// Covers: invalid regex filters must not look like ordinary empty matches.
// Owner: tui picker matching
#[test]
fn empty_match_message_distinguishes_invalid_regex() {
    let mut picker = UiPicker::new(
        "resume session",
        vec![item("alpha"), item("beta")],
        PickerAction::ResumeSession,
    );

    picker.filter = "nope".into();
    assert_eq!(picker.matching_indices(), Vec::<usize>::new());
    assert!(!picker.filter_is_invalid_regex());
    assert_eq!(picker.empty_match_message(), "no matches");

    picker.filter = "(".into();
    assert_eq!(picker.matching_indices(), Vec::<usize>::new());
    assert!(picker.filter_is_invalid_regex());
    assert_eq!(picker.empty_match_message(), "invalid regex");
}

// Covers: dismiss-only pickers keep a single Enter/Esc close hint.
// Owner: tui picker policy
#[test]
fn dismiss_picker_collapses_enter_and_esc() {
    let picker = UiPicker::new(
        "Keyboard shortcuts",
        vec![item("help")],
        PickerAction::Dismiss,
    )
    .with_confirm_verb("close");
    assert_eq!(
        picker.action_footer_parts(),
        vec!["Enter/Esc close".to_string()]
    );
}
