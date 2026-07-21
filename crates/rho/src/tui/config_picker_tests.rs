use super::{
    category_for_setting, category_picker, config_picker, permission_mode_picker,
    web_search_api_key_is_set, AGENT_CATEGORY_VALUE, CONTEXT_CATEGORY_VALUE,
    CONVERSATION_MODEL_VALUE, MODELS_CATEGORY_VALUE, PERMISSION_MODE_PREFIX, PERMISSION_MODE_VALUE,
    PROVIDERS_CATEGORY_VALUE, PROVIDER_LOGIN_VALUE, PROVIDER_LOGOUT_VALUE,
    REFRESH_MODEL_LIST_VALUE, TOOLS_CATEGORY_VALUE, UPDATES_CATEGORY_VALUE,
};
use {crate::permission::PermissionMode, rho_providers::credentials::CredentialError};

#[test]
fn config_picker_lists_scannable_categories_with_summaries() {
    let app = crate::tui::tests::test_app();
    let config = app.info.services.config_repository.load().unwrap();
    let picker = config_picker(&app.info.runtime, &config);
    let values = picker
        .items
        .iter()
        .map(|item| item.value.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        values,
        [
            MODELS_CATEGORY_VALUE,
            AGENT_CATEGORY_VALUE,
            CONTEXT_CATEGORY_VALUE,
            TOOLS_CATEGORY_VALUE,
            PROVIDERS_CATEGORY_VALUE,
            UPDATES_CATEGORY_VALUE,
        ]
    );
    assert_eq!(picker.items[0].badge.as_ref().unwrap().text, "gpt-5.5");
    assert_eq!(
        picker.items[1].badge.as_ref().unwrap().text,
        "permissions: auto"
    );
    assert_eq!(
        picker.items[2].badge.as_ref().unwrap().text,
        "auto compaction off"
    );
    assert_eq!(
        picker.items[3].badge.as_ref().unwrap().text,
        format!(
            "{} shell · search {}",
            config.inline_shell, config.web_search_provider
        )
    );
    assert!(picker.items[4].badge.is_none());
    assert_eq!(
        picker.items[5].badge.as_ref().unwrap().text,
        "startup checks on"
    );
    assert!(picker.items[0]
        .detail
        .as_deref()
        .unwrap()
        .contains("Conversation model"));
}

#[test]
fn config_search_matches_settings_inside_categories() {
    let app = crate::tui::tests::test_app();
    let config = app.info.services.config_repository.load().unwrap();
    let mut picker = config_picker(&app.info.runtime, &config);
    picker.filter = "permission mode".into();
    picker.select_first_match();

    assert_eq!(picker.selected_item().unwrap().value, AGENT_CATEGORY_VALUE);
}

#[test]
fn models_category_includes_model_settings() {
    let app = crate::tui::tests::test_app();
    let config = app.info.services.config_repository.load().unwrap();
    let picker = category_picker(MODELS_CATEGORY_VALUE, &app.info.runtime, &config).unwrap();
    let values = picker
        .items
        .iter()
        .map(|item| item.value.as_str())
        .collect::<Vec<_>>();

    assert_eq!(values[0], CONVERSATION_MODEL_VALUE);
    assert!(!values.contains(&"title_model"));
    assert_eq!(
        picker.items[0].badge.as_ref().unwrap().text,
        "openai/gpt-5.5"
    );
    assert_eq!(
        category_for_setting(CONVERSATION_MODEL_VALUE),
        Some(MODELS_CATEGORY_VALUE)
    );
}

#[test]
fn providers_category_keeps_provider_actions_together() {
    let app = crate::tui::tests::test_app();
    let config = app.info.services.config_repository.load().unwrap();
    let picker = category_picker(PROVIDERS_CATEGORY_VALUE, &app.info.runtime, &config).unwrap();
    let values = picker
        .items
        .iter()
        .map(|item| item.value.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        values,
        [
            PROVIDER_LOGIN_VALUE,
            PROVIDER_LOGOUT_VALUE,
            REFRESH_MODEL_LIST_VALUE,
        ]
    );
}

#[test]
fn agent_category_includes_current_permission_mode() {
    let app = crate::tui::tests::test_app();
    let config = app.info.services.config_repository.load().unwrap();
    let picker = category_picker(AGENT_CATEGORY_VALUE, &app.info.runtime, &config).unwrap();
    let item = picker
        .items
        .iter()
        .find(|item| item.value == PERMISSION_MODE_VALUE)
        .unwrap();

    assert_eq!(item.label, "Permission mode");
    assert_eq!(item.badge.as_ref().unwrap().text, "Auto");
}

#[test]
fn permission_mode_picker_lists_and_selects_modes() {
    let picker = permission_mode_picker(PermissionMode::Plan);
    let values = picker
        .items
        .iter()
        .map(|item| item.value.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        values,
        vec![
            format!("{PERMISSION_MODE_PREFIX}auto"),
            format!("{PERMISSION_MODE_PREFIX}plan"),
            format!("{PERMISSION_MODE_PREFIX}supervised"),
        ]
    );
    assert!(picker.items[1].badge.is_some());
    assert!(picker.items[0]
        .detail
        .as_deref()
        .unwrap()
        .contains("No permission checks"));
    assert!(picker.items[1]
        .detail
        .as_deref()
        .unwrap()
        .contains("denied"));
    assert!(picker.items[2]
        .detail
        .as_deref()
        .unwrap()
        .contains("Ask before"));
}

#[test]
fn badge_uses_legacy_web_search_key_when_store_has_no_entry() {
    assert!(web_search_api_key_is_set(Ok(None), Some("legacy-key")));
}

#[test]
fn badge_uses_legacy_web_search_key_when_store_is_unavailable() {
    assert!(web_search_api_key_is_set(
        Err(CredentialError::StoreUnavailable("test".into())),
        Some("legacy-key"),
    ));
}

#[test]
fn badge_is_unset_without_stored_or_legacy_web_search_key() {
    assert!(!web_search_api_key_is_set(Ok(None), None));
}
