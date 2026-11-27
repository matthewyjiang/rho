use std::time::Instant;

use pretty_assertions::assert_eq;
use rho_protocol::{CompareState, ErrorCode, SealRecipient, SecretComparison, SecretItem};
use rho_providers::credentials::{CredentialStore, MemoryCredentialStore};

use super::*;

fn api_key(auth: &str) -> SecretItem {
    SecretItem::ApiKey { auth: auth.into() }
}

fn store(saved: &[(&str, &str)]) -> MemoryCredentialStore {
    let store = MemoryCredentialStore::default();
    for (auth, key) in saved {
        store
            .set_secret(saved_account(&api_key(auth)).unwrap(), key)
            .unwrap();
    }
    store
}

// Covers: a source seals its saved keys and reports the unsaved ones as
// missing; the target opens the bundle and says, per key, whether it holds
// the same value, another, or none.
// Owner: sync policy (`sync/mod.rs`).
#[test]
fn sealed_keys_compare_with_the_targets() {
    let source = store(&[
        ("anthropic-api-key", "sk-same"),
        ("openrouter-api-key", "sk-new"),
        ("moonshot-api-key", "sk-only-here"),
    ]);
    let target = store(&[
        ("anthropic-api-key", "sk-same"),
        ("openrouter-api-key", "sk-old"),
    ]);
    let recipients = seal::Recipients::default();
    let recipient = SealRecipient::new(SUITE, recipients.issue(Instant::now()), 0);

    let saved = saved_values(
        &source,
        vec![
            api_key("anthropic-api-key"),
            api_key("google-api-key"),
            api_key("openrouter-api-key"),
            api_key("moonshot-api-key"),
        ],
    )
    .unwrap();
    assert_eq!(saved.missing, vec![api_key("google-api-key")]);
    let sealed = seal_values(&recipient, saved.items, &saved.values).unwrap();
    let values = open_values(&recipients, Instant::now(), &sealed).unwrap();

    assert_eq!(
        compare_values(&target, sealed.items, &values).unwrap(),
        vec![
            SecretComparison::new(api_key("anthropic-api-key"), CompareState::Same),
            SecretComparison::new(api_key("openrouter-api-key"), CompareState::Differs),
            SecretComparison::new(api_key("moonshot-api-key"), CompareState::Absent),
        ]
    );
}

// Covers: only an API key auth profile's saved key may be exported; a
// login's credential, an unknown profile, or an unknown kind is refused.
// Owner: sync policy (`sync/mod.rs`).
#[test]
fn only_api_keys_are_exported() {
    for item in [
        api_key("codex"),
        api_key("no-such-auth"),
        SecretItem::Unknown,
    ] {
        assert_eq!(
            saved_values(&store(&[]), vec![item.clone()])
                .err()
                .and_then(|error| error.error_code()),
            Some(ErrorCode::InvalidParams),
            "{item:?}"
        );
    }
}
