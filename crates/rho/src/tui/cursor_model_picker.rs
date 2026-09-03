//! Agent-editor picker for cached Cursor Agent models.

use crate::cursor_runtime::models::CursorModel;

use super::{
    picker::OverlayChrome, PickerBadge, PickerBadgeTone, PickerItem, PickerLayout, UiPicker,
};

pub(super) const CURSOR_MODEL_OTHER: &str = "agent_cursor_model:other";

pub(super) fn cursor_model_picker(models: &[CursorModel], current: &str) -> UiPicker {
    let mut items = models.iter().map(model_item).collect::<Vec<_>>();
    items.sort_by(|left, right| {
        left.section
            .cmp(&right.section)
            .then_with(|| left.value.cmp(&right.value))
    });
    items.push(other_item());
    let selected = items
        .iter()
        .position(|item| item.value == current)
        .unwrap_or(0);
    let mut picker = UiPicker::edit_agent("Select Cursor model", items)
        .with_layout(PickerLayout::Overlay)
        .with_fuzzy_filter()
        .with_confirm_verb("select")
        .with_overlay_chrome(OverlayChrome {
            nav_label: " CURSOR MODELS".into(),
            detail_label: Some(" DETAILS".into()),
            nav_keys_hint: "↑↓ models".into(),
        });
    picker.selected = selected;
    picker
}

fn model_item(model: &CursorModel) -> PickerItem {
    PickerItem {
        section: Some(model.display_family()),
        label: model.id.clone(),
        detail: Some(model.display_name.clone()),
        preview: None,
        badge: model_badge(model),
        value: model.id.clone(),
        selection_verb: None,
        allow_filter_completion: true,
    }
}

fn other_item() -> PickerItem {
    PickerItem {
        section: None,
        label: "Other… (type a model id)".into(),
        detail: Some(
            "Type a Cursor model id or a bracket override such as name[effort=high,fast=false]."
                .into(),
        ),
        preview: None,
        badge: None,
        value: CURSOR_MODEL_OTHER.into(),
        selection_verb: None,
        allow_filter_completion: false,
    }
}

fn model_badge(model: &CursorModel) -> Option<PickerBadge> {
    let mut parts = Vec::new();
    if model.is_default {
        parts.push("default");
    }
    if model.is_current {
        parts.push("current");
    }
    if !model.zdr {
        parts.push("no ZDR");
    }
    if parts.is_empty() {
        return None;
    }
    Some(PickerBadge {
        text: parts.join(", "),
        tone: if model.zdr {
            PickerBadgeTone::Selected
        } else {
            PickerBadgeTone::Warning
        },
    })
}

#[cfg(test)]
#[path = "cursor_model_picker_tests.rs"]
mod tests;
