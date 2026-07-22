use super::{PickerAction, PickerItem, UiPicker};
use rho_providers::{
    auth::login_dispatch::ProviderAuthentication, credentials::CredentialStore, model::catalog,
    provider,
};

pub(super) const ALL_REFRESHABLE_PROVIDERS: &str = "all";

pub(super) fn login_group_picker() -> UiPicker {
    // login_groups() is already alphabetical by display prompt.
    let items = catalog::login_groups()
        .into_iter()
        .map(|group| PickerItem {
            label: group.prompt,
            detail: None,
            preview: None,
            badge: None,
            value: group.id,
        })
        .collect();
    UiPicker::new(
        "select provider to login",
        "type regex filter, tab complete, up/down select, enter confirm, esc cancel",
        items,
        PickerAction::LoginGroup,
    )
}

pub(super) fn login_method_picker(group: catalog::LoginGroup) -> UiPicker {
    let title = format!("select {} login method", group.prompt);
    let items = group
        .methods
        .into_iter()
        .map(|method| PickerItem {
            label: method.prompt,
            detail: None,
            preview: None,
            badge: None,
            value: method.target.provider,
        })
        .collect();
    UiPicker::new(
        title,
        "type regex filter, tab complete, up/down select, enter confirm, esc cancel",
        items,
        PickerAction::LoginProvider,
    )
}

pub(super) fn refresh_model_list_picker(available_auths: &[String]) -> UiPicker {
    let mut items = vec![PickerItem {
        label: "All configured providers".into(),
        detail: Some("Refresh every available provider with model discovery support.".into()),
        preview: None,
        badge: None,
        value: ALL_REFRESHABLE_PROVIDERS.into(),
    }];
    let mut providers = provider::providers()
        .iter()
        .filter(|descriptor| descriptor.model_refresh.is_some())
        .filter(|descriptor| available_auths.iter().any(|auth| auth == descriptor.auth))
        .map(|descriptor| PickerItem {
            label: descriptor.display_name.into(),
            detail: Some(format!(
                "Refresh cached {} models with {}.",
                descriptor.display_name, descriptor.login_label
            )),
            preview: None,
            badge: None,
            value: descriptor.name.into(),
        })
        .collect::<Vec<_>>();
    providers.sort_by(|left, right| {
        left.label
            .to_ascii_lowercase()
            .cmp(&right.label.to_ascii_lowercase())
            .then_with(|| left.value.cmp(&right.value))
    });
    items.extend(providers);
    UiPicker::new(
        "Refresh model lists",
        "type regex filter, enter refresh, esc back",
        items,
        PickerAction::RefreshModelList,
    )
}

pub(super) fn logout_provider_picker(
    store: &dyn CredentialStore,
) -> rho_providers::credentials::CredentialResult<UiPicker> {
    let mut targets = Vec::new();
    for target in catalog::login_targets() {
        if ProviderAuthentication::has_stored_credentials(store, &target.provider)? {
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
    let mut items = targets
        .into_iter()
        .map(|target| PickerItem {
            label: target.provider.clone(),
            detail: Some(target.label),
            preview: None,
            badge: None,
            value: target.provider,
        })
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        left.label
            .to_ascii_lowercase()
            .cmp(&right.label.to_ascii_lowercase())
    });

    UiPicker::new(
        format!("select provider to {verb}"),
        "type regex filter, tab complete, up/down select, enter confirm, esc cancel",
        items,
        action,
    )
}

#[cfg(test)]
#[path = "provider_picker_tests.rs"]
mod tests;
