use pretty_assertions::assert_eq;
use rho_providers::{model::catalog, provider};

use super::{ApiKeySubmission, BlankKey, SecretInput};

// Covers: a field offering a sibling login's key reuses it on a blank Enter,
// while a typed key still wins
// Owner: API-key login field
#[test]
fn blank_key_reuses_the_offered_sibling_key() {
    let zen = catalog::login_target_for_auth("opencode-api-key").unwrap();
    let (_, go) = provider::resolve_auth_mode("opencode-go-api-key").unwrap();
    let submit = |typed: &str| {
        let mut secret = SecretInput::with_blank(zen.clone(), BlankKey::Reuse(go));
        secret.editor.insert_text(typed);
        secret.submission()
    };

    assert_eq!(
        submit(""),
        ApiKeySubmission::Reuse {
            target: zen.clone(),
            source: go,
        }
    );
    assert_eq!(
        submit(" zen-key "),
        ApiKeySubmission::Save {
            target: zen.clone(),
            key: "zen-key".into(),
        }
    );
}
