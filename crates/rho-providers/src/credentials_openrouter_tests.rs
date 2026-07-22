use std::sync::Arc;

use super::{
    provider_has_credentials, save_openrouter_oauth_key, CredentialStore, MemoryCredentialStore,
};
use crate::{
    auth::provider_credentials::{ApplicationCredentialSource, ProviderCredentialSource},
    model::registry::missing_credentials_error,
    provider::OPENROUTER_OAUTH_KEY_ACCOUNT,
};

#[test]
fn blank_openrouter_oauth_keys_are_rejected() {
    let store = MemoryCredentialStore::default();

    assert!(save_openrouter_oauth_key(&store, " \t").is_err());
    store
        .set_secret(OPENROUTER_OAUTH_KEY_ACCOUNT, " \t")
        .unwrap();

    assert!(!provider_has_credentials(&store, "openrouter-oauth").unwrap());
}

#[test]
fn missing_openrouter_oauth_credentials_name_the_selected_login_profile() {
    let source = ApplicationCredentialSource::new(Arc::new(MemoryCredentialStore::default()));
    let error = match source.acquire("openrouter-oauth") {
        Ok(_) => panic!("credential acquisition unexpectedly succeeded"),
        Err(error) => error,
    };
    let refresh_error = missing_credentials_error("openrouter-oauth");

    for message in [error.to_string(), refresh_error.to_string()] {
        assert!(message.contains("/login openrouter-oauth"), "{message}");
        assert!(!message.contains("/login openrouter in"), "{message}");
    }
}
