use super::web_search_api_key_is_set;
use crate::credentials::CredentialError;

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
