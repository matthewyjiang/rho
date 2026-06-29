use crate::model::{AuthMode, ModelError};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProviderId {
    OpenAi,
    OpenAiCodex,
    Anthropic,
    GithubCopilot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderRuntime {
    OpenAi { auth_mode: AuthMode },
    Anthropic,
    GithubCopilot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderModelSource {
    StaticCatalog,
    CachedProviderModels,
    CachedProviderModelsWithStaticFallback,
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
    pub runtime: ProviderRuntime,
}

const PROVIDER_IDS: &[&str] = &["openai", "openai-codex", "anthropic", "github-copilot"];

pub const PROVIDERS: &[ProviderDescriptor] = &[
    ProviderDescriptor {
        id: ProviderId::OpenAi,
        name: "openai",
        display_name: "OpenAI",
        auth: "api-key",
        login_label: "OpenAI API key",
        auth_kind: ProviderAuthKind::ApiKey {
            env_var: "OPENAI_API_KEY",
            account: "provider:openai:api-key",
            entry_label: "OpenAI API key",
            missing: MissingCredential::OpenAiApiKey,
        },
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::OpenAi),
        metadata_upstream: "openai",
        runtime: ProviderRuntime::OpenAi {
            auth_mode: AuthMode::ApiKey,
        },
    },
    ProviderDescriptor {
        id: ProviderId::OpenAiCodex,
        name: "openai-codex",
        display_name: "OpenAI Codex",
        auth: "codex",
        login_label: "Codex OAuth",
        auth_kind: ProviderAuthKind::CodexOAuth {
            env_var: "CODEX_ACCESS_TOKEN",
            account: "provider:openai-codex:tokens",
        },
        model_source: ProviderModelSource::StaticCatalog,
        model_refresh: None,
        metadata_upstream: "openai",
        runtime: ProviderRuntime::OpenAi {
            auth_mode: AuthMode::Codex,
        },
    },
    ProviderDescriptor {
        id: ProviderId::Anthropic,
        name: "anthropic",
        display_name: "Anthropic",
        auth: "anthropic-api-key",
        login_label: "Anthropic API key",
        auth_kind: ProviderAuthKind::ApiKey {
            env_var: "ANTHROPIC_API_KEY",
            account: "provider:anthropic:api-key",
            entry_label: "Anthropic API key",
            missing: MissingCredential::AnthropicApiKey,
        },
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::Anthropic),
        metadata_upstream: "anthropic",
        runtime: ProviderRuntime::Anthropic,
    },
    ProviderDescriptor {
        id: ProviderId::GithubCopilot,
        name: "github-copilot",
        display_name: "GitHub Copilot",
        auth: "github-copilot",
        login_label: "GitHub Copilot device login",
        auth_kind: ProviderAuthKind::GithubCopilotDevice {
            env_var: "GITHUB_COPILOT_TOKEN",
            account: "provider:github-copilot:tokens",
        },
        model_source: ProviderModelSource::CachedProviderModelsWithStaticFallback,
        model_refresh: Some(ProviderModelRefreshKind::GithubCopilot),
        metadata_upstream: "github-copilot",
        runtime: ProviderRuntime::GithubCopilot,
    },
];

pub fn provider_ids() -> &'static [&'static str] {
    PROVIDER_IDS
}

pub fn providers() -> &'static [ProviderDescriptor] {
    PROVIDERS
}

pub fn provider_descriptor(provider: &str) -> Option<&'static ProviderDescriptor> {
    providers()
        .iter()
        .find(|descriptor| descriptor.name == provider)
}

pub fn missing_credential_error(missing: MissingCredential) -> ModelError {
    match missing {
        MissingCredential::OpenAiApiKey => ModelError::MissingApiKey,
        MissingCredential::AnthropicApiKey => ModelError::MissingAnthropicApiKey,
    }
}

pub fn missing_credentials_error(provider: &str) -> ModelError {
    match provider_descriptor(provider).map(|descriptor| descriptor.auth_kind) {
        Some(ProviderAuthKind::ApiKey { missing, .. }) => missing_credential_error(missing),
        Some(ProviderAuthKind::CodexOAuth { .. }) => ModelError::MissingCodexAuth,
        Some(ProviderAuthKind::GithubCopilotDevice { .. }) => ModelError::MissingGithubCopilotAuth,
        None => ModelError::UnsupportedProvider(provider.to_string()),
    }
}
