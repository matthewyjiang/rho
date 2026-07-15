use super::{PickerAction, PickerItem, UiPicker};
use crate::{
    credentials::{provider_has_stored_credentials, CredentialStore},
    model::catalog,
};

pub(super) fn provider_picker(verb: &str, action: PickerAction) -> UiPicker {
    provider_picker_for_targets(verb, action, catalog::login_targets())
}

pub(super) fn logout_provider_picker(
    store: &dyn CredentialStore,
) -> crate::credentials::CredentialResult<UiPicker> {
    let mut targets = Vec::new();
    for target in catalog::login_targets() {
        if provider_has_stored_credentials(store, &target.provider)? {
            targets.push(target);
        }
    }
    Ok(provider_picker_for_targets(
        "logout",
        PickerAction::LogoutProvider,
        targets,
    ))
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
