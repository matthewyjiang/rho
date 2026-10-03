use pretty_assertions::assert_eq;
use rho_providers::{
    credentials::{CredentialError, CredentialResult, MemoryCredentialStore},
    model::{
        decision_models::replace_cached_decision_models_for_tests,
        provider_models::{
            replace_cached_provider_models_for_tests, with_provider_models_cache_dir_for_tests,
            ProviderModel,
        },
        ReasoningCapabilities,
    },
    provider::{provider_descriptor, ANTHROPIC_API_KEY_ACCOUNT, OLLAMA_API_KEY_ACCOUNT},
    CredentialStore,
};

use rho_sdk::SecretString;

use super::{api_key, has_credentials, kind_mismatch, ConfigError};
use crate::config::{InternalAgentModelConfig, ModelKind};

const ENTRY: &str = "feature-screen";

/// Name, environment value, store, and expected key.
type Case<'a> = (
    &'a str,
    Option<&'a str>,
    &'a dyn CredentialStore,
    Option<&'a str>,
);

/// A store that fails every read, to show a key from the environment
/// never reads the store.
struct UnreadableStore;

impl CredentialStore for UnreadableStore {
    fn get_secret(&self, _account: &str) -> CredentialResult<Option<String>> {
        Err(CredentialError::StoreUnavailable("locked".into()))
    }
    fn set_secret(&self, _account: &str, _secret: &str) -> CredentialResult<()> {
        unreachable!()
    }
    fn delete_secret(&self, _account: &str) -> CredentialResult<bool> {
        unreachable!()
    }
}

// Covers: an Ollama API key comes from a nonblank environment variable
// without touching the credential store, else from the store; a blank
// variable does not hide a stored key, and neither source is an error
// naming the entry.
// Owner: decision-model config resolution.
#[test]
fn api_key_prefers_a_nonblank_env_var_then_the_store() {
    let stored = MemoryCredentialStore::default();
    stored
        .set_secret(OLLAMA_API_KEY_ACCOUNT, "from-store")
        .unwrap();
    let empty = MemoryCredentialStore::default();
    // `None` expects the missing-key error.
    let cases: [Case; 4] = [
        (
            "env wins",
            Some("from-env"),
            &UnreadableStore,
            Some("from-env"),
        ),
        ("blank env", Some("  "), &stored, Some("from-store")),
        ("no env", None, &stored, Some("from-store")),
        ("neither", None, &empty, None),
    ];
    for (name, env, store, expected) in cases {
        let env_lookup = |var: &str| {
            env.filter(|_| var == "RHO_OLLAMA_API_KEY")
                .map(str::to_owned)
        };

        let result = api_key(ENTRY, "ollama", "ollama-api-key", &env_lookup, store);

        let actual = match &result {
            Ok(key) => key.as_ref().map(SecretString::expose_secret),
            Err(error) => {
                assert!(
                    matches!(
                        error.downcast_ref::<ConfigError>(),
                        Some(ConfigError::MissingApiKey { entry: ENTRY, .. })
                    ),
                    "{name}: {error:#}"
                );
                None
            }
        };
        assert_eq!(actual, expected, "{name}");
    }
}

// Covers: an entry warns when its kind disagrees with what its provider
// listed: a decision entry on a model the host did not list, or a text entry
// on a listed decision model that is not also listed as a chat model, or a
// decision entry off a decision host. An untagged Ollama name matches its
// `:latest` tag, and a host that listed nothing gives no basis to warn.
// Owner: decision-model config resolution.
#[test]
fn kind_mismatch_judges_entries_by_discovered_decision_models() {
    // (name, provider, model, kind, warns)
    let cases: [(&str, &str, &str, Option<ModelKind>, bool); 9] = [
        ("untagged listed", "ollama", "clef", None, false),
        (
            "tagged listed",
            "ollama",
            "clef:latest",
            Some(ModelKind::Decision),
            false,
        ),
        (
            "decision default on a chat model",
            "ollama",
            "qwen3",
            None,
            true,
        ),
        (
            "text on a chat model",
            "ollama",
            "qwen3",
            Some(ModelKind::Text),
            false,
        ),
        (
            "text on a decision model",
            "ollama",
            "clef",
            Some(ModelKind::Text),
            true,
        ),
        ("host never listed", "typesafe", "jev-latest", None, false),
        ("text host", "openai", "gpt-5", None, false),
        (
            "decision kind on a text host",
            "openai",
            "gpt-5",
            Some(ModelKind::Decision),
            true,
        ),
        (
            "text on a model listed as both",
            "ollama",
            "clef-flash",
            Some(ModelKind::Text),
            false,
        ),
    ];
    let cache = tempfile::tempdir().unwrap();
    with_provider_models_cache_dir_for_tests(cache.path().into(), || {
        replace_cached_decision_models_for_tests(
            "ollama",
            vec!["clef:latest".into(), "clef-flash:latest".into()],
        );
        replace_cached_provider_models_for_tests(
            "ollama",
            &[ProviderModel {
                provider: "ollama".into(),
                model: "clef-flash:latest".into(),
                display_name: "clef-flash:latest".into(),
                context_window: None,
                max_output_tokens: None,
                reasoning_capabilities: ReasoningCapabilities::Unknown,
            }],
        )
        .unwrap();
        for (name, provider, model, kind, warns) in cases {
            let mut entry =
                InternalAgentModelConfig::new(provider.into(), model.into(), "keyless".into());
            if let crate::config::InternalAgentTarget::Rho(rho) = &mut entry.target {
                rho.kind = kind;
            }

            let mismatch = kind_mismatch(entry.rho().unwrap());

            assert_eq!(mismatch.is_some(), warns, "{name}: {mismatch:?}");
        }
    });
}

/// Name, provider, auth, environment value, store, and whether it is ready.
type CredentialCase<'a> = (
    &'a str,
    &'a str,
    &'a str,
    Option<&'a str>,
    &'a dyn CredentialStore,
    bool,
);

// Covers: a text screen counts as ready only when its auth mode belongs to
// its provider and has a nonblank environment variable or a stored
// credential; a keyless mode always is. This is what stops a headless run
// whose text screen cannot sign in.
// Owner: decision-model config resolution.
#[test]
fn text_screen_credentials_come_from_env_or_store() {
    let stored = MemoryCredentialStore::default();
    stored
        .set_secret(ANTHROPIC_API_KEY_ACCOUNT, "from-store")
        .unwrap();
    let empty = MemoryCredentialStore::default();
    let cases: [CredentialCase; 6] = [
        (
            "stored key",
            "anthropic",
            "anthropic-api-key",
            None,
            &stored,
            true,
        ),
        (
            "env key",
            "anthropic",
            "anthropic-api-key",
            Some("from-env"),
            &empty,
            true,
        ),
        (
            "blank env",
            "anthropic",
            "anthropic-api-key",
            Some(" "),
            &empty,
            false,
        ),
        (
            "no key",
            "anthropic",
            "anthropic-api-key",
            None,
            &empty,
            false,
        ),
        (
            "another provider's auth",
            "anthropic",
            "codex",
            None,
            &stored,
            false,
        ),
        ("keyless", "ollama", "none", None, &empty, true),
    ];
    for (name, provider, auth, env, store, ready) in cases {
        let descriptor = provider_descriptor(provider).unwrap();
        let env_lookup = |var: &str| {
            env.filter(|_| var == "ANTHROPIC_API_KEY")
                .map(str::to_owned)
        };

        assert_eq!(
            has_credentials(descriptor, auth, &env_lookup, store),
            ready,
            "{name}"
        );
    }
}
