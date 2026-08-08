use super::{
    theme::{list_themes, theme_display_name, Theme, ThemeEntry, THEME_TERMINAL_ID},
    PickerAction, PickerBadge, PickerBadgeTone, PickerItem, PickerLayout, UiPicker,
};

pub(super) fn theme_picker(current_id: &str) -> UiPicker {
    let current = {
        let trimmed = current_id.trim();
        if trimmed.is_empty() {
            Theme::committed_id()
        } else {
            trimmed.to_string()
        }
    };

    let entries = list_themes();
    // Retain resolved schemes so preview/apply do not re-read custom files.
    Theme::set_picker_catalog(&entries);

    let items = entries
        .into_iter()
        .map(|entry| {
            let badge = if entry.id() == current {
                Some(PickerBadge {
                    text: "current".into(),
                    tone: PickerBadgeTone::Selected,
                })
            } else {
                Some(PickerBadge {
                    text: entry.source_label().into(),
                    tone: match &entry {
                        ThemeEntry::Terminal => PickerBadgeTone::Healthy,
                        ThemeEntry::Fixed(_) if entry.is_custom() => PickerBadgeTone::Editable,
                        ThemeEntry::Fixed(_) => PickerBadgeTone::Internal,
                    },
                })
            };
            PickerItem {
                section: None,
                label: entry.name().to_string(),
                detail: Some(entry.detail()),
                preview: None,
                badge,
                value: entry.id().to_string(),
                selection_verb: Some("apply"),
            }
        })
        .collect::<Vec<_>>();

    let mut picker = UiPicker::new("theme", items, PickerAction::SelectTheme)
        .with_layout(PickerLayout::Overlay)
        .with_confirm_verb("apply");
    if let Some(index) = picker.items.iter().position(|item| item.value == current) {
        picker.selected = index;
    } else if let Some(index) = picker
        .items
        .iter()
        .position(|item| item.value == THEME_TERMINAL_ID)
    {
        picker.selected = index;
    }
    picker
}

/// Shared display name for status / config badge.
pub(super) fn label_for_theme_id(id: &str) -> String {
    theme_display_name(id)
}
