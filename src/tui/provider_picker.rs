use super::{PickerAction, PickerItem, UiPicker};
use crate::{
    credentials::{provider_has_stored_credentials, CredentialStore},
    model::catalog,
};

pub(super) fn provider_picker(verb: &str, action: PickerAction) -> UiPicker {
    provider_picker_for_targets(verb, action, catalog::login_targets())
}

pub(super) fn logout_provider_picker(store: &dyn CredentialStore) -> UiPicker {
    let targets = catalog::login_targets()
        .into_iter()
        .filter(|target| provider_has_stored_credentials(store, &target.provider).unwrap_or(false))
        .collect();
    provider_picker_for_targets("logout", PickerAction::LogoutProvider, targets)
}

fn provider_picker_for_targets(
    verb: &str,
    action: PickerAction,
    targets: Vec<catalog::LoginTarget>,
) -> UiPicker {
    let items = targets
        .into_iter()
        .map(|target| PickerItem {
            label: target.provider.clone(),
            detail: Some(target.label),
            preview: None,
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
