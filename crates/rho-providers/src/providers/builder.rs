use std::{fmt, sync::Arc, time::Duration};

use rho_sdk::SecretString;
use url::Url;

use crate::{
    auth::{github_copilot_token::GitHubCopilotAuthManager, xai_token::XaiAuthManager},
    credentials::CredentialStore,
    model::{registry::provider_runtime, ModelError},
    provider::{self, OpenAiRuntimeAuth, ProviderAuthKind, ProviderRuntime},
    providers::{
        anthropic::AnthropicProvider,
        github_copilot::GitHubCopilotProvider,
        google::{GoogleProvider, API_BASE as GOOGLE_API_BASE},
        openai::{auth::Auth, OpenAiProvider},
        openai_compatible::{CompatibleAuth, OpenAiCompatibleProvider},
        xai::XaiProvider,
    },
    reasoning::ReasoningLevel,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const OPENAI_API_BASE: &str = "https://api.openai.com/v1";
const OPENAI_CODEX_API_BASE: &str = "https://chatgpt.com/backend-api/codex";
const ANTHROPIC_API_BASE: &str = "https://api.anthropic.com/v1";
const XAI_API_BASE: &str = "https://api.x.ai/v1";

/// Provider construction values derived explicitly from application config.
///
/// This type contains no credentials and never reads process environment or an
/// OS credential store. Endpoint and timeout overrides are opt-in and typed so
/// construction cannot confuse positional strings or durations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderBuildOptions {
    provider: String,
    auth: String,
    model: String,
    endpoint: Option<Url>,
    request_timeout: Option<Duration>,
    /// Prefer provider-hosted web search when the transport supports it.
    hosted_web_search: bool,
}

impl ProviderBuildOptions {
    /// The reasoning argument is retained for application bootstrap compatibility.
    /// Providers intentionally do not cache it; each request owns its reasoning level.
    pub fn new(
        provider: impl Into<String>,
        model: impl Into<String>,
        _reasoning: ReasoningLevel,
    ) -> Result<Self, ModelError> {
        let provider = provider.into();
        let model = model.into();
        if provider.trim().is_empty() {
            return Err(ModelError::InvalidResponse(
                "provider name must not be empty".into(),
            ));
        }
        if model.trim().is_empty() {
            return Err(ModelError::InvalidResponse(
                "model name must not be empty".into(),
            ));
        }
        let profile = provider::resolve_provider_reference(&provider)
            .map_err(|error| ModelError::InvalidResponse(error.to_string()))?;
        Ok(Self {
            provider: profile.provider_name().into(),
            auth: profile.auth_id().into(),
            model,
            endpoint: None,
            request_timeout: None,
            hosted_web_search: true,
        })
    }

    /// Selects an auth profile registered for this provider (or a same-runtime legacy profile).
    pub fn with_auth(mut self, auth: impl Into<String>) -> Result<Self, ModelError> {
        let auth = auth.into();
        let profile = provider::resolve_profile(&self.provider, &auth)
            .map_err(|error| ModelError::InvalidResponse(error.to_string()))?;
        self.provider = profile.provider_name().into();
        self.auth = profile.auth_id().into();
        Ok(self)
    }

    /// Overrides the provider API base or chat endpoint.
    pub fn endpoint(mut self, endpoint: Url) -> Result<Self, ModelError> {
        if !matches!(endpoint.scheme(), "http" | "https") {
            return Err(ModelError::InvalidResponse(
                "provider endpoint must use http or https".into(),
            ));
        }
        self.endpoint = Some(endpoint);
        Ok(self)
    }

    /// Bounds the complete HTTP request, including streamed response delivery.
    pub fn request_timeout(mut self, timeout: Duration) -> Result<Self, ModelError> {
        if timeout.is_zero() {
            return Err(ModelError::InvalidResponse(
                "provider request timeout must be greater than zero".into(),
            ));
        }
        self.request_timeout = Some(timeout);
        Ok(self)
    }

    /// Prefer the chat provider's hosted web search tool when supported.
    pub fn hosted_web_search(mut self, enabled: bool) -> Self {
        self.hosted_web_search = enabled;
        self
    }

    pub(crate) fn provider(&self) -> &str {
        &self.provider
    }

    pub(crate) fn auth(&self) -> &str {
        &self.auth
    }

    #[cfg(any(debug_assertions, test))]
    pub(crate) fn model(&self) -> &str {
        &self.model
    }
}

/// Explicit credential material accepted by [`ProviderBuilder`].
///
/// Formatting reveals only the credential kind. Application login, environment
/// lookup, and keychain access are intentionally absent from this type.
pub enum ProviderCredential {
    OpenAi {
        auth: Auth,
        refresh_store: Arc<dyn CredentialStore>,
    },
    AnthropicApiKey(SecretString),
    GoogleApiKey(SecretString),
    GitHubCopilot(GitHubCopilotAuthManager),
    Xai(XaiAuthManager),
    OpenAiCompatible(CompatibleAuth),
}

impl fmt::Debug for ProviderCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::OpenAi { .. } => "openai",
            Self::AnthropicApiKey(_) => "anthropic-api-key",
            Self::GoogleApiKey(_) => "google-api-key",
            Self::GitHubCopilot(_) => "github-copilot",
            Self::Xai(_) => "xai",
            Self::OpenAiCompatible(_) => "openai-compatible",
        };
        formatter
            .debug_struct("ProviderCredential")
            .field("kind", &kind)
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

/// Side-effect-free provider builder with explicit options and credentials.
///
/// Constructing the builder performs no environment or keychain access. The
/// credential kind is checked against the selected provider at [`Self::build`].
pub(crate) struct ProviderBuilder {
    options: ProviderBuildOptions,
    credential: ProviderCredential,
}

impl ProviderBuilder {
    pub(crate) fn new(options: ProviderBuildOptions, credential: ProviderCredential) -> Self {
        Self {
            options,
            credential,
        }
    }

    pub(crate) fn build(self) -> Result<Arc<dyn rho_sdk::provider::ModelProvider>, ModelError> {
        let descriptor = provider::provider_descriptor(&self.options.provider)
            .ok_or_else(|| ModelError::UnsupportedProvider(self.options.provider.clone()))?;
        let runtime =
            provider_runtime(descriptor.name).expect("registered providers must declare a runtime");
        let provider_name = descriptor.name;
        let auth_kind = descriptor
            .auth_mode(&self.options.auth)
            .map(|mode| mode.auth_kind)
            .unwrap_or_else(|| descriptor.default_auth().auth_kind);
        let client = provider_http_client(self.options.request_timeout)?;
        let endpoint = self.options.endpoint.map(|endpoint| endpoint.to_string());

        match (runtime, self.credential) {
            (
                ProviderRuntime::OpenAi { auth_mode },
                ProviderCredential::OpenAi {
                    auth,
                    refresh_store,
                },
            ) if auth_matches_mode(&auth, auth_mode) => {
                let endpoint = endpoint.or_else(|| {
                    Some(
                        match auth_mode {
                            OpenAiRuntimeAuth::ApiKey => OPENAI_API_BASE,
                            OpenAiRuntimeAuth::Codex => OPENAI_CODEX_API_BASE,
                        }
                        .to_string(),
                    )
                });
                Ok(Arc::new(OpenAiProvider::new_with_transport(
                    self.options.model,
                    auth,
                    refresh_store,
                    client,
                    endpoint,
                    self.options.hosted_web_search,
                )))
            }
            (ProviderRuntime::Anthropic, ProviderCredential::AnthropicApiKey(api_key)) => {
                let provider = AnthropicProvider::new_with_transport(
                    self.options.model,
                    api_key.into_secret(),
                    anthropic_max_tokens,
                    client,
                    endpoint.unwrap_or_else(|| ANTHROPIC_API_BASE.into()),
                );
                Ok(Arc::new(provider))
            }
            (ProviderRuntime::Google, ProviderCredential::GoogleApiKey(api_key)) => {
                Ok(Arc::new(GoogleProvider::new_with_transport(
                    self.options.model,
                    api_key.into_secret(),
                    client,
                    endpoint.unwrap_or_else(|| GOOGLE_API_BASE.into()),
                )))
            }
            (ProviderRuntime::GithubCopilot, ProviderCredential::GitHubCopilot(auth)) => {
                Ok(Arc::new(GitHubCopilotProvider::new_with_transport(
                    self.options.model,
                    auth,
                    client,
                    endpoint,
                )?))
            }
            (
                ProviderRuntime::OpenAiCompatible {
                    dialect,
                    default_api_base,
                },
                ProviderCredential::OpenAiCompatible(auth),
            ) if compatible_auth_matches_kind(&auth, auth_kind) => {
                let model = descriptor.canonicalize_model_id(&self.options.model);
                Ok(Arc::new(OpenAiCompatibleProvider::new(
                    client,
                    provider_name,
                    model,
                    dialect,
                    auth,
                    endpoint.unwrap_or_else(|| default_api_base.into()),
                )))
            }
            (ProviderRuntime::Xai, ProviderCredential::Xai(auth)) => {
                Ok(Arc::new(XaiProvider::new_with_transport(
                    "xai",
                    self.options.model,
                    auth,
                    client,
                    endpoint.unwrap_or_else(|| XAI_API_BASE.into()),
                    self.options.hosted_web_search,
                )))
            }
            _ => Err(ModelError::InvalidResponse(format!(
                "credential kind does not match provider '{}'",
                self.options.provider
            ))),
        }
    }
}

fn compatible_auth_matches_kind(auth: &CompatibleAuth, kind: ProviderAuthKind) -> bool {
    matches!(
        (auth, kind),
        (CompatibleAuth::None, ProviderAuthKind::None)
            | (CompatibleAuth::ApiKey(_), ProviderAuthKind::ApiKey { .. })
            | (
                CompatibleAuth::ApiKey(_),
                ProviderAuthKind::BearerCredential { .. }
            )
            | (
                CompatibleAuth::KimiOAuth(_),
                ProviderAuthKind::KimiOAuth { .. }
            )
            | (
                CompatibleAuth::OllamaDevice(_),
                ProviderAuthKind::OllamaDeviceKey { .. }
            )
    )
}

fn auth_matches_mode(auth: &Auth, mode: OpenAiRuntimeAuth) -> bool {
    matches!(
        (auth, mode),
        (Auth::ApiKey(_), OpenAiRuntimeAuth::ApiKey)
            | (Auth::Codex { .. }, OpenAiRuntimeAuth::Codex)
    )
}

fn provider_http_client(timeout: Option<Duration>) -> Result<reqwest::Client, ModelError> {
    let mut builder = reqwest::Client::builder().connect_timeout(CONNECT_TIMEOUT);
    if let Some(timeout) = timeout {
        builder = builder.timeout(timeout);
    }
    builder.build().map_err(ModelError::Request)
}

fn anthropic_max_tokens(model: &str) -> u32 {
    crate::model::provider_models::cached_provider_model("anthropic", model)
        .and_then(|metadata| metadata.max_output_tokens)
        .or_else(|| {
            crate::model::models_dev::cached_model_metadata("anthropic", model)
                .and_then(|metadata| metadata.max_output_tokens)
        })
        .and_then(|tokens| u32::try_from(tokens).ok())
        .unwrap_or(crate::providers::anthropic::DEFAULT_MAX_TOKENS)
}

#[cfg(test)]
#[path = "builder_tests.rs"]
mod tests;
