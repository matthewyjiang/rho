use crate::{
    model::ModelError,
    provider::{self, MissingCredential, ProviderAuthKind},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthMode {
    ApiKey,
    Codex,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderRuntime {
    OpenAi { auth_mode: AuthMode },
    Anthropic,
    GithubCopilot,
    Xai,
}

pub fn provider_runtime(provider: &str) -> Option<ProviderRuntime> {
    let descriptor = provider::provider_descriptor(provider)?;
    Some(match descriptor.id {
        provider::ProviderId::OpenAi => ProviderRuntime::OpenAi {
            auth_mode: AuthMode::ApiKey,
        },
        provider::ProviderId::OpenAiCodex => ProviderRuntime::OpenAi {
            auth_mode: AuthMode::Codex,
        },
        provider::ProviderId::Anthropic => ProviderRuntime::Anthropic,
        provider::ProviderId::GithubCopilot => ProviderRuntime::GithubCopilot,
        provider::ProviderId::Xai => ProviderRuntime::Xai,
    })
}

pub fn missing_credential_error(missing: MissingCredential) -> ModelError {
    match missing {
        MissingCredential::OpenAiApiKey => ModelError::MissingApiKey,
        MissingCredential::AnthropicApiKey => ModelError::MissingAnthropicApiKey,
    }
}

pub fn missing_credentials_error(provider_name: &str) -> ModelError {
    match provider::provider_descriptor(provider_name).map(|descriptor| descriptor.auth_kind) {
        Some(ProviderAuthKind::ApiKey { missing, .. }) => missing_credential_error(missing),
        Some(ProviderAuthKind::CodexOAuth { .. }) => ModelError::MissingCodexAuth,
        Some(ProviderAuthKind::GithubCopilotDevice { .. }) => ModelError::MissingGithubCopilotAuth,
        Some(ProviderAuthKind::XaiOAuth { .. }) => ModelError::MissingXaiAuth,
        None => ModelError::UnsupportedProvider(provider_name.to_string()),
    }
}
