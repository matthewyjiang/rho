use std::{fmt, sync::Arc, time::Duration};

use rho_sdk::SecretString;
use url::Url;

use crate::{
    auth::{github_copilot_token::GitHubCopilotAuthManager, xai_token::XaiAuthManager},
    credentials::{CredentialResult, CredentialStore},
    model::{models_dev::CatalogSdkAdapter, ModelError},
    openai_compatible_dialect::OpenAiCompatibleDialect,
    provider::{
        self, CatalogConstruction, OpenAiCompatibleApi, OpenAiRuntimeAuth, ProviderAuthKind,
        ProviderRuntime,
    },
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
    profile: provider::ResolvedProviderProfile,
    model: String,
    endpoint: Option<Url>,
    request_timeout: Option<Duration>,
    /// Prefer provider-hosted web search when the transport supports it.
    hosted_web_search: bool,
    /// Prefer xAI hosted image generation when the transport supports it.
    hosted_image_generation: bool,
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
            profile,
            model,
            endpoint: None,
            request_timeout: None,
            hosted_web_search: true,
            hosted_image_generation: true,
        })
    }

    /// Selects an auth profile registered for this provider (or a same-runtime legacy profile).
    pub fn with_auth(mut self, auth: impl Into<String>) -> Result<Self, ModelError> {
        let auth = auth.into();
        self.profile = provider::resolve_profile(self.profile.provider_name(), &auth)
            .map_err(|error| ModelError::InvalidResponse(error.to_string()))?;
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

    /// Prefer xAI's hosted image generation tool when supported.
    pub fn hosted_image_generation(mut self, enabled: bool) -> Self {
        self.hosted_image_generation = enabled;
        self
    }

    pub(crate) fn provider(&self) -> &str {
        self.profile.provider_name()
    }

    /// Awaits the models.dev hydrate when the resolved provider picks its
    /// wire adapter from the catalog `npm` mapping instead of its declared
    /// runtime; a no-op for every other provider.
    ///
    /// The hydrate is bounded and stays cache-only offline, leaving
    /// construction on the declared-runtime fallback.
    pub async fn ensure_catalog_for_construction(&self) {
        if self.profile.provider.runtime.catalog_construction()
            == CatalogConstruction::PreferModelsDevNpm
        {
            crate::model::models_dev::ensure_models_dev_catalog().await;
        }
    }

    pub(crate) fn auth(&self) -> &str {
        self.profile.auth_id()
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
        // Identity was resolved when the options were built. Consume that
        // descriptor directly so a later dropped custom-host scope cannot
        // change construction and build never bypasses visibility checks.
        let descriptor = self.options.profile.provider;
        let runtime = descriptor.runtime;
        let provider_name = descriptor.name;
        let auth_kind = self.options.profile.auth_kind();
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
                    catalog_construction,
                },
                ProviderCredential::OpenAiCompatible(auth),
            ) if compatible_auth_matches_kind(&auth, auth_kind) => {
                let model = descriptor.canonicalize_model_id(&self.options.model);
                let api_base = if descriptor.is_custom_openai_compatible() {
                    endpoint.ok_or_else(|| {
                        ModelError::InvalidResponse(format!(
                            "custom provider '{provider_name}' requires a configured base_url"
                        ))
                    })?
                } else {
                    endpoint.unwrap_or_else(|| default_api_base.into())
                };
                build_openai_compatible_provider(
                    catalog_construction,
                    descriptor.openai_compatible_api(),
                    provider_name,
                    model,
                    OpenAiCompatibleBuild {
                        dialect,
                        auth,
                        api_base,
                        client,
                        hosted_web_search: self.options.hosted_web_search,
                    },
                )
            }
            (ProviderRuntime::Xai, ProviderCredential::Xai(auth)) => {
                Ok(Arc::new(XaiProvider::new_with_transport(
                    "xai",
                    self.options.model,
                    auth,
                    client,
                    endpoint.unwrap_or_else(|| XAI_API_BASE.into()),
                    crate::providers::xai::XaiHostedTools {
                        web_search: self.options.hosted_web_search,
                        image_generation: self.options.hosted_image_generation,
                    },
                )))
            }
            _ => Err(ModelError::InvalidResponse(format!(
                "credential kind does not match provider '{provider_name}'"
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
    match mode {
        OpenAiRuntimeAuth::ApiKey => matches!(auth, Auth::ApiKey(_)),
        OpenAiRuntimeAuth::Codex => matches!(auth, Auth::Codex { .. }),
    }
}

fn provider_http_client(timeout: Option<Duration>) -> Result<reqwest::Client, ModelError> {
    let mut builder = reqwest::Client::builder().connect_timeout(CONNECT_TIMEOUT);
    if let Some(timeout) = timeout {
        builder = builder.timeout(timeout);
    }
    builder.build().map_err(ModelError::Request)
}

struct OpenAiCompatibleBuild {
    dialect: OpenAiCompatibleDialect,
    auth: CompatibleAuth,
    api_base: String,
    client: reqwest::Client,
    hosted_web_search: bool,
}

/// Empty store for construction paths that require a `CredentialStore` but
/// never read or write secrets. `MemoryCredentialStore` is debug/test-only.
struct InertCredentialStore;

impl CredentialStore for InertCredentialStore {
    fn get_secret(&self, _account: &str) -> CredentialResult<Option<String>> {
        Ok(None)
    }

    fn set_secret(&self, _account: &str, _secret: &str) -> CredentialResult<()> {
        Ok(())
    }

    fn delete_secret(&self, _account: &str) -> CredentialResult<bool> {
        Ok(false)
    }
}

fn build_openai_compatible_provider(
    catalog_construction: CatalogConstruction,
    openai_compatible_api: OpenAiCompatibleApi,
    provider_name: &'static str,
    model: String,
    build: OpenAiCompatibleBuild,
) -> Result<Arc<dyn rho_sdk::provider::ModelProvider>, ModelError> {
    // Declared Responses and Anthropic Messages are host APIs, not npm
    // construction policy. Adapter choice from catalog npm is only the Chat
    // Completions fallback for PreferModelsDevNpm gateways.
    let adapter = match openai_compatible_api {
        OpenAiCompatibleApi::Responses => CatalogSdkAdapter::OpenAiResponses,
        OpenAiCompatibleApi::AnthropicMessages => CatalogSdkAdapter::AnthropicMessages,
        OpenAiCompatibleApi::ChatCompletions => match catalog_construction {
            CatalogConstruction::Runtime => CatalogSdkAdapter::OpenAiCompatible,
            CatalogConstruction::PreferModelsDevNpm => CatalogSdkAdapter::from_sdk_package(
                crate::model::models_dev::cached_model_metadata(provider_name, &model)
                    .as_ref()
                    .and_then(|metadata| metadata.sdk_package.as_deref()),
            ),
        },
    };
    // Declared Responses hosts do not advertise OpenAI hosted tools. Catalog npm
    // Responses (opencode-go) still follows the caller flag.
    let hosted_web_search = if openai_compatible_api == OpenAiCompatibleApi::Responses {
        false
    } else {
        build.hosted_web_search
    };
    match (adapter, build.auth) {
        (CatalogSdkAdapter::OpenAiResponses, CompatibleAuth::ApiKey(key)) => {
            // Api-key and keyless Responses construction never refresh tokens,
            // so an inert store satisfies the Codex refresh dependency without
            // touching real credentials.
            // NEXT_MAJOR(rho-providers): give Responses construction explicit
            // api-key and keyless paths so they no longer need a placeholder
            // CredentialStore to satisfy the Codex refresh dependency.
            Ok(Arc::new(OpenAiProvider::new_with_identity(
                model,
                Some(Auth::ApiKey(key)),
                Arc::new(InertCredentialStore),
                build.client,
                Some(build.api_base),
                hosted_web_search,
                provider_name,
            )))
        }
        (CatalogSdkAdapter::OpenAiResponses, CompatibleAuth::None) => {
            Ok(Arc::new(OpenAiProvider::new_with_identity(
                model,
                None,
                Arc::new(InertCredentialStore),
                build.client,
                Some(build.api_base),
                hosted_web_search,
                provider_name,
            )))
        }
        (CatalogSdkAdapter::AnthropicMessages, CompatibleAuth::ApiKey(key)) => {
            Ok(Arc::new(AnthropicProvider::new_with_identity(
                model,
                key,
                build.client,
                build.api_base,
                provider_name,
            )))
        }
        (CatalogSdkAdapter::AnthropicMessages, _) => Err(ModelError::InvalidResponse(format!(
            "provider '{provider_name}' Anthropic Messages requires an API key"
        ))),
        // Chat Completions is the descriptor's declared runtime, so it is also
        // the fallback when the catalog has no row yet (cold cache before the
        // first hydrate) or names an unrecognized package. OpenAiResponses
        // plus Kimi/Ollama device auth also falls through; custom Responses
        // hosts only construct on ApiKey or None.
        (_, auth) => Ok(Arc::new(OpenAiCompatibleProvider::new(
            build.client,
            provider_name,
            model,
            build.dialect,
            auth,
            build.api_base,
        ))),
    }
}

#[cfg(test)]
#[path = "builder_tests.rs"]
mod tests;
