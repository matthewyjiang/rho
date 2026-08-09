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

// Covers: Tab must not fill the filter from the synthetic conversation-model row.
// Owner: tui picker filter completion
#[test]
fn complete_filter_skips_internal_agent_conversation_model_row() {
    let mut picker = UiPicker::new(
        "select model for explorer",
        vec![
            PickerItem {
                section: None,
                label: "Use conversation model".into(),
                detail: None,
                preview: None,
                badge: None,
                value: super::super::model_picker::USE_CONVERSATION_MODEL.into(),
                selection_verb: None,
            },
            item("openai/gpt-5.5"),
        ],
        PickerAction::SelectInternalAgentModel,
    );
    picker.selected = 0;
    picker.filter = "gpt".into();
    picker.complete_filter();
    assert_eq!(picker.filter, "gpt");

    picker.selected = 1;
    picker.complete_filter();
    assert_eq!(picker.filter, "openai/gpt-5.5");
}

// Covers: fuzzy pickers must find items by any visible short field, not only
// the internal value; a user typing a label or badge word must get a match.
// Owner: tui picker filter policy
#[test]
fn fuzzy_filter_matches_label_section_and_badge() {
    let mut labeled = item("anthropic/claude-fable-5");
    labeled.value = "claude-fable-5".into();
    let mut sectioned = item("gpt-oss:120b");
    sectioned.section = Some("OLLAMA CLOUD".into());
    let mut badged = item("kimi-k3");
    badged.badge = Some(super::PickerBadge {
        text: "pinned".into(),
        tone: super::PickerBadgeTone::Favorite,
    });
    let items = vec![labeled, sectioned, badged];

    let matches_for = |filter: &str| super::fuzzy_picker_matching_indices(&items, filter);
    assert_eq!(matches_for("anthropic"), vec![0], "label word must match");
    assert_eq!(matches_for("ollama"), vec![1], "section word must match");
    assert_eq!(matches_for("pinned"), vec![2], "badge word must match");
    assert_eq!(matches_for("zzzz"), Vec::<usize>::new());
}

// Covers: typing a row's own text must match that row. The bonus-seeking walk
// is greedy and can strand the rest of the needle on a haystack that repeats
// characters, which read to the user as "no matches" for the exact label.
// Owner: tui picker filter policy
#[test]
fn fuzzy_filter_matches_a_row_by_its_full_text() {
    let items = vec![
        item("claude-code/default"),
        item("claude-code/opus"),
        item("anthropic/claude-fable-5"),
    ];

    let matches_for = |filter: &str| super::fuzzy_picker_matching_indices(&items, filter);
    assert_eq!(matches_for("claude-code/opus"), vec![1]);
    assert_eq!(matches_for("claude-code/default"), vec![0]);
    assert_eq!(matches_for("opus"), vec![1]);
    assert_eq!(matches_for("claude-code"), vec![0, 1]);
}

// Covers: the wheel scrolls the nav viewport without moving the selection,
// clamps to the overflow range, and keyboard navigation afterwards brings the
// window back to the selection with minimal movement.
// Owner: tui picker nav scroll policy
#[test]
fn nav_wheel_scroll_is_independent_of_selection() {
    let items = (0..20).map(|i| item(&format!("item-{i:02}"))).collect();
    let mut picker = UiPicker::new("list", items, PickerAction::ViewAgent);
    let viewport = 5;
    assert_eq!(picker.nav_window_start(viewport), 0);

    picker.scroll_nav_by(6, viewport);
    assert_eq!(picker.nav_window_start(viewport), 6);
    assert_eq!(picker.selected, 0, "wheel must not move the selection");

    picker.scroll_nav_by(100, viewport);
    assert_eq!(picker.nav_window_start(viewport), 15, "clamps to max start");

    picker.select_next();
    assert_eq!(picker.selected, 1);
    assert_eq!(
        picker.nav_window_start(viewport),
        1,
        "keyboard navigation snaps the window back to the selection"
    );
}

// Covers: clicking a nav row selects its item, ignores section headers, and
// never shifts the viewport.
// Owner: tui picker mouse selection
#[test]
fn click_selects_nav_row_and_keeps_window() {
    let items = (0..10)
        .map(|i| {
            let mut it = item(&format!("row-{i}"));
            it.section = Some("GROUP".into());
            it
        })
        .collect();
    let mut picker = UiPicker::new("list", items, PickerAction::ViewAgent);
    let viewport = 5;
    picker.scroll_nav_by(3, viewport);

    assert!(
        !picker.select_nav_row(0, viewport),
        "section header rows are not selectable"
    );
    assert!(picker.select_nav_row(4, viewport));
    assert_eq!(picker.selected, 3, "row space offsets by the header row");
    assert_eq!(
        picker.nav_window_start(viewport),
        3,
        "click must not shift the window"
    );
}

// Covers: after deleting a nearby row the reopened picker must land on the
// following match, not jump back to the top of the list.
// Owner: pure unit (picker cursor restore)
#[test]
fn restore_cursor_keeps_nearby_match_after_removal() {
    let items = (0..10)
        .map(|i| item(&format!("session-{i:02}")))
        .collect::<Vec<_>>();
    let mut picker = UiPicker::new("sessions", items, PickerAction::ManageSessions);
    picker.select_by_offset(6);
    assert_eq!(picker.selected_item().unwrap().value, "session-06");
    let cursor = picker.cursor();
    assert_eq!(cursor.match_index, 6);

    let remaining = (0..10)
        .filter(|i| *i != 6)
        .map(|i| item(&format!("session-{i:02}")))
        .collect::<Vec<_>>();
    let mut reopened = UiPicker::new("sessions", remaining, PickerAction::ManageSessions);
    reopened.restore_cursor(&cursor);
    assert_eq!(
        reopened.selected_item().unwrap().value,
        "session-07",
        "cursor should advance to the next surviving neighbor"
    );
}

// Covers: absolute nav scrollbar jumps must move the viewport without changing
// selection, matching history-scrollbar track clicks.
// Owner: pure unit (picker nav scroll)
#[test]
fn scroll_nav_to_jumps_viewport_without_selection() {
    let items = (0..20).map(|i| item(&format!("item-{i:02}"))).collect();
    let mut picker = UiPicker::new("list", items, PickerAction::ViewAgent);
    let viewport = 5;
    picker.scroll_nav_to(9, viewport);
    assert_eq!(picker.nav_window_start(viewport), 9);
    assert_eq!(picker.selected, 0);
    picker.scroll_nav_to(100, viewport);
    assert_eq!(picker.nav_window_start(viewport), 15);
}
