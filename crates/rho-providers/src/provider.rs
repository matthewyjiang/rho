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
pub const OPENROUTER_API_KEY_ACCOUNT: &str = "provider:openrouter:api-key";
pub const KIMI_TOKENS_ACCOUNT: &str = "provider:kimi-code:tokens";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProviderId {
    Ollama,
    OpenAi,
    OpenAiCodex,
    Anthropic,
    Google,
    GithubCopilot,
    Xai,
    XaiOAuth,
    Moonshot,
    OpenRouter,
    KimiCode,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderAuthKind {
    None,
    ApiKey {
        env_var: &'static str,
        account: &'static str,
        entry_label: &'static str,
        missing: MissingCredential,
    },
    CodexOAuth {
        env_var: &'static str,
        account: &'static str,
    },
    GithubCopilotDevice {
        env_var: &'static str,
        account: &'static str,
    },
    XaiOAuth {
        env_var: &'static str,
        account: &'static str,
    },
    KimiOAuth {
        env_var: &'static str,
        account: &'static str,
    },
}

impl ProviderDescriptor {
    /// Resolves a provider-facing model ID to its models.dev catalog ID.
    ///
    /// Provider model discovery remains authoritative. This only bridges model
    /// names when the provider API and metadata catalog use different IDs.
    pub fn metadata_model<'a>(&self, model: &'a str) -> &'a str {
        match (self.id, model) {
            (ProviderId::KimiCode, "k3") => "kimi-k3",
            (ProviderId::OpenRouter, model) => model
                .split_once('/')
                .map(|(_, upstream_model)| upstream_model)
                .unwrap_or(model),
            _ => model,
        }
    }

    /// Resolves an aggregator model ID to its models.dev provider.
    pub fn metadata_upstream_for_model<'a>(&self, model: &'a str) -> &'a str {
        match self.id {
            ProviderId::OpenRouter => model
                .split_once('/')
                .map(|(upstream, _)| upstream)
                .unwrap_or(self.metadata_upstream),
            _ => self.metadata_upstream,
        }
    }

    /// Returns a safe effective context when account-scoped model metadata is unavailable.
    pub fn effective_context_fallback(&self, model: &str) -> Option<u64> {
        match (self.id, model) {
            (ProviderId::KimiCode, "k3") => Some(262_144),
            _ => None,
        }
    }
}

impl ProviderAuthKind {
    pub fn env_var(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::ApiKey { env_var, .. }
            | Self::CodexOAuth { env_var, .. }
            | Self::GithubCopilotDevice { env_var, .. }
            | Self::XaiOAuth { env_var, .. }
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
            | Self::KimiOAuth { account, .. } => Some(account),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissingCredential {
    OpenAi,
    Anthropic,
    Google,
    Moonshot,
    OpenRouter,
    Xai,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderDescriptor {
    pub id: ProviderId,
    pub name: &'static str,
    pub display_name: &'static str,
    pub auth: &'static str,
    pub login_label: &'static str,
    pub auth_kind: ProviderAuthKind,
    pub model_source: ProviderModelSource,
    pub model_refresh: Option<ProviderModelRefreshKind>,
    pub metadata_upstream: &'static str,
    pub catalog_reasoning: CatalogReasoningPolicy,
}

pub const PROVIDERS: &[ProviderDescriptor] = &[
    ProviderDescriptor {
        id: ProviderId::Ollama,
        name: "ollama",
        display_name: "Ollama",
        auth: "none",
        login_label: "No authentication required",
        auth_kind: ProviderAuthKind::None,
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::OpenAiCompatible),
        metadata_upstream: "ollama",
        catalog_reasoning: CatalogReasoningPolicy::NotConfigurable,
    },
    ProviderDescriptor {
        id: ProviderId::OpenAi,
        name: "openai",
        display_name: "OpenAI",
        auth: "api-key",
        login_label: "OpenAI API key",
        auth_kind: ProviderAuthKind::ApiKey {
            env_var: "OPENAI_API_KEY",
            account: OPENAI_API_KEY_ACCOUNT,
            entry_label: "OpenAI API key",
            missing: MissingCredential::OpenAi,
        },
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::OpenAi),
        metadata_upstream: "openai",
        catalog_reasoning: CatalogReasoningPolicy::ExactAdvertised,
    },
    ProviderDescriptor {
        id: ProviderId::OpenAiCodex,
        name: "openai-codex",
        display_name: "OpenAI Codex",
        auth: "codex",
        login_label: "Codex OAuth",
        auth_kind: ProviderAuthKind::CodexOAuth {
            env_var: "CODEX_ACCESS_TOKEN",
            account: CODEX_TOKENS_ACCOUNT,
        },
        model_source: ProviderModelSource::StaticCatalog,
        model_refresh: None,
        metadata_upstream: "openai",
        catalog_reasoning: CatalogReasoningPolicy::ExactAdvertised,
    },
    ProviderDescriptor {
        id: ProviderId::Anthropic,
        name: "anthropic",
        display_name: "Anthropic",
        auth: "anthropic-api-key",
        login_label: "Anthropic API key",
        auth_kind: ProviderAuthKind::ApiKey {
            env_var: "ANTHROPIC_API_KEY",
            account: ANTHROPIC_API_KEY_ACCOUNT,
            entry_label: "Anthropic API key",
            missing: MissingCredential::Anthropic,
        },
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::Anthropic),
        metadata_upstream: "anthropic",
        catalog_reasoning: CatalogReasoningPolicy::Unknown,
    },
    ProviderDescriptor {
        id: ProviderId::Google,
        name: "google",
        display_name: "Google Gemini",
        auth: "google-api-key",
        login_label: "Google Gemini API key",
        auth_kind: ProviderAuthKind::ApiKey {
            env_var: "GEMINI_API_KEY",
            account: GOOGLE_API_KEY_ACCOUNT,
            entry_label: "Google Gemini API key",
            missing: MissingCredential::Google,
        },
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::Google),
        metadata_upstream: "google",
        catalog_reasoning: CatalogReasoningPolicy::ExactAdvertised,
    },
    ProviderDescriptor {
        id: ProviderId::GithubCopilot,
        name: "github-copilot",
        display_name: "GitHub Copilot",
        auth: "github-copilot",
        login_label: "GitHub Copilot device login",
        auth_kind: ProviderAuthKind::GithubCopilotDevice {
            env_var: "GITHUB_COPILOT_TOKEN",
            account: GITHUB_COPILOT_TOKENS_ACCOUNT,
        },
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::GithubCopilot),
        metadata_upstream: "github-copilot",
        catalog_reasoning: CatalogReasoningPolicy::NotConfigurable,
    },
    ProviderDescriptor {
        id: ProviderId::Moonshot,
        name: "moonshot",
        display_name: "Moonshot AI",
        auth: "moonshot-api-key",
        login_label: "Moonshot API key",
        auth_kind: ProviderAuthKind::ApiKey {
            env_var: "MOONSHOT_API_KEY",
            account: MOONSHOT_API_KEY_ACCOUNT,
            entry_label: "Moonshot API key",
            missing: MissingCredential::Moonshot,
        },
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::OpenAiCompatible),
        metadata_upstream: "moonshotai",
        catalog_reasoning: CatalogReasoningPolicy::ExactAdvertised,
    },
    ProviderDescriptor {
        id: ProviderId::OpenRouter,
        name: "openrouter",
        display_name: "OpenRouter",
        auth: "openrouter-api-key",
        login_label: "OpenRouter API key",
        auth_kind: ProviderAuthKind::ApiKey {
            env_var: "OPENROUTER_API_KEY",
            account: OPENROUTER_API_KEY_ACCOUNT,
            entry_label: "OpenRouter API key",
            missing: MissingCredential::OpenRouter,
        },
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::OpenAiCompatible),
        metadata_upstream: "openrouter",
        catalog_reasoning: CatalogReasoningPolicy::OffAsNone,
    },
    ProviderDescriptor {
        id: ProviderId::KimiCode,
        name: "kimi-code",
        display_name: "Kimi Code",
        auth: "kimi-oauth",
        login_label: "Kimi Code OAuth",
        auth_kind: ProviderAuthKind::KimiOAuth {
            env_var: "KIMI_ACCESS_TOKEN",
            account: KIMI_TOKENS_ACCOUNT,
        },
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::OpenAiCompatible),
        metadata_upstream: "moonshotai",
        catalog_reasoning: CatalogReasoningPolicy::OffByAdvertisedToggle,
    },
    ProviderDescriptor {
        id: ProviderId::Xai,
        name: "xai",
        display_name: "xAI",
        auth: "xai-api-key",
        login_label: "xAI API key",
        auth_kind: ProviderAuthKind::ApiKey {
            env_var: "XAI_API_KEY",
            account: XAI_API_KEY_ACCOUNT,
            entry_label: "xAI API key",
            missing: MissingCredential::Xai,
        },
        model_source: ProviderModelSource::StaticCatalog,
        model_refresh: None,
        metadata_upstream: "xai",
        catalog_reasoning: CatalogReasoningPolicy::OffByAdvertisedToggle,
    },
    ProviderDescriptor {
        id: ProviderId::XaiOAuth,
        name: "xai-oauth",
        display_name: "xAI",
        auth: "xai-oauth",
        login_label: "xAI OAuth",
        auth_kind: ProviderAuthKind::XaiOAuth {
            env_var: "XAI_ACCESS_TOKEN",
            account: XAI_TOKENS_ACCOUNT,
        },
        model_source: ProviderModelSource::StaticCatalog,
        model_refresh: None,
        metadata_upstream: "xai",
        catalog_reasoning: CatalogReasoningPolicy::OffByAdvertisedToggle,
    },
];

pub fn providers() -> &'static [ProviderDescriptor] {
    PROVIDERS
}

pub fn provider_descriptor(provider: &str) -> Option<&'static ProviderDescriptor> {
    providers()
        .iter()
        .find(|descriptor| descriptor.name == provider)
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
