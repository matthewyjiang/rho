use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use pretty_assertions::assert_eq;
use ratatui::layout::Rect;

use super::{
    super::{
        picker_overlay_layout::picker_overlay_layout, tests::test_app, ComposerMode, PickerAction,
        PickerItem, PickerKeyHints, UiPicker,
    },
    apply_picker_key, OverlayScrollTargets, OverlayScrollbarDrag, PickerKeyEffect,
    PickerMouseEvent,
};

fn keys() -> crate::keybindings::Keybindings {
    crate::keybindings::Keybindings::default()
}

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

// Covers: shifted letters must reach the filter; crossterm reports uppercase
// with SHIFT set, and generated titles are Title Case.
// Owner: tui picker key dispatch
#[test]
fn filter_accepts_shift_modified_characters() {
    let mut picker = UiPicker::new("attach", vec![item("Quoted Title")], PickerAction::Config);
    let mut shift = key(KeyCode::Char('Q'));
    shift.modifiers = KeyModifiers::SHIFT;

    let effect = apply_picker_key(
        &mut picker,
        shift,
        None,
        /*space_confirms*/ false,
        &keys(),
    );

    assert_eq!(effect, PickerKeyEffect::Handled);
    assert_eq!(picker.filter, "Q");

    let mut ctrl = key(KeyCode::Char('q'));
    ctrl.modifiers = KeyModifiers::CONTROL;
    let effect = apply_picker_key(
        &mut picker,
        ctrl,
        None,
        /*space_confirms*/ false,
        &keys(),
    );
    assert_eq!(effect, PickerKeyEffect::None);
    assert_eq!(picker.filter, "Q");
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
        &keys(),
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
        pin_toggle: None,
        scope_toggle: None,
        tab_complete: true,
        row_delete: false,
    });
    picker.filter = "gpt".into();

    let effect = apply_picker_key(
        &mut picker,
        key(KeyCode::Tab),
        None,
        /*space_confirms*/ false,
        &keys(),
    );

    assert_eq!(effect, PickerKeyEffect::Handled);
    assert_eq!(picker.filter, "openai/gpt-5.5");
}

// Covers: model pickers must treat Ctrl-O as a scope toggle, not a dead key
// or a filter character.
// Owner: tui picker key dispatch
#[test]
fn ctrl_o_toggles_model_scope_when_enabled() {
    let mut picker = UiPicker::new(
        "select model",
        vec![item("openai/gpt-5.5")],
        PickerAction::SelectModel,
    )
    .with_key_hints(PickerKeyHints {
        pin_toggle: Some("Ctrl+P".into()),
        scope_toggle: Some("Ctrl+O".into()),
        tab_complete: true,
        row_delete: false,
    });
    let mut key = key(KeyCode::Char('o'));
    key.modifiers = KeyModifiers::CONTROL;

    assert_eq!(
        apply_picker_key(
            &mut picker,
            key,
            None,
            /*space_confirms*/ false,
            &keys()
        ),
        PickerKeyEffect::ToggleModelScope
    );

    picker.key_hints.scope_toggle = None;
    assert_eq!(
        apply_picker_key(
            &mut picker,
            key,
            None,
            /*space_confirms*/ false,
            &keys()
        ),
        PickerKeyEffect::None
    );
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
        detail: Some(super::super::picker_overlay_layout::DetailViewport { width: 40, rows: 5 }),
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
    apply_picker_key(&mut picker, key(KeyCode::Down), targets, false, &keys());
    assert_eq!(picker.selected, 1);
    apply_picker_key(&mut picker, key(KeyCode::Up), targets, false, &keys());
    assert_eq!(picker.selected, 0);

    // PgDn pages the nav list while nav is focused.
    apply_picker_key(&mut picker, key(KeyCode::PageDown), targets, false, &keys());
    assert_eq!(picker.selected, 1, "page down clamps to the last row");
    apply_picker_key(&mut picker, key(KeyCode::Home), targets, false, &keys());
    assert_eq!(picker.selected, 0);

    // Right focuses the detail pane; Down/PgDn/End scroll detail lines.
    apply_picker_key(&mut picker, key(KeyCode::Right), targets, false, &keys());
    assert!(picker.detail_pane_focused());
    apply_picker_key(&mut picker, key(KeyCode::Down), targets, false, &keys());
    assert_eq!(picker.detail_scroll, 1);
    apply_picker_key(&mut picker, key(KeyCode::PageDown), targets, false, &keys());
    assert_eq!(picker.detail_scroll, 6);
    apply_picker_key(&mut picker, key(KeyCode::End), targets, false, &keys());
    assert_eq!(picker.detail_scroll, 35);
    apply_picker_key(&mut picker, key(KeyCode::Up), targets, false, &keys());
    assert_eq!(picker.detail_scroll, 34);
    assert_eq!(picker.selected, 0, "detail scrolling leaves the selection");

    // Left returns focus to the nav list.
    apply_picker_key(&mut picker, key(KeyCode::Left), targets, false, &keys());
    assert!(!picker.detail_pane_focused());
    apply_picker_key(&mut picker, key(KeyCode::Down), targets, false, &keys());
    assert_eq!(picker.selected, 1);
}

// Covers: ←/→ must keep typing into the filter for pickers without a detail
// pane instead of becoming dead keys.
// Owner: tui picker key dispatch
#[test]
fn pane_focus_keys_are_inert_without_detail() {
    let mut picker = UiPicker::new("plain", vec![item("one")], PickerAction::Config);
    let effect = apply_picker_key(&mut picker, key(KeyCode::Right), None, false, &keys());
    assert_eq!(effect, PickerKeyEffect::None);
    assert!(!picker.detail_pane_focused());
}

// Covers: the detail scrollbar's painted gutter must accept track clicks and
// drag events instead of treating them as inert detail-pane clicks.
// Owner: tui picker mouse routing
#[test]
fn detail_scrollbar_click_and_drag_scroll_the_right_pane() {
    let width = 120;
    let height = 40;
    let picker = overlay_picker_with_detail();
    let layout = picker_overlay_layout(Rect::new(0, 0, width, height), picker.overlay_sizing());
    let detail = layout.detail_body_rect().expect("detail pane");
    let scrollbar_column = detail.x + detail.width - 1;
    let bottom_row = detail.y + detail.height - 1;
    let top_row = detail.y;
    let mut app = test_app();
    app.input_ui.set_composer(ComposerMode::Picker(picker));

    assert!(app.route_picker_mouse(
        PickerMouseEvent::Click,
        scrollbar_column,
        bottom_row,
        width,
        height,
    ));
    let ComposerMode::Picker(picker) = app.input_ui.composer() else {
        panic!("picker closed after detail scrollbar click");
    };
    assert!(picker.detail_scroll > 0, "track click must jump downward");
    assert!(picker.detail_pane_focused());
    assert!(matches!(
        picker.overlay_scrollbar_drag(),
        Some(OverlayScrollbarDrag::Detail(_))
    ));

    assert!(app.route_picker_mouse(
        PickerMouseEvent::Drag,
        scrollbar_column,
        top_row,
        width,
        height,
    ));
    let ComposerMode::Picker(picker) = app.input_ui.composer() else {
        panic!("picker closed during detail scrollbar drag");
    };
    assert_eq!(
        picker.detail_scroll, 0,
        "dragging to the top must reach top"
    );

    assert!(app.route_picker_mouse(
        PickerMouseEvent::Release,
        scrollbar_column,
        top_row,
        width,
        height,
    ));
    let ComposerMode::Picker(picker) = app.input_ui.composer() else {
        panic!("picker closed after detail scrollbar release");
    };
    assert_eq!(picker.overlay_scrollbar_drag(), None);
}
