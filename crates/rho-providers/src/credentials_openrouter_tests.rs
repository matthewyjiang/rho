use super::{
    provider_has_credentials, save_openrouter_oauth_key, CredentialStore, MemoryCredentialStore,
};
use crate::provider::OPENROUTER_OAUTH_KEY_ACCOUNT;

#[test]
fn blank_openrouter_oauth_keys_are_rejected() {
    let store = MemoryCredentialStore::default();

    assert!(save_openrouter_oauth_key(&store, " \t").is_err());
    store
        .set_secret(OPENROUTER_OAUTH_KEY_ACCOUNT, " \t")
        .unwrap();

    assert!(!provider_has_credentials(&store, "openrouter-oauth").unwrap());
}
