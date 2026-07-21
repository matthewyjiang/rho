use std::sync::Arc;

use rho_sdk::SecretString;

use crate::{
    auth::{
        github_copilot_token::GitHubCopilotAuthManager,
        kimi_token::{KimiAuthManager, KimiAuthSource},
        xai_token::{XaiAuthManager, XaiAuthSource},
    },
    credentials::{
        load_codex_tokens, load_kimi_tokens, load_provider_api_key, load_xai_tokens, CodexTokens,
        CredentialStore, KimiTokens, XaiTokens,
    },
    model::{
        registry::{
            missing_credential_error, provider_runtime, AuthMode, ProviderRuntime, XaiAuthMode,
        },
        ModelError,
    },
    provider::{self, ProviderAuthKind},
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
    fn acquire(&self, provider: &str) -> Result<ProviderCredential, ModelError>;
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
    fn acquire(&self, provider: &str) -> Result<ProviderCredential, ModelError> {
        let runtime = provider_runtime(provider)
            .ok_or_else(|| ModelError::UnsupportedProvider(provider.to_string()))?;
        match runtime {
            ProviderRuntime::OpenAi { auth_mode } => {
                let auth = match auth_mode {
                    AuthMode::ApiKey => load_openai_api_key_auth(self.store.as_ref())?,
                    AuthMode::Codex => load_codex_auth(self.store.as_ref())?,
                };
                Ok(ProviderCredential::OpenAi {
                    auth,
                    refresh_store: self.store.clone(),
                })
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
                let descriptor = provider::provider_descriptor(provider)
                    .expect("compatible provider runtime must be registered");
                let auth = match descriptor.auth_kind {
                    ProviderAuthKind::None => CompatibleAuth::None,
                    ProviderAuthKind::ApiKey { .. } => CompatibleAuth::ApiKey(
                        load_provider_api_key_auth(provider, self.store.as_ref())?,
                    ),
                    ProviderAuthKind::KimiOAuth { .. } => {
                        let env_var = descriptor
                            .auth_kind
                            .env_var()
                            .expect("Kimi OAuth must declare an environment variable");
                        let (source, tokens) = match std::env::var(env_var) {
                            Ok(access_token) if !access_token.trim().is_empty() => (
                                KimiAuthSource::Env,
                                KimiTokens {
                                    access_token,
                                    refresh_token: None,
                                    expires_at_unix: None,
                                    scope: String::new(),
                                    token_type: "Bearer".into(),
                                    expires_in: None,
                                },
                            ),
                            _ => (
                                KimiAuthSource::Store,
                                load_kimi_tokens(self.store.as_ref())?
                                    .ok_or(ModelError::MissingKimiAuth)?,
                            ),
                        };
                        CompatibleAuth::KimiOAuth(KimiAuthManager::from_tokens(
                            self.store.clone(),
                            source,
                            tokens,
                        ))
                    }
                    _ => return Err(ModelError::UnsupportedProvider(provider.into())),
                };
                Ok(ProviderCredential::OpenAiCompatible(auth))
            }
            ProviderRuntime::Xai { auth_mode } => {
                let (source, tokens) = match auth_mode {
                    XaiAuthMode::ApiKey => (
                        XaiAuthSource::ApiKey,
                        XaiTokens {
                            access_token: load_provider_api_key_auth("xai", self.store.as_ref())?,
                            refresh_token: None,
                            expires_at_unix: None,
                            id_token: None,
                        },
                    ),
                    XaiAuthMode::OAuth => {
                        let descriptor = provider::provider_descriptor("xai-oauth")
                            .expect("xAI OAuth provider must be registered");
                        match std::env::var(
                            descriptor
                                .auth_kind
                                .env_var()
                                .expect("xAI OAuth must declare an environment variable"),
                        ) {
                            Ok(access_token) if !access_token.trim().is_empty() => (
                                XaiAuthSource::Env,
                                XaiTokens {
                                    access_token,
                                    refresh_token: None,
                                    expires_at_unix: None,
                                    id_token: None,
                                },
                            ),
                            _ => (
                                XaiAuthSource::Store,
                                load_xai_tokens(self.store.as_ref())?
                                    .ok_or(ModelError::MissingXaiAuth)?,
                            ),
                        }
                    }
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

fn load_provider_api_key_auth(
    provider_name: &str,
    store: &dyn CredentialStore,
) -> Result<String, ModelError> {
    let descriptor = provider::provider_descriptor(provider_name)
        .ok_or_else(|| ModelError::UnsupportedProvider(provider_name.into()))?;
    let ProviderAuthKind::ApiKey {
        env_var, missing, ..
    } = descriptor.auth_kind
    else {
        return Err(ModelError::UnsupportedProvider(provider_name.into()));
    };
    if let Ok(key) = std::env::var(env_var) {
        if !key.trim().is_empty() {
            return Ok(key);
        }
    }
    load_provider_api_key(store, descriptor.name)?.ok_or_else(|| missing_credential_error(missing))
}

fn load_openai_api_key_auth(store: &dyn CredentialStore) -> Result<Auth, ModelError> {
    let descriptor = provider::provider_descriptor("openai")
        .ok_or_else(|| ModelError::UnsupportedProvider("openai".into()))?;
    let ProviderAuthKind::ApiKey {
        env_var, missing, ..
    } = descriptor.auth_kind
    else {
        return Err(ModelError::UnsupportedProvider("openai".into()));
    };
    if let Ok(key) = std::env::var(env_var) {
        if !key.trim().is_empty() {
            return Ok(Auth::ApiKey(key));
        }
    }
    let key = load_provider_api_key(store, descriptor.name)?
        .ok_or_else(|| missing_credential_error(missing))?;
    Ok(Auth::ApiKey(key))
}

fn load_codex_auth(store: &dyn CredentialStore) -> Result<Auth, ModelError> {
    let env_var = provider::provider_descriptor_by_id(provider::ProviderId::OpenAiCodex)
        .auth_kind
        .env_var()
        .expect("Codex OAuth must declare an environment variable");
    if let Ok(access_token) = std::env::var(env_var) {
        return Ok(Auth::Codex {
            tokens: CodexTokens {
                access_token,
                refresh_token: None,
                id_token: None,
                account_id: std::env::var("CODEX_ACCOUNT_ID").ok(),
            },
            source: CodexAuthSource::Env,
        });
    }
    let tokens = load_codex_tokens(store)?.ok_or(ModelError::MissingCodexAuth)?;
    Ok(Auth::Codex {
        tokens,
        source: CodexAuthSource::Store,
    })
}

fn load_anthropic_api_key(store: &dyn CredentialStore) -> Result<String, ModelError> {
    let descriptor = provider::provider_descriptor("anthropic")
        .ok_or_else(|| ModelError::UnsupportedProvider("anthropic".into()))?;
    let ProviderAuthKind::ApiKey {
        env_var, missing, ..
    } = descriptor.auth_kind
    else {
        return Err(ModelError::UnsupportedProvider("anthropic".into()));
    };
    if let Ok(key) = std::env::var(env_var) {
        if !key.trim().is_empty() {
            return Ok(key);
        }
    }
    load_provider_api_key(store, descriptor.name)?.ok_or_else(|| missing_credential_error(missing))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::{CredentialResult, CredentialStore};

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

    #[test]
    fn ollama_acquisition_returns_explicit_no_auth_without_store_access() {
        let source = ApplicationCredentialSource::new(Arc::new(RejectingStore));

        let credential = source.acquire("ollama").unwrap();

        assert!(matches!(
            credential,
            ProviderCredential::OpenAiCompatible(CompatibleAuth::None)
        ));
    }
}
