use std::time::Instant;

use pretty_assertions::assert_eq;
use rho_protocol::{CompareState, ErrorCode, SealRecipient, SecretComparison, SecretItem};
use rho_providers::{
    auth::login_dispatch::ProviderAuthentication, credentials::MemoryCredentialStore,
};

use super::*;
use crate::config::ConfigDocument;

fn api_key(auth: &str) -> SecretItem {
    SecretItem::api_key(auth)
}

/// A `config.toml` holding one MCP server, `docs`, with the
/// `Authorization` header `header`.
fn mcp_file(header: &str) -> ConfigDocument {
    ConfigDocument::parse(&format!(
        "[mcp.servers.docs]\nurl = \"https://docs.example/mcp\"\nheaders = {{ Authorization = \"{header}\" }}\n"
    ))
    .unwrap()
}

fn none() -> ConfigDocument {
    ConfigDocument::parse("").unwrap()
}

fn store(saved: &[(&str, &str)]) -> MemoryCredentialStore {
    let store = MemoryCredentialStore::default();
    for (auth, key) in saved {
        ProviderAuthentication::save_api_key(&store, auth, key).unwrap();
    }
    store
}

// Covers: a source seals its saved keys and MCP server values and reports
// the unsaved ones, and blank keys, as missing; the target opens the
// bundle and says, per item, whether it holds the same value, another, or
// none.
// Owner: sync policy (`sync/mod.rs`, `sync/values.rs`).
#[test]
fn sealed_keys_compare_with_the_targets() {
    let source = store(&[
        ("anthropic-api-key", "sk-same"),
        ("openrouter-api-key", "sk-new"),
        ("moonshot-api-key", "sk-only-here"),
        ("google-api-key", "  "),
    ]);
    let target = store(&[
        ("anthropic-api-key", "sk-same"),
        ("openrouter-api-key", "sk-old"),
    ]);
    let recipients = seal::Recipients::default();
    let recipient = SealRecipient::new(SUITE, recipients.issue(Instant::now()), 0);

    let header = SecretItem::mcp_header("docs", "Authorization");
    let unset = SecretItem::mcp_env("docs", "TOKEN");

    let saved = values::saved_values(
        &source,
        &mcp_file("Bearer source"),
        vec![
            api_key("anthropic-api-key"),
            api_key("google-api-key"),
            api_key("openrouter-api-key"),
            api_key("moonshot-api-key"),
            header.clone(),
            unset.clone(),
        ],
    )
    .unwrap();
    assert_eq!(saved.missing, vec![api_key("google-api-key"), unset]);
    let sealed = seal_values(&recipient, saved.items, &saved.values).unwrap();
    let values = open_values(&recipients, Instant::now(), &sealed).unwrap();

    assert_eq!(
        values::compare_values(&target, &mcp_file("Bearer target"), sealed.items, &values).unwrap(),
        vec![
            SecretComparison::new(api_key("anthropic-api-key"), CompareState::Same),
            SecretComparison::new(api_key("openrouter-api-key"), CompareState::Differs),
            SecretComparison::new(api_key("moonshot-api-key"), CompareState::Absent),
            SecretComparison::new(header, CompareState::Differs),
        ]
    );
    // A profile only the source defines, such as its custom provider,
    // which the target does not know until settings sync writes it.
    assert_eq!(
        values::compare_values(
            &target,
            &none(),
            vec![api_key("only-on-source")],
            &["sk".into()]
        )
        .unwrap(),
        vec![SecretComparison::new(
            api_key("only-on-source"),
            CompareState::Absent
        )]
    );
}

// Covers: a bundle holding a kind of secret this daemon does not know, or
// naming a secret twice, is refused before opening, so its recipient
// still opens the bundle sent again without the bad items.
// Owner: sync policy (`sync/mod.rs`).
#[test]
fn a_bundle_it_cannot_take_is_refused_without_using_the_recipient() {
    let recipients = seal::Recipients::default();
    let recipient = SealRecipient::new(SUITE, recipients.issue(Instant::now()), 0);
    for (items, values) in [
        (vec![SecretItem::Unknown], vec!["sk".to_owned()]),
        (
            vec![api_key("openai"), api_key("openai")],
            vec!["sk".to_owned(), "sk-2".to_owned()],
        ),
    ] {
        let refused = seal_values(&recipient, items.clone(), &values).unwrap();
        assert_eq!(
            open_values(&recipients, Instant::now(), &refused)
                .err()
                .and_then(|error| error.error_code()),
            Some(ErrorCode::InvalidParams),
            "{items:?}"
        );
    }
    let known = seal_values(&recipient, vec![api_key("openai")], &["sk".into()]).unwrap();
    assert_eq!(
        open_values(&recipients, Instant::now(), &known)
            .unwrap()
            .to_vec(),
        vec!["sk".to_owned()]
    );
}

// Covers: of the credentials, only an API key auth profile's saved key may
// be exported; a login's credential, an unknown profile, or an unknown kind
// is refused.
// Owner: sync values (`sync/values.rs`).
#[test]
fn only_api_keys_are_exported_of_the_credentials() {
    for item in [
        api_key("codex"),
        api_key("no-such-auth"),
        SecretItem::Unknown,
    ] {
        assert_eq!(
            values::saved_values(&store(&[]), &none(), vec![item.clone()])
                .err()
                .and_then(|error| error.error_code()),
            Some(ErrorCode::InvalidParams),
            "{item:?}"
        );
    }
}

// Covers: the recipient keys go when the sync state does, even while a
// sweep timer for one is still waiting.
// Owner: sync policy (`sync/mod.rs`).
#[tokio::test]
async fn issued_keys_do_not_outlive_the_sync_state() {
    let sync = SecretSync::default();
    sync.recipient();
    let keys = Arc::downgrade(&sync.recipients);
    drop(sync);
    assert!(keys.upgrade().is_none());
}
