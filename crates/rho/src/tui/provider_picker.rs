use super::{
    sort_items_by_ascii_label, PickerAction, PickerBadge, PickerBadgeTone, PickerItem, UiPicker,
};
use rho_providers::{
    auth::login_dispatch::ProviderAuthentication,
    credentials::{CredentialError, CredentialStore},
    model::catalog,
    provider,
};

pub(super) const ALL_REFRESHABLE_PROVIDERS: &str = "all";

/// Next step after choosing a top-level `/login` provider group.
pub(super) enum LoginGroupNext {
    /// One method only: start that provider login directly.
    Provider(String),
    /// Multiple methods: open the method picker (for example Anthropic API vs Claude Code).
    MethodPicker(Box<UiPicker>),
}

pub(super) fn login_group_picker() -> UiPicker {
    let mut items = catalog::login_groups()
        .into_iter()
        .map(|group| PickerItem {
            section: None,
            label: group.prompt,
            detail: None,
            preview: None,
            badge: None,
            value: group.id,
            selection_verb: None,
        })
        .collect::<Vec<_>>();
    items.extend(super::custom_provider_login::login_group_items());
    sort_items_by_ascii_label(&mut items);
    UiPicker::new("Select provider to login", items, PickerAction::LoginGroup).with_key_hints(
        super::PickerKeyHints {
            tab_complete: true,
            row_delete: false,
            ..Default::default()
        },
    )
}

/// Resolve whether a login group continues directly or opens a method picker.
///
/// Built from the same item list the picker would show, so a group with one
/// method short-circuits only when that really is the only way in.
pub(super) fn login_group_next(group: catalog::LoginGroup) -> LoginGroupNext {
    let picker = login_method_picker(group);
    match picker.items.as_slice() {
        [only] => LoginGroupNext::Provider(only.value.clone()),
        _ => LoginGroupNext::MethodPicker(Box::new(picker)),
    }
}

/// Methods for one login group: catalog providers plus any external runtime
/// offered under the same group.
pub(super) fn login_method_picker(group: catalog::LoginGroup) -> UiPicker {
    let title = format!("Select {} login method", group.prompt);
    let group_id = group.id.clone();
    let mut items = group
        .methods
        .into_iter()
        .map(|method| PickerItem {
            section: None,
            label: method.prompt,
            detail: None,
            preview: None,
            badge: None,
            value: method.target.auth,
            selection_verb: None,
        })
        .collect::<Vec<_>>();
    items.extend(
        super::claude_login::EXTERNAL_LOGIN_METHODS
            .iter()
            .filter(|method| method.group_id == group_id)
            .map(|method| PickerItem {
                section: None,
                label: method.label.into(),
                detail: Some(method.detail.into()),
                preview: None,
                badge: None,
                value: method.value.into(),
                selection_verb: None,
            }),
    );
    UiPicker::new(title, items, PickerAction::LoginProvider).with_key_hints(super::PickerKeyHints {
        tab_complete: true,
        row_delete: false,
        ..Default::default()
    })
}

pub(super) fn auth_mode_picker(
    store: &dyn CredentialStore,
    provider_name: &str,
    active_auth: &str,
) -> rho_providers::credentials::CredentialResult<UiPicker> {
    let Some(descriptor) = provider::provider_descriptor(provider_name) else {
        return Ok(UiPicker::new(
            "Switch active auth mode",
            Vec::new(),
            PickerAction::SwitchAuthMode,
        ));
    };

    let mut items = Vec::new();
    for mode in descriptor.auth_modes() {
        match ProviderAuthentication::has_credentials(store, mode.id) {
            Ok(true) => {}
            Ok(false) | Err(CredentialError::InvalidData(_)) => continue,
            Err(error @ CredentialError::StoreUnavailable(_)) => return Err(error),
        }
        items.push(PickerItem {
            section: None,
            label: mode.login_label.into(),
            detail: Some(format!(
                "Use {} for {}.",
                mode.login_label, descriptor.display_name
            )),
            preview: None,
            badge: (mode.id == active_auth).then(|| PickerBadge {
                text: "active".into(),
                tone: PickerBadgeTone::Selected,
            }),
            value: mode.id.into(),
            selection_verb: None,
        });
    }
    sort_items_by_ascii_label(&mut items);

    Ok(UiPicker::new(
        format!("Switch {} auth mode", descriptor.display_name),
        items,
        PickerAction::SwitchAuthMode,
    )
    .with_confirm_verb("switch"))
}

pub(super) fn refresh_model_list_picker(available_auths: &[String]) -> UiPicker {
    let mut items = vec![PickerItem {
        section: None,
        label: "All configured providers".into(),
        detail: Some("Refresh every available provider with model discovery support.".into()),
        preview: None,
        badge: None,
        value: ALL_REFRESHABLE_PROVIDERS.into(),
        selection_verb: None,
    }];
    let mut providers = provider::visible_providers()
        .iter()
        .filter(|descriptor| descriptor.supports_model_refresh())
        .filter(|descriptor| {
            descriptor
                .auth_modes()
                .any(|mode| available_auths.iter().any(|auth| auth == mode.id))
        })
        .map(|descriptor| PickerItem {
            section: None,
            label: descriptor.display_name.into(),
            detail: Some(format!(
                "Refresh cached {} models.",
                descriptor.display_name
            )),
            preview: None,
            badge: None,
            value: descriptor.name.into(),
            selection_verb: None,
        })
        .collect::<Vec<_>>();
    sort_items_by_ascii_label(&mut providers);
    items.extend(providers);
    UiPicker::new("Refresh model lists", items, PickerAction::RefreshModelList)
}

pub(super) fn logout_provider_picker(
    store: &dyn CredentialStore,
    claude_signed_in: bool,
) -> rho_providers::credentials::CredentialResult<UiPicker> {
    let mut targets = Vec::new();
    for target in catalog::login_targets() {
        if ProviderAuthentication::has_stored_credentials(store, &target.auth)? {
            targets.push(target);
        }
    }
    let mut picker = provider_picker_for_targets("logout", PickerAction::LogoutProvider, targets);
    // Claude Code is not a Rho credential. Offer it when the caller already
    // knows the binary reports signed in so logout stays honest about the
    // global effect without probing here.
    if claude_signed_in {
        picker.items.push(PickerItem {
            section: None,
            label: super::claude_login::CLAUDE_CODE_TARGET.into(),
            detail: Some("Sign out of Claude Code everywhere the claude binary is used.".into()),
            preview: None,
            badge: None,
            value: super::claude_login::CLAUDE_CODE_TARGET.into(),
            selection_verb: None,
        });
        sort_items_by_ascii_label(&mut picker.items);
    }
    Ok(picker)
}

fn provider_picker_for_targets(
    verb: &str,
    action: PickerAction,
    targets: Vec<catalog::LoginTarget>,
) -> UiPicker {
    let mut items = targets
        .into_iter()
        .map(|target| {
            let multi_mode = provider::provider_descriptor(&target.provider)
                .map(|descriptor| {
                    descriptor
                        .auth_modes
                        .iter()
                        .filter(|mode| mode.auth_kind != provider::ProviderAuthKind::None)
                        .count()
                        > 1
                })
                .unwrap_or(false);
            let label = if multi_mode {
                format!("{} · {}", target.provider, target.label)
            } else {
                target.provider.clone()
            };
            PickerItem {
                section: None,
                label,
                detail: Some(target.label),
                preview: None,
                badge: None,
                value: target.auth,
                selection_verb: None,
            }
        })
        .collect::<Vec<_>>();
    sort_items_by_ascii_label(&mut items);

    UiPicker::new(format!("Select provider to {verb}"), items, action).with_key_hints(
        super::PickerKeyHints {
            tab_complete: true,
            row_delete: false,
            ..Default::default()
        },
    )
}
