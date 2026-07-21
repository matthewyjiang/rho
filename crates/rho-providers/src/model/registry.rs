use crate::{
    model::ModelError,
    provider::{self, MissingCredential, ProviderAuthKind},
    providers::openai_compatible::OpenAiCompatibleDialect,
};

pub const OLLAMA_API_BASE: &str = "http://127.0.0.1:11434/v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthMode {
    ApiKey,
    Codex,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XaiAuthMode {
    ApiKey,
    OAuth,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderRuntime {
    OpenAi {
        auth_mode: AuthMode,
    },
    OpenAiCompatible {
        dialect: OpenAiCompatibleDialect,
        default_api_base: &'static str,
    },
    Anthropic,
    Google,
    GithubCopilot,
    Xai {
        auth_mode: XaiAuthMode,
    },
}

pub fn provider_runtime(provider: &str) -> Option<ProviderRuntime> {
    let descriptor = provider::provider_descriptor(provider)?;
    Some(match descriptor.id {
        provider::ProviderId::Ollama => ProviderRuntime::OpenAiCompatible {
            dialect: OpenAiCompatibleDialect::Standard,
            default_api_base: OLLAMA_API_BASE,
        },
        provider::ProviderId::OpenAi => ProviderRuntime::OpenAi {
            auth_mode: AuthMode::ApiKey,
        },
        provider::ProviderId::OpenAiCodex => ProviderRuntime::OpenAi {
            auth_mode: AuthMode::Codex,
        },
        provider::ProviderId::Anthropic => ProviderRuntime::Anthropic,
        provider::ProviderId::Google => ProviderRuntime::Google,
        provider::ProviderId::GithubCopilot => ProviderRuntime::GithubCopilot,
        provider::ProviderId::KimiCode => ProviderRuntime::OpenAiCompatible {
            dialect: OpenAiCompatibleDialect::KimiCode,
            default_api_base: "https://api.kimi.com/coding/v1",
        },
        provider::ProviderId::Moonshot => ProviderRuntime::OpenAiCompatible {
            dialect: OpenAiCompatibleDialect::Moonshot,
            default_api_base: "https://api.moonshot.ai/v1",
        },
        provider::ProviderId::OpenRouter => ProviderRuntime::OpenAiCompatible {
            dialect: OpenAiCompatibleDialect::OpenRouter,
            default_api_base: "https://openrouter.ai/api/v1",
        },
        provider::ProviderId::Xai => ProviderRuntime::Xai {
            auth_mode: XaiAuthMode::ApiKey,
        },
        provider::ProviderId::XaiOAuth => ProviderRuntime::Xai {
            auth_mode: XaiAuthMode::OAuth,
        },
    })
}

pub fn missing_credential_error(missing: MissingCredential) -> ModelError {
    match missing {
        MissingCredential::OpenAi => ModelError::MissingApiKey,
        MissingCredential::Anthropic => ModelError::MissingAnthropicApiKey,
        MissingCredential::Google => ModelError::MissingGoogleApiKey,
        MissingCredential::Moonshot => ModelError::MissingMoonshotApiKey,
        MissingCredential::OpenRouter => ModelError::MissingOpenRouterApiKey,
        MissingCredential::Xai => ModelError::MissingXaiApiKey,
    }
}

pub fn missing_credentials_error(provider_name: &str) -> ModelError {
    match provider::provider_descriptor(provider_name).map(|descriptor| descriptor.auth_kind) {
        Some(ProviderAuthKind::None) => ModelError::InvalidResponse(format!(
            "provider '{provider_name}' does not require credentials"
        )),
        Some(ProviderAuthKind::ApiKey { missing, .. }) => missing_credential_error(missing),
        Some(ProviderAuthKind::CodexOAuth { .. }) => ModelError::MissingCodexAuth,
        Some(ProviderAuthKind::GithubCopilotDevice { .. }) => ModelError::MissingGithubCopilotAuth,
        Some(ProviderAuthKind::XaiOAuth { .. }) => ModelError::MissingXaiAuth,
        Some(ProviderAuthKind::KimiOAuth { .. }) => ModelError::MissingKimiAuth,
        None => ModelError::UnsupportedProvider(provider_name.to_string()),
    }
}
