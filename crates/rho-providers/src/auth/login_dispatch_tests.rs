use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use base64::{engine::general_purpose, Engine as _};

use super::{
    AuthenticationError, InteractiveLoginCompletion, InteractiveLoginMode, ProviderAuthentication,
};
use crate::{
    auth::{
        browser::BrowserAvailability,
        ollama_device::OllamaDeviceKey,
        provider_credentials::{ApplicationCredentialSource, ProviderCredentialSource},
    },
    credentials::{available_auth_modes, CredentialResult, CredentialStore, MemoryCredentialStore},
};

/// The credentials of a party that keeps its Ollama device key in `key_dir`,
/// as a daemon client that sets `OLLAMA_DEVICE_KEY_DIR` does. Parties may
/// share `saved`, as two clients of one daemon share its store.
struct Party {
    key_dir: PathBuf,
    saved: Arc<MemoryCredentialStore>,
}

impl Party {
    fn new(key_dir: &Path, saved: &Arc<MemoryCredentialStore>) -> Arc<Self> {
        Arc::new(Self {
            key_dir: key_dir.into(),
            saved: Arc::clone(saved),
        })
    }
}

impl CredentialStore for Party {
    fn get_secret(&self, account: &str) -> CredentialResult<Option<String>> {
        self.saved.get_secret(account)
    }

    fn set_secret(&self, account: &str, secret: &str) -> CredentialResult<()> {
        self.saved.set_secret(account, secret)
    }

    fn delete_secret(&self, account: &str) -> CredentialResult<bool> {
        self.saved.delete_secret(account)
    }

    fn environment_variable(&self, name: &str) -> Option<String> {
        (name == "OLLAMA_DEVICE_KEY_DIR").then(|| self.key_dir.to_string_lossy().into_owned())
    }
}

/// Starts the device-key login for whoever `store` belongs to and accepts
/// its handle, as a host does once the user says they approved. Returns the
/// public key the login asks the user to approve.
fn accept_device_login(store: &dyn CredentialStore, auth: &str) -> String {
    let login = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(ProviderAuthentication::start_interactive_login(
            store,
            auth,
            InteractiveLoginMode::Device,
            BrowserAvailability::Headless,
        ))
        .unwrap();
    let InteractiveLoginCompletion::Unconfirmed { authentication, .. } = login.completion else {
        panic!("{auth} login sends no completion callback");
    };
    authentication.accept(store).unwrap();
    let url = url::Url::parse(login.prompt.copyable_url()).unwrap();
    let (_, key) = url
        .query_pairs()
        .find(|(name, _)| name == "key")
        .expect("the connect URL names the key to approve");
    String::from_utf8(
        general_purpose::URL_SAFE_NO_PAD
            .decode(key.as_bytes())
            .unwrap(),
    )
    .unwrap()
}

// Covers: public device-login capability matches the providers with a device grant
// Owner: login dispatch
#[test]
fn supports_device_login_only_for_device_capable_providers() {
    for (provider, expected) in [
        ("xai-oauth", true),
        ("ollama-cloud-device", true),
        ("openrouter-oauth", false),
    ] {
        assert_eq!(
            ProviderAuthentication::supports_device_login(provider),
            expected,
            "{provider}"
        );
    }
}

// Covers: headless prefers device when the provider has one
// Owner: login dispatch
#[test]
fn preferred_mode_uses_device_only_when_headless_and_capable() {
    let cases = [
        (
            "openai-codex",
            BrowserAvailability::Headless,
            super::InteractiveLoginMode::Device,
        ),
        (
            "openai-codex",
            BrowserAvailability::Graphical,
            super::InteractiveLoginMode::Browser,
        ),
        (
            "github-copilot",
            BrowserAvailability::Headless,
            super::InteractiveLoginMode::Browser,
        ),
        (
            "github-copilot",
            BrowserAvailability::Graphical,
            super::InteractiveLoginMode::Browser,
        ),
        (
            "xai-oauth",
            BrowserAvailability::Headless,
            super::InteractiveLoginMode::Device,
        ),
        (
            "kimi-code",
            BrowserAvailability::Headless,
            super::InteractiveLoginMode::Browser,
        ),
        (
            "ollama-cloud-device",
            BrowserAvailability::Headless,
            super::InteractiveLoginMode::Browser,
        ),
        (
            "openrouter-oauth",
            BrowserAvailability::Headless,
            super::InteractiveLoginMode::Browser,
        ),
        (
            "openrouter-oauth",
            BrowserAvailability::Graphical,
            super::InteractiveLoginMode::Browser,
        ),
        (
            "openai",
            BrowserAvailability::Headless,
            super::InteractiveLoginMode::Browser,
        ),
    ];
    for (provider, availability, expected) in cases {
        pretty_assertions::assert_eq!(
            ProviderAuthentication::preferred_mode(provider, availability),
            expected,
            "{provider} {availability:?}"
        );
    }
}

// Covers: login method lookup refuses an ambiguous or unknown provider instead
// of guessing
// Owner: provider login dispatch
#[test]
fn login_method_rejects_ambiguous_and_unknown_providers() {
    assert!(matches!(
        ProviderAuthentication::method("missing"),
        Err(AuthenticationError::UnsupportedProvider(provider)) if provider == "missing"
    ));
    let error = ProviderAuthentication::method("ollama-cloud").unwrap_err();
    match error {
        AuthenticationError::AmbiguousProvider { provider, auth_ids } => {
            assert_eq!(provider, "ollama-cloud");
            assert!(auth_ids.contains(&"ollama-cloud-api-key"));
            assert!(auth_ids.contains(&"ollama-cloud-device"));
        }
        other => panic!("expected AmbiguousProvider, got {other:?}"),
    }
}

#[test]
fn owns_api_key_storage_and_deletion() {
    let store = MemoryCredentialStore::default();

    ProviderAuthentication::save_api_key(&store, "openai", "sk-test").unwrap();
    assert!(ProviderAuthentication::has_credentials(&store, "openai").unwrap());
    assert!(ProviderAuthentication::has_stored_credentials(&store, "openai").unwrap());
    assert!(ProviderAuthentication::delete_credentials(&store, "openai").unwrap());
    assert!(!ProviderAuthentication::has_credentials(&store, "openai").unwrap());
}

// Covers: a device key on disk stays unused until the Rho device login's
// acceptance handle records it, and logout undoes it
// Owner: login dispatch
#[test]
fn ollama_device_key_needs_rho_login_and_logout_undoes_it() {
    let dir = tempfile::tempdir().unwrap();
    OllamaDeviceKey::load_or_create(dir.path()).unwrap();
    let store = Party::new(dir.path(), &Arc::default());
    let source = ApplicationCredentialSource::new(store.clone());
    let device = "ollama-cloud-device";
    let signed_in = || ProviderAuthentication::has_credentials(store.as_ref(), device).unwrap();
    let listed = || available_auth_modes(store.as_ref()).contains(&device.to_string());
    assert!(!signed_in());
    assert!(!listed());
    assert!(source.acquire("ollama-cloud", device).is_err());

    accept_device_login(store.as_ref(), device);
    assert!(signed_in());
    assert!(listed());
    assert!(source.acquire("ollama-cloud", device).is_ok());

    assert!(ProviderAuthentication::delete_credentials(store.as_ref(), device).unwrap());
    assert!(!signed_in());
    assert!(!listed());
    assert!(source.acquire("ollama-cloud", device).is_err());
    assert!(!ProviderAuthentication::delete_credentials(store.as_ref(), device).unwrap());
    // The key belongs to the Ollama CLI too; logout must leave it on disk.
    assert!(OllamaDeviceKey::load_from_dir(dir.path()).is_ok());

    // Provider-level logout covers every mode, including the device key.
    accept_device_login(store.as_ref(), device);
    assert!(ProviderAuthentication::delete_credentials(store.as_ref(), "ollama-cloud").unwrap());
    assert!(!signed_in());
}

// Covers: the device key is kept where the party signing in keeps it. Two
// parties that share a store, as two clients of one daemon do, each get a
// key of their own in their own `OLLAMA_DEVICE_KEY_DIR`: one's login creates
// no key for the other and does not sign the other in, and each is asked to
// approve the key in its own directory (#1789).
// Owner: login dispatch
#[test]
fn each_party_signs_in_with_the_device_key_in_its_own_directory() {
    let (first_dir, second_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let saved = Arc::default();
    let first = Party::new(first_dir.path(), &saved);
    let second = Party::new(second_dir.path(), &saved);
    let device = "ollama-cloud-device";
    let signed_in = |party: &Party| ProviderAuthentication::has_credentials(party, device).unwrap();
    let on_disk = |dir: &Path| {
        OllamaDeviceKey::load_from_dir(dir)
            .ok()
            .map(|key| key.public_key_openssh().to_owned())
    };

    let first_approved = accept_device_login(first.as_ref(), device);
    let after_first = (
        signed_in(&first),
        signed_in(&second),
        on_disk(second_dir.path()),
    );
    let second_approved = accept_device_login(second.as_ref(), device);

    assert_eq!(after_first, (true, false, None));
    assert_ne!(first_approved, second_approved);
    assert_eq!(
        (
            Some(first_approved),
            Some(second_approved),
            signed_in(&second),
        ),
        (on_disk(first_dir.path()), on_disk(second_dir.path()), true,)
    );
}

// Covers: one OpenCode console key logs in to Zen and Go, so login offers a
// sibling's stored key; unrelated or unstored profiles offer nothing, and a
// profile with its own key is not offered a replacement
// Owner: login dispatch
#[test]
fn opencode_logins_reuse_a_sibling_stored_key() {
    let store = MemoryCredentialStore::default();
    let reuse_source =
        |auth| ProviderAuthentication::reusable_api_key_source(&store, auth).unwrap();
    assert_eq!(reuse_source("opencode-api-key"), None);

    ProviderAuthentication::save_api_key(&store, "opencode-go-api-key", "console-key").unwrap();

    let go = reuse_source("opencode-api-key").expect("the stored Go key is offered to Zen");
    assert_eq!(go.id, "opencode-go-api-key");
    assert_eq!(reuse_source("opencode-go-api-key"), None);
    assert_eq!(reuse_source("api-key"), None);
    let shared = |auth| ProviderAuthentication::shared_api_key(&store, go, auth).unwrap();
    assert_eq!(shared("opencode-api-key").as_deref(), Some("console-key"));
    assert_eq!(shared("anthropic-api-key"), None);

    ProviderAuthentication::save_api_key(&store, "opencode-api-key", "zen-key").unwrap();
    assert_eq!(reuse_source("opencode-api-key"), None);
}
