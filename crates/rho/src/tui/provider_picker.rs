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
        .map(|group| PickerItem {
            search_terms: login_group_search_terms(&group),
            ..PickerItem::new(group.prompt, group.id)
        })
        .collect::<Vec<_>>();
    items.extend(super::custom_provider_login::login_group_items());
    sort_items_by_ascii_label(&mut items);
    UiPicker::login_group("Select provider to login", items).with_key_hints(super::PickerKeyHints {
        tab: super::TabKey::CompleteFilter,
        ..Default::default()
    })
}

/// What a user might type to reach a group through a method nested under it.
///
/// A catalog method contributes the distinctive words of its login label: the
/// label minus its generic auth prompt, so "Gemini API key" under "API Key"
/// yields `gemini`. A delegated runtime contributes its label, value, and
/// aliases. Auth ids stay out, since their `api-key` and `oauth` stems would
/// match nearly every group.
fn login_group_search_terms(group: &catalog::LoginGroup) -> Vec<String> {
    let mut terms = Vec::new();
    for method in &group.methods {
        let generic = method.prompt.to_lowercase();
        let generic = generic.split_whitespace().collect::<Vec<_>>();
        terms.extend(
            method
                .target
                .label
                .split_whitespace()
                .map(str::to_lowercase)
                .filter(|word| !generic.contains(&word.as_str())),
        );
    }
    for method in delegated_methods(&group.id) {
        terms.push(method.label());
        terms.push(method.value.into());
        terms.extend(method.aliases.iter().map(|alias| alias.to_string()));
    }
    terms
}

/// One row per auth mode. The value is the auth id, so confirm deletes that mode only.
pub(super) fn logout_method_picker(group: catalog::LoginGroup) -> UiPicker {
    let title = format!("Select {} logout method", group.prompt);
    let items = group
        .methods
        .into_iter()
        .map(|method| PickerItem {
            detail: Some(method.target.label.into()),
            ..PickerItem::new(method.prompt, method.target.auth)
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
        .map(|method| PickerItem::new(method.prompt, method.target.auth))
        .collect::<Vec<_>>();
    items.extend(delegated_methods(&group_id).map(external_login_picker_item));
    items
}

/// Delegated runtimes offered under one login group.
fn delegated_methods(
    group_id: &str,
) -> impl Iterator<Item = super::login_target::ExternalLoginMethod> + '_ {
    super::login_target::external_login_methods()
        .into_iter()
        .filter(move |method| method.group_id == group_id)
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
            detail: Some(
                format!("Use {} for {}.", mode.login_label, descriptor.display_name).into(),
            ),
            badge: (mode.id == active_auth).then(|| PickerBadge {
                text: "active".into(),
                tone: PickerBadgeTone::Selected,
            }),
            ..PickerItem::new(mode.login_label.into(), mode.id.into())
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
        detail: Some("Refresh every available provider with model discovery support.".into()),
        ..PickerItem::new(
            "All configured providers".into(),
            ALL_REFRESHABLE_PROVIDERS.into(),
        )
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
            detail: Some(format!("Refresh cached {} models.", descriptor.display_name).into()),
            ..PickerItem::new(descriptor.display_name.into(), descriptor.name.into())
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
            detail: Some("Sign out of Claude Code everywhere the claude binary is used.".into()),
            ..PickerItem::new(
                super::claude_login::CLAUDE_CODE_TARGET.into(),
                super::claude_login::CLAUDE_CODE_TARGET.into(),
            )
        });
        sort_items_by_ascii_label(&mut picker.items);
    }
    Ok(picker)
}

fn external_login_picker_item(method: super::login_target::ExternalLoginMethod) -> PickerItem {
    PickerItem {
        detail: Some(method.detail.into()),
        ..PickerItem::new(method.label(), method.value.into())
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
                detail: Some(target.label.into()),
                ..PickerItem::new(label, target.auth)
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

    // Covers: typing a nested method's distinctive name or a delegated runtime
    // at the top level finds the group that owns it, while generic auth words
    // and auth-id stems match no group
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
        // Only rows whose own visible text says "API" or "key" (custom host
        // details, "Meta Model API"); no group may match through a generic
        // auth word or auth-id stem it nests.
        let custom_hosts = vec![
            super::super::custom_provider_login::NEW_CUSTOM_CHAT_COMPLETIONS_HOST_VALUE,
            super::super::custom_provider_login::NEW_CUSTOM_RESPONSES_HOST_VALUE,
        ];
        for (filter, expected) in [
            ("cursor", vec!["xai"]),
            ("cursor-agent", vec!["xai"]),
            ("antigravity", vec!["google"]),
            ("gemini", vec!["google"]),
            ("claude-code", vec!["anthropic"]),
            ("kimi", vec!["moonshot"]),
            ("muse", vec!["meta"]),
            ("delegation", vec!["anthropic", "google", "xai"]),
            ("api", [custom_hosts.as_slice(), &["meta"]].concat()),
            ("key", custom_hosts.clone()),
            ("oauth", vec![]),
        ] {
            pretty_assertions::assert_eq!(groups_for(filter), expected, "{filter:?}");
        }
    }
}
