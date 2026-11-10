use std::sync::Arc;

use rho_sdk::SecretString;

use crate::{
    auth::{
        github_copilot_token::GitHubCopilotAuthManager,
        kimi_token::{KimiAuthManager, KimiAuthSource},
        meta_token::MetaAuthManager,
        ollama_device::OllamaDeviceKey,
        xai_token::{XaiAuthManager, XaiAuthSource},
    },
    credentials::{
        load_codex_tokens, load_kimi_tokens, load_meta_tokens, load_provider_api_key,
        load_xai_tokens, CodexTokens, CredentialStore, KimiTokens, XaiTokens,
    },
    model::{
        registry::{
            missing_credential_error, missing_credentials_error, provider_runtime, ProviderRuntime,
        },
        ModelError,
    },
    provider::{self, OpenAiRuntimeAuth, ProviderAuthKind},
    providers::{
        builder::ProviderCredential,
        openai::auth::{Auth, CodexAuthSource},
        openai_compatible::CompatibleAuth,
    },
};

/// Opt-in application adapter for environment and credential-store lookup.
///
/// Provider builders never invoke this adapter implicitly. The application
/// bootstrap chooses when to acquire credentials and passes the returned value
/// into provider construction. Login and keychain UX therefore remain outside
/// provider execution and outside `rho-sdk`.
pub trait ProviderCredentialSource: Send + Sync {
    fn acquire(&self, provider: &str, auth: &str) -> Result<ProviderCredential, ModelError>;
}

/// Rho's first-party environment and OS-keychain credential adapter.
///
/// Environment overrides are evaluated only when [`Self::acquire`] is called.
/// The configured store is retained only by OAuth transports that need to
/// persist refreshed tokens; API-key transports receive an owned secret value.
#[derive(Clone)]
pub struct ApplicationCredentialSource {
    store: Arc<dyn CredentialStore>,
}

impl ApplicationCredentialSource {
    pub fn new(store: Arc<dyn CredentialStore>) -> Self {
        Self { store }
    }
}

impl ProviderCredentialSource for ApplicationCredentialSource {
    fn acquire(&self, provider: &str, auth: &str) -> Result<ProviderCredential, ModelError> {
        let profile = provider::resolve_profile(provider, auth)
            .map_err(|error| ModelError::InvalidResponse(error.to_string()))?;
        let descriptor = profile.provider;
        let selected = profile.auth;
        let runtime = provider_runtime(descriptor.name)
            .ok_or_else(|| ModelError::UnsupportedProvider(provider.to_string()))?;
        match runtime {
            ProviderRuntime::OpenAi { auth_mode: mode } => {
                let openai_auth = match mode {
                    OpenAiRuntimeAuth::ApiKey => load_openai_api_key_auth(self.store.as_ref())?,
                    OpenAiRuntimeAuth::Codex => load_codex_auth(self.store.clone())?,
                };
                Ok(ProviderCredential::OpenAi { auth: openai_auth })
            }
            ProviderRuntime::Anthropic => Ok(ProviderCredential::AnthropicApiKey(
                SecretString::new(load_anthropic_api_key(self.store.as_ref())?),
            )),
            ProviderRuntime::Google => Ok(ProviderCredential::GoogleApiKey(SecretString::new(
                load_provider_api_key_auth("google", self.store.as_ref())?,
            ))),
            ProviderRuntime::GithubCopilot => Ok(ProviderCredential::GitHubCopilot(
                GitHubCopilotAuthManager::new(self.store.clone())?,
            )),
            ProviderRuntime::OpenAiCompatible { .. } => {
                let auth = match selected.auth_kind {
                    ProviderAuthKind::None => CompatibleAuth::None,
                    ProviderAuthKind::ApiKey { .. } => CompatibleAuth::ApiKey(
                        load_api_key_for_mode(selected.auth_kind, self.store.as_ref())?,
                    ),
                    ProviderAuthKind::BearerCredential {
                        env_var,
                        account,
                        missing_message,
                        ..
                    } => CompatibleAuth::ApiKey(load_stored_bearer_key(
                        env_var,
                        account,
                        missing_message,
                        self.store.as_ref(),
                    )?),
                    ProviderAuthKind::KimiOAuth { .. } => {
                        let env_var = selected
                            .auth_kind
                            .env_var()
                            .expect("Kimi OAuth must declare an environment variable");
                        let missing = missing_credentials_error("kimi-code");
                        let (source, tokens) = env_or_stored(
                            self.store.environment_variable(env_var),
                            |access_token| KimiTokens {
                                access_token,
                                refresh_token: None,
                                expires_at_unix: None,
                                scope: String::new(),
                                token_type: "Bearer".into(),
                                expires_in: None,
                            },
                            || Ok(load_kimi_tokens(self.store.as_ref())?),
                            missing,
                            KimiAuthSource::Env,
                            KimiAuthSource::Store,
                        )?;
                        CompatibleAuth::KimiOAuth(KimiAuthManager::from_tokens(
                            self.store.clone(),
                            source,
                            tokens,
                        ))
                    }
                    ProviderAuthKind::MetaOAuth { .. } => {
                        let env_var = selected
                            .auth_kind
                            .env_var()
                            .expect("Muse subscription auth must declare an environment variable");
                        if let Some(api_key) =
                            crate::auth::meta_token::env_api_key(self.store.as_ref(), env_var)
                        {
                            CompatibleAuth::ApiKey(api_key)
                        } else {
                            let tokens = load_meta_tokens(self.store.as_ref())?
                                .ok_or_else(|| missing_credentials_error("meta-muse"))?;
                            CompatibleAuth::MetaOAuth(MetaAuthManager::from_tokens(
                                self.store.clone(),
                                tokens,
                            ))
                        }
                    }
                    ProviderAuthKind::OllamaDeviceKey { missing_message } => {
                        CompatibleAuth::OllamaDevice(load_ollama_device_key(
                            self.store.as_ref(),
                            missing_message,
                        )?)
                    }
                    ProviderAuthKind::CodexOAuth { .. }
                    | ProviderAuthKind::GithubCopilotDevice { .. }
                    | ProviderAuthKind::XaiOAuth { .. } => {
                        return Err(ModelError::UnsupportedProvider(provider.into()));
                    }
                };
                Ok(ProviderCredential::OpenAiCompatible(auth))
            }
            // Decision-model hosts serve no chat models; model selection
            // refuses them before this point.
            ProviderRuntime::SystemOne { .. } => {
                Err(ModelError::UnsupportedProvider(provider.to_string()))
            }
            ProviderRuntime::Xai => {
                let (source, tokens) = match selected.auth_kind {
                    ProviderAuthKind::ApiKey { .. } => (
                        XaiAuthSource::ApiKey,
                        XaiTokens {
                            access_token: load_api_key_for_mode(
                                selected.auth_kind,
                                self.store.as_ref(),
                            )?,
                            refresh_token: None,
                            expires_at_unix: None,
                            id_token: None,
                        },
                    ),
                    ProviderAuthKind::XaiOAuth {
                        env_var,
                        missing_message,
                        ..
                    } => env_or_stored(
                        self.store.environment_variable(env_var),
                        |access_token| XaiTokens {
                            access_token,
                            refresh_token: None,
                            expires_at_unix: None,
                            id_token: None,
                        },
                        || Ok(load_xai_tokens(self.store.as_ref())?),
                        missing_credential_error(missing_message),
                        XaiAuthSource::Env,
                        XaiAuthSource::Store,
                    )?,
                    _ => return Err(ModelError::UnsupportedProvider(provider.into())),
                };
                Ok(ProviderCredential::Xai(XaiAuthManager::from_tokens(
                    self.store.clone(),
                    source,
                    tokens,
                )))
            }
        }
    }
}

pub(crate) fn load_stored_bearer_key(
    env_var: &str,
    account: &str,
    missing_message: &'static str,
    store: &dyn CredentialStore,
) -> Result<String, ModelError> {
    if let Some(key) = store.environment_variable(env_var) {
        if !key.trim().is_empty() {
            return Ok(key);
        }
    }
    store
        .get_secret(account)?
        .filter(|key| !key.trim().is_empty())
        .ok_or_else(|| missing_credential_error(missing_message))
}

fn env_or_stored<T, S>(
    env_value: Option<String>,
    from_env: impl FnOnce(String) -> T,
    load: impl FnOnce() -> Result<Option<T>, ModelError>,
    missing: ModelError,
    env_source: S,
    store_source: S,
) -> Result<(S, T), ModelError> {
    match env_value {
        Some(value) if !value.trim().is_empty() => Ok((env_source, from_env(value))),
        _ => Ok((store_source, load()?.ok_or(missing)?)),
    }
}

pub(crate) fn load_api_key_for_mode(
    auth_kind: ProviderAuthKind,
    store: &dyn CredentialStore,
) -> Result<String, ModelError> {
    let ProviderAuthKind::ApiKey {
        env_var,
        account,
        missing_message,
        ..
    } = auth_kind
    else {
        return Err(ModelError::InvalidResponse(
            "expected API key auth kind".into(),
        ));
    };
    if let Some(key) = store.environment_variable(env_var) {
        if !key.trim().is_empty() {
            return Ok(key);
        }
    }
    store
        .get_secret(account)?
        .filter(|key| !key.trim().is_empty())
        .ok_or_else(|| missing_credential_error(missing_message))
}

fn load_provider_api_key_auth(
    provider_name: &str,
    store: &dyn CredentialStore,
) -> Result<String, ModelError> {
    let descriptor = provider::provider_descriptor(provider_name)
        .ok_or_else(|| ModelError::UnsupportedProvider(provider_name.into()))?;
    let ProviderAuthKind::ApiKey {
        env_var,
        missing_message,
        ..
    } = descriptor.default_auth().auth_kind
    else {
        return Err(ModelError::UnsupportedProvider(provider_name.into()));
    };
    if let Some(key) = store.environment_variable(env_var) {
        if !key.trim().is_empty() {
            return Ok(key);
        }
    }
    load_provider_api_key(store, descriptor.name)?
        .ok_or_else(|| missing_credential_error(missing_message))
}

fn load_openai_api_key_auth(store: &dyn CredentialStore) -> Result<Auth, ModelError> {
    let descriptor = provider::provider_descriptor("openai")
        .ok_or_else(|| ModelError::UnsupportedProvider("openai".into()))?;
    let ProviderAuthKind::ApiKey {
        env_var,
        missing_message,
        ..
    } = descriptor.default_auth().auth_kind
    else {
        return Err(ModelError::UnsupportedProvider("openai".into()));
    };
    if let Some(key) = store.environment_variable(env_var) {
        if !key.trim().is_empty() {
            return Ok(Auth::ApiKey(key));
        }
    }
    let key = load_provider_api_key(store, descriptor.name)?
        .ok_or_else(|| missing_credential_error(missing_message))?;
    Ok(Auth::ApiKey(key))
}

fn load_codex_auth(store: Arc<dyn CredentialStore>) -> Result<Auth, ModelError> {
    let env_var = provider::provider_descriptor_by_id(provider::ProviderId::OpenAiCodex)
        .default_auth()
        .auth_kind
        .env_var()
        .expect("Codex OAuth must declare an environment variable");
    if let Some(access_token) = store.environment_variable(env_var) {
        return Ok(Auth::codex(
            CodexTokens {
                access_token,
                refresh_token: None,
                id_token: None,
                account_id: store.environment_variable("CODEX_ACCOUNT_ID"),
            },
            CodexAuthSource::Env,
            store,
        ));
    }
    let tokens = load_codex_tokens(store.as_ref())?
        .ok_or_else(|| missing_credentials_error("openai-codex"))?;
    Ok(Auth::codex(tokens, CodexAuthSource::Store, store))
}

fn load_anthropic_api_key(store: &dyn CredentialStore) -> Result<String, ModelError> {
    let descriptor = provider::provider_descriptor("anthropic")
        .ok_or_else(|| ModelError::UnsupportedProvider("anthropic".into()))?;
    let ProviderAuthKind::ApiKey {
        env_var,
        missing_message,
        ..
    } = descriptor.default_auth().auth_kind
    else {
        return Err(ModelError::UnsupportedProvider("anthropic".into()));
    };
    if let Some(key) = store.environment_variable(env_var) {
        if !key.trim().is_empty() {
            return Ok(key);
        }
    }
    load_provider_api_key(store, descriptor.name)?
        .ok_or_else(|| missing_credential_error(missing_message))
}

/// Loads the `~/.ollama` device key once the user signed in to it through Rho.
pub(crate) fn load_ollama_device_key(
    store: &dyn CredentialStore,
    missing_message: &'static str,
) -> Result<OllamaDeviceKey, ModelError> {
    if !crate::credentials::ollama_device_login_recorded(store)? {
        return Err(missing_credential_error(missing_message));
    }
    load_ollama_device_key_from(|| OllamaDeviceKey::load_for(store), missing_message)
}

pub(crate) fn load_ollama_device_key_from(
    load: impl FnOnce() -> Result<OllamaDeviceKey, crate::auth::ollama_device::OllamaDeviceError>,
    missing_message: &'static str,
) -> Result<OllamaDeviceKey, ModelError> {
    load().map_err(|error| match error {
        crate::auth::ollama_device::OllamaDeviceError::MissingKey(_) => {
            missing_credential_error(missing_message)
        }
        error => ModelError::InvalidResponse(error.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        credentials::{CredentialResult, CredentialStore, MemoryCredentialStore},
        provider::OLLAMA_CLOUD_API_KEY_ACCOUNT,
    };
    use pretty_assertions::assert_eq;

    struct RejectingStore;

    impl CredentialStore for RejectingStore {
        fn get_secret(&self, _account: &str) -> CredentialResult<Option<String>> {
            panic!("Ollama credential acquisition must not read the store")
        }

        fn set_secret(&self, _account: &str, _secret: &str) -> CredentialResult<()> {
            panic!("Ollama credential acquisition must not write the store")
        }

        fn delete_secret(&self, _account: &str) -> CredentialResult<bool> {
            panic!("Ollama credential acquisition must not delete from the store")
        }
    }

    /// The credentials of a party whose environment is `environment`, as a
    /// daemon session's are its client's.
    struct PartyStore {
        environment: Vec<(&'static str, &'static str)>,
        saved: MemoryCredentialStore,
    }

    impl CredentialStore for PartyStore {
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
            self.environment
                .iter()
                .find(|(variable, _)| *variable == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    // Covers: a provider's key is read from the environment of the party
    // its credential store belongs to, never from this process's: a key in
    // that environment wins over a saved one, a party without the variable
    // gets its saved key, and one with neither gets none. A daemon session's
    // party is its client, so one client's key never signs in another's
    // session (#1779).
    // Owner: provider credential acquisition
    #[test]
    fn acquisition_reads_the_environment_of_the_stores_party() {
        let auth = "ollama-cloud-api-key";
        let variable = provider::provider_descriptor("ollama-cloud")
            .and_then(|descriptor| descriptor.auth_mode(auth))
            .and_then(|mode| mode.auth_kind.env_var())
            .expect("the profile reads an environment variable");
        let cases = [
            (
                "its environment's key wins",
                vec![(variable, "from the environment")],
                Some("saved"),
                Some("from the environment"),
            ),
            (
                "a blank variable is no key",
                vec![(variable, " ")],
                Some("saved"),
                Some("saved"),
            ),
            (
                "no variable, a saved key",
                vec![],
                Some("saved"),
                Some("saved"),
            ),
            ("neither", vec![], None, None),
        ];
        for (name, environment, saved, expected) in cases {
            let store = PartyStore {
                environment,
                saved: MemoryCredentialStore::default(),
            };
            if let Some(saved) = saved {
                store
                    .set_secret(OLLAMA_CLOUD_API_KEY_ACCOUNT, saved)
                    .unwrap();
            }

            let acquired = ApplicationCredentialSource::new(Arc::new(store))
                .acquire("ollama-cloud", auth)
                .ok()
                .map(|credential| match credential {
                    ProviderCredential::OpenAiCompatible(CompatibleAuth::ApiKey(key)) => key,
                    _ => panic!("{name}: an API key credential"),
                });

            assert_eq!(acquired.as_deref(), expected, "{name}");
        }
    }

    #[test]
    fn ollama_cloud_acquisition_reads_store_key_and_stays_isolated_from_local_ollama() {
        let store = MemoryCredentialStore::default();
        store
            .set_secret(OLLAMA_CLOUD_API_KEY_ACCOUNT, "cloud-secret")
            .unwrap();
        let source = ApplicationCredentialSource::new(Arc::new(store));

        let credential = source
            .acquire("ollama-cloud", "ollama-cloud-api-key")
            .unwrap();
        assert!(matches!(
            credential,
            ProviderCredential::OpenAiCompatible(CompatibleAuth::ApiKey(secret))
                if secret == "cloud-secret"
        ));

        let local = ApplicationCredentialSource::new(Arc::new(RejectingStore));
        assert!(matches!(
            local.acquire("ollama", "none").unwrap(),
            ProviderCredential::OpenAiCompatible(CompatibleAuth::None)
        ));
    }

    #[test]
    fn ollama_cloud_acquisition_reports_missing_credentials_without_key() {
        let source = ApplicationCredentialSource::new(Arc::new(MemoryCredentialStore::default()));
        let error = source
            .acquire("ollama-cloud", "ollama-cloud-api-key")
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            missing_credentials_error("ollama-cloud").to_string()
        );
    }

    // Covers: a stored custom-host key resolves to CompatibleAuth::ApiKey
    // Owner: provider credentials
    #[test]
    fn custom_host_acquisition_reads_store_key() {
        let _lock = crate::provider::custom_provider_registry_test_lock();
        crate::provider::reset_custom_openai_compatible_providers_for_tests();
        struct RestoreCustomProviders;
        impl Drop for RestoreCustomProviders {
            fn drop(&mut self) {
                crate::provider::reset_custom_openai_compatible_providers_for_tests();
            }
        }
        let _restore = RestoreCustomProviders;
        crate::provider::install_custom_openai_compatible_providers(["vllm"]).unwrap();

        let store = MemoryCredentialStore::default();
        store
            .set_secret("provider:vllm:api-key", "vllm-secret")
            .unwrap();
        let source = ApplicationCredentialSource::new(Arc::new(store));

        let credential = source.acquire("vllm", "vllm-api-key").unwrap();
        assert!(matches!(
            credential,
            ProviderCredential::OpenAiCompatible(CompatibleAuth::ApiKey(secret))
                if secret == "vllm-secret"
        ));
    }

    #[test]
    fn ollama_cloud_device_acquisition_reports_missing_without_key_dir() {
        let dir = tempfile::tempdir().unwrap();
        let missing_dir = dir.path().join("missing");
        let expected = provider::provider_descriptor("ollama-cloud")
            .unwrap()
            .auth_mode("ollama-cloud-device")
            .unwrap()
            .auth_kind
            .missing_message()
            .unwrap();
        let error =
            load_ollama_device_key_from(|| OllamaDeviceKey::load_from_dir(&missing_dir), expected)
                .unwrap_err();
        assert_eq!(error.to_string(), expected);
    }
}
