use super::{sort_items_by_ascii_label, PickerBadge, PickerBadgeTone, PickerItem, UiPicker};
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
        .map(|group| {
            let label = group.prompt.clone();
            let value = group.id.clone();
            // Nested method labels and values stay searchable from the top
            // level, so `cursor` or `kimi` still finds the group that owns it.
            let search_terms = login_method_items(group)
                .into_iter()
                .flat_map(|method| [method.label, method.value])
                .collect();
            PickerItem {
                section: None,
                label,
                detail: None,
                preview: None,
                badge: None,
                value,
                selection_verb: None,
                allow_filter_completion: true,
                search_terms,
            }
        })
        .collect::<Vec<_>>();
    items.extend(super::custom_provider_login::login_group_items());
    sort_items_by_ascii_label(&mut items);
    UiPicker::login_group("Select provider to login", items).with_key_hints(super::PickerKeyHints {
        tab: super::TabKey::CompleteFilter,
        ..Default::default()
    })
}

/// One row per auth mode. The value is the auth id, so confirm deletes that mode only.
pub(super) fn logout_method_picker(group: catalog::LoginGroup) -> UiPicker {
    let title = format!("Select {} logout method", group.prompt);
    let items = group
        .methods
        .into_iter()
        .map(|method| PickerItem {
            section: None,
            label: method.prompt,
            detail: Some(method.target.label.into()),
            preview: None,
            badge: None,
            value: method.target.auth,
            selection_verb: None,
            allow_filter_completion: true,
            search_terms: Vec::new(),
        })
        .collect();
    UiPicker::logout_provider(title, items).with_key_hints(super::PickerKeyHints {
        tab: super::TabKey::CompleteFilter,
        ..Default::default()
    })
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

pub(super) fn login_method_picker(group: catalog::LoginGroup) -> UiPicker {
    let title = format!("Select {} login method", group.prompt);
    UiPicker::login_provider(title, login_method_items(group)).with_key_hints(
        super::PickerKeyHints {
            tab: super::TabKey::CompleteFilter,
            ..Default::default()
        },
    )
}

/// Methods for one login group: catalog providers plus any delegated runtime
/// offered under the same group.
fn login_method_items(group: catalog::LoginGroup) -> Vec<PickerItem> {
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
            allow_filter_completion: true,
            search_terms: Vec::new(),
        })
        .collect::<Vec<_>>();
    items.extend(
        super::login_target::external_login_methods()
            .into_iter()
            .filter(|method| method.group_id == group_id)
            .map(external_login_picker_item),
    );
    items
}

pub(super) fn auth_mode_picker(
    store: &dyn CredentialStore,
    provider_name: &str,
    active_auth: &str,
) -> rho_providers::credentials::CredentialResult<UiPicker> {
    let Some(descriptor) = provider::provider_descriptor(provider_name) else {
        return Ok(UiPicker::switch_auth_mode(
            "Switch active auth mode",
            Vec::new(),
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
            detail: Some(
                format!("Use {} for {}.", mode.login_label, descriptor.display_name).into(),
            ),
            preview: None,
            badge: (mode.id == active_auth).then(|| PickerBadge {
                text: "active".into(),
                tone: PickerBadgeTone::Selected,
            }),
            value: mode.id.into(),
            selection_verb: None,
            allow_filter_completion: true,
            search_terms: Vec::new(),
        });
    }
    sort_items_by_ascii_label(&mut items);

    Ok(UiPicker::switch_auth_mode(
        format!("Switch {} auth mode", descriptor.display_name),
        items,
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
        allow_filter_completion: true,
        search_terms: Vec::new(),
    }];
    let mut providers = provider::providers()
        .iter()
        .filter(|descriptor| {
            descriptor.supports_model_refresh()
                || rho_providers::model::decision_models::lists_decision_models(descriptor.name)
        })
        .filter(|descriptor| {
            descriptor
                .auth_modes()
                .any(|mode| available_auths.iter().any(|auth| auth == mode.id))
        })
        .map(|descriptor| PickerItem {
            section: None,
            label: descriptor.display_name.into(),
            detail: Some(format!("Refresh cached {} models.", descriptor.display_name).into()),
            preview: None,
            badge: None,
            value: descriptor.name.into(),
            selection_verb: None,
            allow_filter_completion: true,
            search_terms: Vec::new(),
        })
        .collect::<Vec<_>>();
    sort_items_by_ascii_label(&mut providers);
    items.extend(providers);
    UiPicker::refresh_model_list("Refresh model lists", items)
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
    let mut picker = provider_picker_for_targets("logout", targets);
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
            allow_filter_completion: true,
            search_terms: Vec::new(),
        });
        sort_items_by_ascii_label(&mut picker.items);
    }
    Ok(picker)
}

fn external_login_picker_item(method: super::login_target::ExternalLoginMethod) -> PickerItem {
    PickerItem {
        section: None,
        label: method.label(),
        detail: Some(method.detail.into()),
        preview: None,
        badge: None,
        value: method.value.into(),
        selection_verb: None,
        allow_filter_completion: true,
        search_terms: Vec::new(),
    }
}

fn provider_picker_for_targets(verb: &str, targets: Vec<catalog::LoginTarget>) -> UiPicker {
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
                detail: Some(target.label.into()),
                preview: None,
                badge: None,
                value: target.auth,
                selection_verb: None,
                allow_filter_completion: true,
                search_terms: Vec::new(),
            }
        })
        .collect::<Vec<_>>();
    sort_items_by_ascii_label(&mut items);

    UiPicker::logout_provider(format!("Select provider to {verb}"), items).with_key_hints(
        super::PickerKeyHints {
            tab: super::TabKey::CompleteFilter,
            ..Default::default()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Covers: a multi-mode provider logout lists each auth id instead of deleting every mode
    // Owner: logout picker
    #[test]
    fn meta_logout_picker_lists_each_auth_id() {
        let picker = logout_method_picker(catalog::login_group("meta").expect("meta login group"));
        let values = picker
            .items
            .iter()
            .map(|item| item.value.as_str())
            .collect::<Vec<_>>();
        pretty_assertions::assert_eq!(values, ["meta-api-key", "meta-muse"]);
    }

    // Covers: typing a nested method label or value at the top level finds the
    // group that owns it, since delegated runtimes are no longer top-level rows
    // Owner: login group picker
    #[test]
    fn login_group_filter_finds_groups_by_nested_methods() {
        let mut picker = login_group_picker();
        let mut groups_for = |filter: &str| {
            picker.filter = filter.into();
            picker
                .matching_indices()
                .iter()
                .map(|&index| picker.items[index].value.clone())
                .collect::<Vec<_>>()
        };
        for (filter, expected) in [
            ("cursor", ["xai"]),
            ("antigravity", ["google"]),
            ("claude-code", ["anthropic"]),
            ("kimi-oauth", ["moonshot"]),
        ] {
            pretty_assertions::assert_eq!(groups_for(filter), expected, "{filter:?}");
        }
    }
}
