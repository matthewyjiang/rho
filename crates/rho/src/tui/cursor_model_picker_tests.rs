use pretty_assertions::assert_eq;

use crate::cursor_runtime::models::CursorModel;

use super::*;

fn model(
    id: &str,
    display_name: &str,
    is_default: bool,
    is_current: bool,
    zdr: bool,
) -> CursorModel {
    CursorModel {
        id: id.into(),
        display_name: display_name.into(),
        is_default,
        is_current,
        zdr,
    }
}

// Covers: Cursor editor rows group by display-name family, keep the raw id as
// the value, and expose flags as badges plus an Other sentinel.
// Owner: tui agent editor
#[test]
fn cursor_model_picker_groups_by_family_and_keeps_ids() {
    let picker = cursor_model_picker(
        &[
            model(
                "claude-opus-5-thinking-high-fast",
                "Claude Opus 5 1M Thinking Fast",
                false,
                false,
                true,
            ),
            model("auto", "Auto", true, false, true),
            model(
                "claude-fable-5-thinking-high",
                "Claude Fable 5 1M Thinking",
                false,
                false,
                false,
            ),
            model("composer-2.5", "Composer 2.5", false, true, true),
        ],
        "composer-2.5",
    );

    let rows = picker
        .items
        .iter()
        .map(|item| {
            (
                item.section.as_deref(),
                item.value.as_str(),
                item.badge.as_ref().map(|badge| badge.text.as_str()),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        vec![
            (Some("Auto"), "auto", Some("default")),
            (
                Some("Claude Fable 5"),
                "claude-fable-5-thinking-high",
                Some("no ZDR")
            ),
            (
                Some("Claude Opus 5"),
                "claude-opus-5-thinking-high-fast",
                None
            ),
            (Some("Composer 2.5"), "composer-2.5", Some("current")),
            (None, CURSOR_MODEL_OTHER, None),
        ]
    );
    assert_eq!(picker.items[picker.selected].value, "composer-2.5");
    assert!(picker.force_fuzzy_filter);
}
