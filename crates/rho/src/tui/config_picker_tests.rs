use super::{
    config_picker, permission_mode_picker, web_search_api_key_is_set, CONVERSATION_MODEL_VALUE,
    PERMISSION_MODE_PREFIX, PERMISSION_MODE_VALUE, PROVIDER_LOGIN_VALUE, PROVIDER_LOGOUT_VALUE,
    REFRESH_MODEL_LIST_VALUE, TITLE_MODEL_VALUE,
};
use {crate::permission::PermissionMode, rho_providers::credentials::CredentialError};

#[test]
fn config_picker_includes_model_and_provider_settings() {
    let app = crate::tui::tests::test_app();
    let config = app.info.config_repository.load().unwrap();
    let picker = config_picker(&app.info, &config);
    let values = picker
        .items
        .iter()
        .map(|item| item.value.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        &values[..5],
        [
            CONVERSATION_MODEL_VALUE,
            TITLE_MODEL_VALUE,
            REFRESH_MODEL_LIST_VALUE,
            PROVIDER_LOGIN_VALUE,
            PROVIDER_LOGOUT_VALUE,
        ]
    );
    assert_eq!(
        picker.items[0].badge.as_ref().unwrap().text,
        "openai/gpt-5.5"
    );
    assert_eq!(
        picker.items[1].badge.as_ref().unwrap().text,
        "openai/gpt-5.5"
    );
}

#[test]
fn config_picker_includes_current_permission_mode() {
    let app = crate::tui::tests::test_app();
    let config = app.info.config_repository.load().unwrap();
    let picker = config_picker(&app.info, &config);
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
