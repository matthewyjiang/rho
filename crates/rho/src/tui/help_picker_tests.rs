use crate::keybindings::Keybindings;
use ratatui::{layout::Rect, text::Line};

use super::*;
use crate::tui::picker_overlay::render_picker_overlay;

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

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
        "ctrl+end",
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
    let badge = reset.badge.as_ref().unwrap();
    assert_eq!(badge.text, "Reset chat");
    assert_eq!(badge.tone, PickerBadgeTone::Selected);
    assert_eq!(
        reset.detail.as_deref(),
        Some(
            "Clear conversation history so the next message starts a new session. Unavailable while a model turn is running."
        )
    );
}

#[test]
fn help_picker_shows_descriptions_on_unselected_rows() {
    let picker = help_picker(&Keybindings::default());
    let frame = render_picker_overlay(&picker, Rect::new(0, 0, 100, 28));
    let rendered = frame.lines.iter().map(line_text).collect::<Vec<_>>();
    let newline_row = rendered
        .iter()
        .find(|line| line.contains("ctrl+j"))
        .unwrap();
    let truncated_rows = rendered
        .iter()
        .filter_map(|line| line.split_once(" │ ").map(|(nav, _)| nav))
        .filter(|nav| nav.contains('…'))
        .collect::<Vec<_>>();

    assert!(newline_row.contains("New line"), "{newline_row}");
    assert!(
        truncated_rows.is_empty(),
        "truncated help rows: {truncated_rows:?}"
    );
}
