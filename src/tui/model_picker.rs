use super::{PickerAction, PickerBadge, PickerBadgeTone, PickerItem, TuiInfo, UiPicker};
use crate::model::catalog;

pub(super) fn model_picker(info: &TuiInfo, available_auths: &[String]) -> UiPicker {
    model_picker_for_current(
        "select model",
        &info.provider,
        &info.model,
        available_auths,
        PickerAction::SelectModel,
    )
}

pub(super) fn title_model_picker(
    current_provider: &str,
    current_model: &str,
    available_auths: &[String],
) -> UiPicker {
    model_picker_for_current(
        "select title model",
        current_provider,
        current_model,
        available_auths,
        PickerAction::SelectTitleModel,
    )
}

fn model_picker_for_current(
    title: &str,
    current_provider: &str,
    current_model: &str,
    available_auths: &[String],
    action: PickerAction,
) -> UiPicker {
    let current = format!("{current_provider}/{current_model}");
    let mut items =
        catalog::available_models_for_auths(available_auths)
            .into_iter()
            .map(|entry| {
                let value = format!("{}/{}", entry.provider, entry.model);
                let badge = (entry.provider == current_provider && entry.model == current_model)
                    .then(|| PickerBadge {
                        text: "(selected)".into(),
                        tone: PickerBadgeTone::Selected,
                    });
                PickerItem {
                    label: value.clone(),
                    detail: None,
                    preview: None,
                    badge,
                    value,
                }
            })
            .collect::<Vec<_>>();
    items.sort_by_key(|item| item.value != current);

    let mut picker = UiPicker::new(
        title,
        "type regex filter, tab complete, up/down select, enter confirm, esc cancel",
        items,
        action,
    );
    if let Some(index) = picker.items.iter().position(|item| item.value == current) {
        picker.selected = index;
    }
    picker
}
