use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use pretty_assertions::assert_eq;

use super::{
    super::{PickerAction, PickerItem, PickerKeyHints, UiPicker},
    apply_picker_key, OverlayScrollTargets, PickerKeyEffect,
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

fn overlay_picker_with_detail() -> UiPicker {
    let mut first = item("alpha");
    first.detail = Some(
        (0..40)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let mut second = item("beta");
    second.detail = Some("short".into());
    UiPicker::new("agents", vec![first, second], PickerAction::ViewAgent)
        .with_layout(super::super::PickerLayout::Overlay)
}

fn detail_targets() -> Option<OverlayScrollTargets> {
    Some(OverlayScrollTargets {
        nav_rows: 10,
        detail: Some(super::super::picker_overlay::DetailViewport { width: 40, rows: 5 }),
    })
}

// Covers: both overlay panes must scroll from the keyboard; ←/→ moves the
// focus and Up/Down/PgUp/PgDn/Home/End act on the focused pane.
// Owner: tui picker key dispatch
#[test]
fn overlay_focus_routes_scrolling_to_both_panes() {
    let mut picker = overlay_picker_with_detail();
    let targets = detail_targets();

    // Nav focus is the default: Down moves the selection.
    apply_picker_key(&mut picker, key(KeyCode::Down), targets, false);
    assert_eq!(picker.selected, 1);
    apply_picker_key(&mut picker, key(KeyCode::Up), targets, false);
    assert_eq!(picker.selected, 0);

    // PgDn pages the nav list while nav is focused.
    apply_picker_key(&mut picker, key(KeyCode::PageDown), targets, false);
    assert_eq!(picker.selected, 1, "page down clamps to the last row");
    apply_picker_key(&mut picker, key(KeyCode::Home), targets, false);
    assert_eq!(picker.selected, 0);

    // Right focuses the detail pane; Down/PgDn/End scroll detail lines.
    apply_picker_key(&mut picker, key(KeyCode::Right), targets, false);
    assert!(picker.detail_pane_focused());
    apply_picker_key(&mut picker, key(KeyCode::Down), targets, false);
    assert_eq!(picker.detail_scroll, 1);
    apply_picker_key(&mut picker, key(KeyCode::PageDown), targets, false);
    assert_eq!(picker.detail_scroll, 6);
    apply_picker_key(&mut picker, key(KeyCode::End), targets, false);
    assert_eq!(picker.detail_scroll, 35);
    apply_picker_key(&mut picker, key(KeyCode::Up), targets, false);
    assert_eq!(picker.detail_scroll, 34);
    assert_eq!(picker.selected, 0, "detail scrolling leaves the selection");

    // Left returns focus to the nav list.
    apply_picker_key(&mut picker, key(KeyCode::Left), targets, false);
    assert!(!picker.detail_pane_focused());
    apply_picker_key(&mut picker, key(KeyCode::Down), targets, false);
    assert_eq!(picker.selected, 1);
}

// Covers: ←/→ must keep typing into the filter for pickers without a detail
// pane instead of becoming dead keys.
// Owner: tui picker key dispatch
#[test]
fn pane_focus_keys_are_inert_without_detail() {
    let mut picker = UiPicker::new("plain", vec![item("one")], PickerAction::Config);
    let effect = apply_picker_key(&mut picker, key(KeyCode::Right), None, false);
    assert_eq!(effect, PickerKeyEffect::None);
    assert!(!picker.detail_pane_focused());
}
