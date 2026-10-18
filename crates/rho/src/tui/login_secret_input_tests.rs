use pretty_assertions::assert_eq;
use rho_providers::{
    auth::login_dispatch::ProviderAuthentication, credentials::MemoryCredentialStore,
    model::catalog,
};

use super::SecretInput;
use crate::tui::login::ApiKeySubmission;

// Covers: Zen and Go share one OpenCode console key, so a blank Enter on one
// login reuses the other's stored key, while a typed key still wins and an
// unrelated store offers nothing
// Owner: API-key login field
#[test]
fn blank_opencode_key_reuses_the_sibling_login_key() {
    let zen = catalog::login_target_for_auth("opencode-api-key").unwrap();
    let store = MemoryCredentialStore::default();
    let submit = |store: &MemoryCredentialStore, typed: &str| {
        let mut secret =
            SecretInput::for_provider_login(zen.clone(), /*keyless_host*/ false, store);
        secret.editor.insert_text(typed);
        secret.submission()
    };
    assert_eq!(submit(&store, ""), ApiKeySubmission::Rejected);

    ProviderAuthentication::save_api_key(&store, "opencode-go-api-key", "go-key").unwrap();

    assert_eq!(
        submit(&store, ""),
        ApiKeySubmission::Reuse {
            target: zen.clone(),
            source: "opencode-go-api-key",
        }
    );
    assert_eq!(
        submit(&store, " zen-key "),
        ApiKeySubmission::Save {
            target: zen.clone(),
            key: "zen-key".into(),
        }
    );
}
