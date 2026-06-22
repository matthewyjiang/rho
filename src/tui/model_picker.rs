use super::{PickerAction, PickerBadge, PickerBadgeTone, PickerItem, TuiInfo, UiPicker};
use crate::model::catalog;

pub(super) fn model_picker(info: &TuiInfo, available_auths: &[String]) -> UiPicker {
    let current = format!("{}/{}", info.provider, info.model);
    let mut items = catalog::available_models_for_auths(available_auths)
        .into_iter()
        .map(|entry| {
            let value = format!("{}/{}", entry.provider, entry.model);
            let badge = (entry.provider == info.provider && entry.model == info.model).then(|| {
                PickerBadge {
                    text: "(selected)".into(),
                    tone: PickerBadgeTone::Selected,
                }
            });
            PickerItem {
                label: value.clone(),
                detail: None,
                badge,
                value,
            }
        })
        .collect::<Vec<_>>();
    items.sort_by_key(|item| item.value != current);

    let mut picker = UiPicker::new(
        "select model",
        "type regex filter, tab complete, up/down select, enter confirm, esc cancel",
        items,
        PickerAction::SelectModel,
    );
    if let Some(index) = picker.items.iter().position(|item| item.value == current) {
        picker.selected = index;
    }
    picker
}
