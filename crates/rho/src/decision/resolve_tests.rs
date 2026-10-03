use pretty_assertions::assert_eq;
use rho_providers::{
    credentials::{CredentialError, CredentialResult, MemoryCredentialStore},
    provider::OLLAMA_API_KEY_ACCOUNT,
    CredentialStore,
};

use rho_sdk::SecretString;

use super::{api_key, ConfigError};

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
