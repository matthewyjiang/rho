use crate::keybindings::Keybindings;

use super::*;

#[test]
fn help_picker_lists_core_and_configurable_shortcuts() {
    let keybindings = Keybindings {
        reset_conversation: "ctrl+shift+r".parse().unwrap(),
        ..Keybindings::default()
    };
    let picker = help_picker(&keybindings);

    assert_eq!(picker.action, PickerAction::Dismiss);
    assert_eq!(picker.layout, PickerLayout::Overlay);
    assert!(picker.is_overlay());
    let chrome = picker.overlay_chrome.as_ref().unwrap();
    assert_eq!(chrome.nav_label, " KEYS");
    assert_eq!(chrome.detail_label.as_deref(), Some(" DETAILS"));
    assert_eq!(chrome.nav_keys_hint, "↑↓ keys");

    let labels = picker
        .items
        .iter()
        .map(|item| item.label.as_str())
        .collect::<Vec<_>>();
    for expected in [
        "/",
        "@",
        "!",
        "!!",
        "enter",
        "esc",
        "shift+tab",
        "ctrl+c",
        "ctrl+j",
        "ctrl+shift+r",
        "ctrl+g",
        "ctrl+o",
        "ctrl+v",
        "alt+up",
        "alt+q",
    ] {
        assert!(
            labels.contains(&expected),
            "missing help entry {expected} in {labels:?}"
        );
    }
    assert!(
        labels.iter().all(|label| !label.starts_with("/help")),
        "help overlay should not list slash commands: {labels:?}"
    );

    let reset = picker
        .items
        .iter()
        .find(|item| item.label == "ctrl+shift+r")
        .unwrap();
    let detail = reset.detail.as_deref().unwrap();
    assert!(detail.contains("Reset conversation"));
    assert!(detail.contains("new session"));
}
