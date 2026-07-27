//! Stable provider identity and metadata shared across credential, catalog, and runtime layers.
//!
//! This module intentionally contains no credential-store or model-runtime behavior. Provider
//! adapters and `ModelError` mappings belong in the model runtime.

pub const OPENAI_API_KEY_ACCOUNT: &str = "provider:openai:api-key";
pub const ANTHROPIC_API_KEY_ACCOUNT: &str = "provider:anthropic:api-key";
pub const GOOGLE_API_KEY_ACCOUNT: &str = "provider:google:api-key";
pub const CODEX_TOKENS_ACCOUNT: &str = "provider:openai-codex:tokens";
pub const GITHUB_COPILOT_TOKENS_ACCOUNT: &str = "provider:github-copilot:tokens";
pub const XAI_API_KEY_ACCOUNT: &str = "provider:xai:api-key";
pub const XAI_TOKENS_ACCOUNT: &str = "provider:xai:tokens";
pub const MOONSHOT_API_KEY_ACCOUNT: &str = "provider:moonshot:api-key";
pub const OLLAMA_CLOUD_API_KEY_ACCOUNT: &str = "provider:ollama-cloud:api-key";
pub const OLLAMA_CLOUD_DEVICE_SESSION_ACCOUNT: &str = "provider:ollama-cloud-device:session";
pub const POOLSIDE_API_KEY_ACCOUNT: &str = "provider:poolside:api-key";
pub const OPENROUTER_API_KEY_ACCOUNT: &str = "provider:openrouter:api-key";
pub const OPENROUTER_OAUTH_KEY_ACCOUNT: &str = "provider:openrouter:oauth-key";
pub const KIMI_TOKENS_ACCOUNT: &str = "provider:kimi-code:tokens";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProviderId {
    Ollama,
    OllamaCloud,
    OpenAi,
    OpenAiCodex,
    Anthropic,
    Google,
    GithubCopilot,
    Xai,
    XaiOAuth,
    Moonshot,
    Poolside,
    OpenRouter,
    OpenRouterOAuth,
    KimiCode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RuntimeProviderId {
    Ollama,
    OllamaCloud,
    OpenAi,
    Anthropic,
    Google,
    GithubCopilot,
    Xai,
    Moonshot,
    Poolside,
    OpenRouter,
    KimiCode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserOAuthFlow {
    OpenRouter,
}

impl BrowserOAuthFlow {
    pub const fn provider_label(self) -> &'static str {
        match self {
            Self::OpenRouter => "OpenRouter",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BearerCredentialAcquisition {
    BrowserOAuth(BrowserOAuthFlow),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderModelSource {
    StaticCatalog,
    CachedProviderModels,
}

/// Defines how raw models.dev reasoning controls become application capabilities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogReasoningPolicy {
    /// Provider-specific protocol details cannot yet be represented safely.
    Unknown,
    /// This provider path does not forward a user reasoning control.
    NotConfigurable,
    /// Only controls explicitly advertised by the catalog are selectable.
    ExactAdvertised,
    /// A catalog toggle is a supported way to select `Off` for this protocol.
    OffByAdvertisedToggle,
    /// A reasoning model exposes a binary provider control as `Off` or `Max`.
    OffOrMax,
    /// The provider serializes `Off` as a provider-owned `none` control.
    OffAsNone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderModelRefreshKind {
    OpenAi,
    Anthropic,
    Google,
    GithubCopilot,
    OpenAiCompatible,
}

/// How a provider encodes model IDs on the wire versus in Rho cache/config.
///
/// Discovery, selection, and request construction should use this policy instead
/// of hard-coding provider names at call sites.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ModelIdCodec {
    /// Cache, config, identity, and wire IDs all use the same model string.
    #[default]
    Plain,
    /// Wire IDs are `{provider_name}/{internal_id}`; cache and config store the
    /// internal id only. User-facing references remain `provider/internal_id`.
    ProviderPrefixed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderAuthKind {
    None,
    ApiKey {
        env_var: &'static str,
        account: &'static str,
        entry_label: &'static str,
        missing_message: &'static str,
    },
    CodexOAuth {
        env_var: &'static str,
        account: &'static str,
        missing_message: &'static str,
    },
    GithubCopilotDevice {
        env_var: &'static str,
        account: &'static str,
        missing_message: &'static str,
    },
    XaiOAuth {
        env_var: &'static str,
        account: &'static str,
        missing_message: &'static str,
    },
    BearerCredential {
        env_var: &'static str,
        account: &'static str,
        missing_message: &'static str,
        acquisition: BearerCredentialAcquisition,
    },
    KimiOAuth {
        env_var: &'static str,
        account: &'static str,
        missing_message: &'static str,
    },
    OllamaDeviceKey {
        account: &'static str,
        missing_message: &'static str,
    },
}

impl ProviderDescriptor {
    /// Normalizes a model id for cache, config, and identity storage.
    ///
    /// For [`ModelIdCodec::ProviderPrefixed`], strips leading `{name}/` segments
    /// so legacy wire ids and double-prefixed favorites collapse to one internal id.
    pub fn canonicalize_model_id(&self, model: &str) -> String {
        match self.model_id_codec {
            ModelIdCodec::Plain => model.to_string(),
            ModelIdCodec::ProviderPrefixed => {
                let prefix = format!("{}/", self.name);
                let mut model = model;
                while let Some(rest) = model.strip_prefix(prefix.as_str()) {
                    if rest.is_empty() {
                        break;
                    }
                    model = rest;
                }
                model.to_string()
            }
        }
    }

    /// Expands an internal model id to the id sent on this provider's HTTP API.
    pub fn wire_model_id(&self, model: &str) -> String {
        match self.model_id_codec {
            ModelIdCodec::Plain => model.to_string(),
            ModelIdCodec::ProviderPrefixed => {
                let internal = self.canonicalize_model_id(model);
                format!("{}/{internal}", self.name)
            }
        }
    }

    /// Resolves a provider-facing model ID to its models.dev catalog ID.
    ///
    /// Provider model discovery remains authoritative. This only bridges model
    /// names when the provider API and metadata catalog use different IDs.
    pub fn metadata_model<'a>(&self, model: &'a str) -> &'a str {
        match (self.runtime_id, model) {
            (RuntimeProviderId::KimiCode, "k3") => "kimi-k3",
            (RuntimeProviderId::OpenRouter, model) => model
                .split_once('/')
                .map(|(_, upstream_model)| upstream_model)
                .unwrap_or(model),
            _ => model,
        }
    }

    /// Resolves an aggregator model ID to its models.dev provider.
    pub fn metadata_upstream_for_model<'a>(&self, model: &'a str) -> &'a str {
        match self.runtime_id {
            RuntimeProviderId::OpenRouter => model
                .split_once('/')
                .map(|(upstream, _)| upstream)
                .unwrap_or(self.metadata_upstream),
            _ => self.metadata_upstream,
        }
    }

    /// Returns a safe effective context when account-scoped model metadata is unavailable.
    pub fn effective_context_fallback(&self, model: &str) -> Option<u64> {
        match (self.runtime_id, model) {
            (RuntimeProviderId::KimiCode, "k3") => Some(262_144),
            _ => None,
        }
    }
}

impl ProviderAuthKind {
    pub fn env_var(self) -> Option<&'static str> {
        match self {
            Self::None | Self::OllamaDeviceKey { .. } => None,
            Self::ApiKey { env_var, .. }
            | Self::CodexOAuth { env_var, .. }
            | Self::GithubCopilotDevice { env_var, .. }
            | Self::XaiOAuth { env_var, .. }
            | Self::BearerCredential { env_var, .. }
            | Self::KimiOAuth { env_var, .. } => Some(env_var),
        }
    }

    pub fn account(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::ApiKey { account, .. }
            | Self::CodexOAuth { account, .. }
            | Self::GithubCopilotDevice { account, .. }
            | Self::XaiOAuth { account, .. }
            | Self::BearerCredential { account, .. }
            | Self::KimiOAuth { account, .. }
            | Self::OllamaDeviceKey { account, .. } => Some(account),
        }
    }

    /// User-facing guidance when this auth kind has no usable credentials.
    pub fn missing_message(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::ApiKey {
                missing_message, ..
            }
            | Self::CodexOAuth {
                missing_message, ..
            }
            | Self::GithubCopilotDevice {
                missing_message, ..
            }
            | Self::XaiOAuth {
                missing_message, ..
            }
            | Self::BearerCredential {
                missing_message, ..
            }
            | Self::KimiOAuth {
                missing_message, ..
            }
            | Self::OllamaDeviceKey {
                missing_message, ..
            } => Some(missing_message),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthMode {
    pub id: &'static str,
    pub login_label: &'static str,
    pub auth_kind: ProviderAuthKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderDescriptor {
    pub id: ProviderId,
    pub runtime_id: RuntimeProviderId,
    pub name: &'static str,
    pub display_name: &'static str,
    /// Default auth profile id for this provider.
    pub auth: &'static str,
    pub login_label: &'static str,
    pub auth_kind: ProviderAuthKind,
    /// Additional auth profiles that share this provider identity and API.
    pub extra_auth_modes: &'static [AuthMode],
    pub model_source: ProviderModelSource,
    pub model_refresh: Option<ProviderModelRefreshKind>,
    pub model_id_codec: ModelIdCodec,
    pub metadata_upstream: &'static str,
    pub catalog_reasoning: CatalogReasoningPolicy,
}

impl ProviderDescriptor {
    /// Default auth mode plus any extra modes registered on this provider.
    pub fn auth_modes(self) -> impl Iterator<Item = AuthMode> {
        std::iter::once(AuthMode {
            id: self.auth,
            login_label: self.login_label,
            auth_kind: self.auth_kind,
        })
        .chain(self.extra_auth_modes.iter().copied())
    }

    pub fn auth_mode(self, auth: &str) -> Option<AuthMode> {
        self.auth_modes().find(|mode| mode.id == auth)
    }
}

pub const PROVIDERS: &[ProviderDescriptor] = &[
    ProviderDescriptor {
        id: ProviderId::Ollama,
        runtime_id: RuntimeProviderId::Ollama,
        name: "ollama",
        display_name: "Ollama",
        auth: "none",
        login_label: "No authentication required",
        auth_kind: ProviderAuthKind::None,
        extra_auth_modes: &[],
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::OpenAiCompatible),
        model_id_codec: ModelIdCodec::Plain,
        metadata_upstream: "ollama",
        catalog_reasoning: CatalogReasoningPolicy::NotConfigurable,
    },
    ProviderDescriptor {
        id: ProviderId::OllamaCloud,
        runtime_id: RuntimeProviderId::OllamaCloud,
        name: "ollama-cloud",
        display_name: "Ollama Cloud",
        auth: "ollama-cloud-api-key",
        login_label: "Ollama Cloud API key",
        auth_kind: ProviderAuthKind::ApiKey {
            env_var: "OLLAMA_API_KEY",
            account: OLLAMA_CLOUD_API_KEY_ACCOUNT,
            entry_label: "Ollama Cloud API key",
            missing_message: "missing Ollama Cloud API key; run /login ollama-cloud in the TUI or set OLLAMA_API_KEY as a CI/dev override",
        },
        extra_auth_modes: &[AuthMode {
            id: "ollama-cloud-device",
            login_label: "Ollama Cloud device key",
            auth_kind: ProviderAuthKind::OllamaDeviceKey {
                account: OLLAMA_CLOUD_DEVICE_SESSION_ACCOUNT,
                missing_message: "missing Ollama Cloud device key; run /login ollama-cloud in the TUI and choose Device Key, or sign in with `ollama signin` so ~/.ollama/id_ed25519 is registered",
            },
        }],
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::OpenAiCompatible),
        model_id_codec: ModelIdCodec::Plain,
        metadata_upstream: "ollama",
        catalog_reasoning: CatalogReasoningPolicy::NotConfigurable,
    },
    ProviderDescriptor {
        id: ProviderId::OpenAi,
        runtime_id: RuntimeProviderId::OpenAi,
        name: "openai",
        display_name: "OpenAI",
        auth: "api-key",
        login_label: "OpenAI API key",
        auth_kind: ProviderAuthKind::ApiKey {
            env_var: "OPENAI_API_KEY",
            account: OPENAI_API_KEY_ACCOUNT,
            entry_label: "OpenAI API key",
            missing_message: "missing OpenAI API key; run /login openai in the TUI or set OPENAI_API_KEY as a CI/dev override",
        },
        extra_auth_modes: &[],
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::OpenAi),
        model_id_codec: ModelIdCodec::Plain,
        metadata_upstream: "openai",
        catalog_reasoning: CatalogReasoningPolicy::ExactAdvertised,
    },
    ProviderDescriptor {
        id: ProviderId::OpenAiCodex,
        runtime_id: RuntimeProviderId::OpenAi,
        name: "openai-codex",
        display_name: "OpenAI Codex",
        auth: "codex",
        login_label: "Codex OAuth",
        auth_kind: ProviderAuthKind::CodexOAuth {
            env_var: "CODEX_ACCESS_TOKEN",
            account: CODEX_TOKENS_ACCOUNT,
            missing_message: "missing Codex OAuth credentials; run /login openai-codex in the TUI or set CODEX_ACCESS_TOKEN as a CI/dev override",
        },
        extra_auth_modes: &[],
        model_source: ProviderModelSource::StaticCatalog,
        model_refresh: None,
        model_id_codec: ModelIdCodec::Plain,
        metadata_upstream: "openai",
        catalog_reasoning: CatalogReasoningPolicy::ExactAdvertised,
    },
    ProviderDescriptor {
        id: ProviderId::Anthropic,
        runtime_id: RuntimeProviderId::Anthropic,
        name: "anthropic",
        display_name: "Anthropic",
        auth: "anthropic-api-key",
        login_label: "Anthropic API key",
        auth_kind: ProviderAuthKind::ApiKey {
            env_var: "ANTHROPIC_API_KEY",
            account: ANTHROPIC_API_KEY_ACCOUNT,
            entry_label: "Anthropic API key",
            missing_message: "missing Anthropic API key; run /login anthropic in the TUI or set ANTHROPIC_API_KEY as a CI/dev override",
        },
        extra_auth_modes: &[],
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::Anthropic),
        model_id_codec: ModelIdCodec::Plain,
        metadata_upstream: "anthropic",
        catalog_reasoning: CatalogReasoningPolicy::Unknown,
    },
    ProviderDescriptor {
        id: ProviderId::Google,
        runtime_id: RuntimeProviderId::Google,
        name: "google",
        display_name: "Google Gemini",
        auth: "google-api-key",
        login_label: "Google Gemini API key",
        auth_kind: ProviderAuthKind::ApiKey {
            env_var: "GEMINI_API_KEY",
            account: GOOGLE_API_KEY_ACCOUNT,
            entry_label: "Google Gemini API key",
            missing_message: "missing Google Gemini API key; run /login google in the TUI or set GEMINI_API_KEY as a CI/dev override",
        },
        extra_auth_modes: &[],
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::Google),
        model_id_codec: ModelIdCodec::Plain,
        metadata_upstream: "google",
        catalog_reasoning: CatalogReasoningPolicy::ExactAdvertised,
    },
    ProviderDescriptor {
        id: ProviderId::GithubCopilot,
        runtime_id: RuntimeProviderId::GithubCopilot,
        name: "github-copilot",
        display_name: "GitHub Copilot",
        auth: "github-copilot",
        login_label: "GitHub Copilot device login",
        auth_kind: ProviderAuthKind::GithubCopilotDevice {
            env_var: "GITHUB_COPILOT_TOKEN",
            account: GITHUB_COPILOT_TOKENS_ACCOUNT,
            missing_message: "missing GitHub Copilot credentials; run /login github-copilot in the TUI or set GITHUB_COPILOT_TOKEN as a CI/dev override",
        },
        extra_auth_modes: &[],
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::GithubCopilot),
        model_id_codec: ModelIdCodec::Plain,
        metadata_upstream: "github-copilot",
        catalog_reasoning: CatalogReasoningPolicy::NotConfigurable,
    },
    ProviderDescriptor {
        id: ProviderId::Moonshot,
        runtime_id: RuntimeProviderId::Moonshot,
        name: "moonshot",
        display_name: "Moonshot AI",
        auth: "moonshot-api-key",
        login_label: "Moonshot API key",
        auth_kind: ProviderAuthKind::ApiKey {
            env_var: "MOONSHOT_API_KEY",
            account: MOONSHOT_API_KEY_ACCOUNT,
            entry_label: "Moonshot API key",
            missing_message: "missing Moonshot API key; run /login moonshot in the TUI or set MOONSHOT_API_KEY as a CI/dev override",
        },
        extra_auth_modes: &[],
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::OpenAiCompatible),
        model_id_codec: ModelIdCodec::Plain,
        metadata_upstream: "moonshotai",
        catalog_reasoning: CatalogReasoningPolicy::ExactAdvertised,
    },
    ProviderDescriptor {
        id: ProviderId::Poolside,
        runtime_id: RuntimeProviderId::Poolside,
        name: "poolside",
        display_name: "Poolside",
        auth: "poolside-api-key",
        login_label: "Poolside API key",
        auth_kind: ProviderAuthKind::ApiKey {
            env_var: "POOLSIDE_API_KEY",
            account: POOLSIDE_API_KEY_ACCOUNT,
            entry_label: "Poolside API key",
            missing_message: "missing Poolside API key; run /login poolside in the TUI or set POOLSIDE_API_KEY as a CI/dev override",
        },
        extra_auth_modes: &[],
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::OpenAiCompatible),
        model_id_codec: ModelIdCodec::ProviderPrefixed,
        metadata_upstream: "poolside",
        catalog_reasoning: CatalogReasoningPolicy::OffOrMax,
    },
    ProviderDescriptor {
        id: ProviderId::OpenRouter,
        runtime_id: RuntimeProviderId::OpenRouter,
        name: "openrouter",
        display_name: "OpenRouter",
        auth: "openrouter-api-key",
        login_label: "OpenRouter API key",
        auth_kind: ProviderAuthKind::ApiKey {
            env_var: "OPENROUTER_API_KEY",
            account: OPENROUTER_API_KEY_ACCOUNT,
            entry_label: "OpenRouter API key",
            missing_message: "missing OpenRouter API key; run /login openrouter in the TUI or set OPENROUTER_API_KEY as a CI/dev override",
        },
        extra_auth_modes: &[],
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::OpenAiCompatible),
        model_id_codec: ModelIdCodec::Plain,
        metadata_upstream: "openrouter",
        catalog_reasoning: CatalogReasoningPolicy::OffAsNone,
    },
    ProviderDescriptor {
        id: ProviderId::OpenRouterOAuth,
        runtime_id: RuntimeProviderId::OpenRouter,
        name: "openrouter-oauth",
        display_name: "OpenRouter",
        auth: "openrouter-oauth",
        login_label: "OpenRouter OAuth",
        auth_kind: ProviderAuthKind::BearerCredential {
            env_var: "OPENROUTER_API_KEY",
            account: OPENROUTER_OAUTH_KEY_ACCOUNT,
            missing_message: "missing OpenRouter OAuth credentials; run /login openrouter-oauth in the TUI or set OPENROUTER_API_KEY as a CI/dev override",
            acquisition: BearerCredentialAcquisition::BrowserOAuth(BrowserOAuthFlow::OpenRouter),
        },
        extra_auth_modes: &[],
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::OpenAiCompatible),
        model_id_codec: ModelIdCodec::Plain,
        metadata_upstream: "openrouter",
        catalog_reasoning: CatalogReasoningPolicy::OffAsNone,
    },
    ProviderDescriptor {
        id: ProviderId::KimiCode,
        runtime_id: RuntimeProviderId::KimiCode,
        name: "kimi-code",
        display_name: "Kimi Code",
        auth: "kimi-oauth",
        login_label: "Kimi Code OAuth",
        auth_kind: ProviderAuthKind::KimiOAuth {
            env_var: "KIMI_ACCESS_TOKEN",
            account: KIMI_TOKENS_ACCOUNT,
            missing_message: "missing Kimi OAuth credentials; run /login kimi-code or set KIMI_ACCESS_TOKEN as a CI/dev override",
        },
        extra_auth_modes: &[],
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::OpenAiCompatible),
        model_id_codec: ModelIdCodec::Plain,
        metadata_upstream: "moonshotai",
        catalog_reasoning: CatalogReasoningPolicy::OffByAdvertisedToggle,
    },
    ProviderDescriptor {
        id: ProviderId::Xai,
        runtime_id: RuntimeProviderId::Xai,
        name: "xai",
        display_name: "xAI",
        auth: "xai-api-key",
        login_label: "xAI API key",
        auth_kind: ProviderAuthKind::ApiKey {
            env_var: "XAI_API_KEY",
            account: XAI_API_KEY_ACCOUNT,
            entry_label: "xAI API key",
            missing_message: "missing xAI API key; run /login xai in the TUI or set XAI_API_KEY as a CI/dev override",
        },
        extra_auth_modes: &[],
        model_source: ProviderModelSource::StaticCatalog,
        model_refresh: None,
        model_id_codec: ModelIdCodec::Plain,
        metadata_upstream: "xai",
        catalog_reasoning: CatalogReasoningPolicy::OffByAdvertisedToggle,
    },
    ProviderDescriptor {
        id: ProviderId::XaiOAuth,
        runtime_id: RuntimeProviderId::Xai,
        name: "xai-oauth",
        display_name: "xAI",
        auth: "xai-oauth",
        login_label: "xAI OAuth",
        auth_kind: ProviderAuthKind::XaiOAuth {
            env_var: "XAI_ACCESS_TOKEN",
            account: XAI_TOKENS_ACCOUNT,
            missing_message: "missing xAI OAuth credentials; run /login xai-oauth in the TUI or set XAI_ACCESS_TOKEN as a CI/dev override",
        },
        extra_auth_modes: &[],
        model_source: ProviderModelSource::StaticCatalog,
        model_refresh: None,
        model_id_codec: ModelIdCodec::Plain,
        metadata_upstream: "xai",
        catalog_reasoning: CatalogReasoningPolicy::OffByAdvertisedToggle,
    },
];

pub fn providers() -> &'static [ProviderDescriptor] {
    PROVIDERS
}

/// Environment variable names used as provider credential overrides.
///
/// Derived from [`PROVIDERS`] auth kinds so newly registered provider credentials
/// are included automatically. Hosts should strip these from child process
/// environments by default, for example with
/// [`rho_sdk::ProcessEnvironment::inherit_except`].
pub fn credential_env_vars() -> &'static [&'static str] {
    use std::sync::OnceLock;

    static VARS: OnceLock<Vec<&'static str>> = OnceLock::new();
    VARS.get_or_init(|| {
        let mut vars: Vec<&'static str> = PROVIDERS
            .iter()
            .flat_map(|descriptor| descriptor.auth_modes())
            .filter_map(|mode| mode.auth_kind.env_var())
            .collect();
        vars.sort_unstable();
        vars.dedup();
        vars
    })
    .as_slice()
}

/// Auth profile names accepted by CLI `--auth` and config `auth`.
///
/// Derived from [`PROVIDERS`] so newly registered profiles are included
/// automatically. Keyless providers use `auth = "none"` and are omitted.
pub fn auth_profiles() -> &'static [&'static str] {
    use std::sync::OnceLock;

    static PROFILES: OnceLock<Vec<&'static str>> = OnceLock::new();
    PROFILES
        .get_or_init(|| {
            PROVIDERS
                .iter()
                .flat_map(|descriptor| descriptor.auth_modes())
                .map(|mode| mode.id)
                .filter(|auth| *auth != "none")
                .collect()
        })
        .as_slice()
}

pub fn provider_descriptor(provider: &str) -> Option<&'static ProviderDescriptor> {
    providers()
        .iter()
        .find(|descriptor| descriptor.name == provider)
}

/// Formats a provider-qualified model reference for user input and display.
pub fn model_reference(provider: &str, model: &str) -> String {
    format!("{provider}/{model}")
}

pub fn provider_descriptor_for_auth(auth: &str) -> Option<&'static ProviderDescriptor> {
    providers()
        .iter()
        .find(|descriptor| descriptor.auth_mode(auth).is_some())
}

/// Resolves an auth profile id to its provider and mode.
pub fn resolve_auth_mode(auth: &str) -> Option<(&'static ProviderDescriptor, AuthMode)> {
    let descriptor = provider_descriptor_for_auth(auth)?;
    let mode = descriptor.auth_mode(auth)?;
    Some((descriptor, mode))
}

/// Resolves a provider/auth pair to one registered provider identity.
///
/// - Auth modes registered on the named provider keep that provider name.
/// - Legacy dual-provider auth profiles (same runtime, different provider name)
///   still switch to the auth profile's provider when needed.
pub fn resolve_profile(
    provider_name: &str,
    auth: &str,
) -> Result<&'static ProviderDescriptor, ProfileResolutionError> {
    let provider = provider_descriptor(provider_name)
        .ok_or_else(|| ProfileResolutionError::UnknownProvider(provider_name.into()))?;
    if provider.auth_mode(auth).is_some() {
        return Ok(provider);
    }
    let auth_profile = provider_descriptor_for_auth(auth)
        .ok_or_else(|| ProfileResolutionError::UnknownAuth(auth.into()))?;
    if provider.runtime_id == auth_profile.runtime_id {
        Ok(auth_profile)
    } else {
        Ok(provider)
    }
}

/// Auth id to persist after [`resolve_profile`].
///
/// Keeps an in-provider extra auth mode when valid; otherwise uses the resolved
/// provider's default auth profile.
pub fn resolved_auth_id(provider: &ProviderDescriptor, requested_auth: &str) -> &'static str {
    provider
        .auth_mode(requested_auth)
        .map(|mode| mode.id)
        .unwrap_or(provider.auth)
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProfileResolutionError {
    #[error("unknown provider '{0}'")]
    UnknownProvider(String),
    #[error("unknown auth profile '{0}'")]
    UnknownAuth(String),
}

pub fn provider_descriptor_by_id(id: ProviderId) -> &'static ProviderDescriptor {
    providers()
        .iter()
        .find(|descriptor| descriptor.id == id)
        .expect("every provider ID must have a descriptor")
}

#[cfg(test)]
#[path = "provider_tests.rs"]
mod tests;
