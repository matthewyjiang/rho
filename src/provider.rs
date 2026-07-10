//! Stable provider identity and metadata shared across credential, catalog, and runtime layers.
//!
//! This module intentionally contains no credential-store or model-runtime behavior. Provider
//! adapters and `ModelError` mappings belong in the model runtime.

pub const OPENAI_API_KEY_ACCOUNT: &str = "provider:openai:api-key";
pub const ANTHROPIC_API_KEY_ACCOUNT: &str = "provider:anthropic:api-key";
pub const CODEX_TOKENS_ACCOUNT: &str = "provider:openai-codex:tokens";
pub const GITHUB_COPILOT_TOKENS_ACCOUNT: &str = "provider:github-copilot:tokens";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProviderId {
    OpenAi,
    OpenAiCodex,
    Anthropic,
    GithubCopilot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderModelSource {
    StaticCatalog,
    CachedProviderModels,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderModelRefreshKind {
    OpenAi,
    Anthropic,
    GithubCopilot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderAuthKind {
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
}

impl ProviderAuthKind {
    pub fn env_var(self) -> &'static str {
        match self {
            Self::ApiKey { env_var, .. }
            | Self::CodexOAuth { env_var, .. }
            | Self::GithubCopilotDevice { env_var, .. } => env_var,
        }
    }

    pub fn account(self) -> &'static str {
        match self {
            Self::ApiKey { account, .. }
            | Self::CodexOAuth { account, .. }
            | Self::GithubCopilotDevice { account, .. } => account,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissingCredential {
    OpenAiApiKey,
    AnthropicApiKey,
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
}

pub const PROVIDERS: &[ProviderDescriptor] = &[
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
            missing: MissingCredential::OpenAiApiKey,
        },
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::OpenAi),
        metadata_upstream: "openai",
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
            missing: MissingCredential::AnthropicApiKey,
        },
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::Anthropic),
        metadata_upstream: "anthropic",
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
