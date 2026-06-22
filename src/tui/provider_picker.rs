use super::{PickerAction, PickerItem, UiPicker};
use crate::model::catalog;

pub(super) fn provider_picker(verb: &str, action: PickerAction) -> UiPicker {
    let items = catalog::login_targets()
        .into_iter()
        .map(|target| PickerItem {
            label: target.provider.clone(),
            detail: Some(target.label),
            badge: None,
            value: target.provider,
        })
        .collect();

    UiPicker::new(
        format!("select provider to {verb}"),
        "type regex filter, tab complete, up/down select, enter confirm, esc cancel",
        items,
        action,
    )
}
