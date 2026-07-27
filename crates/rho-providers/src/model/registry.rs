use crate::{
    model::ModelError,
    provider::{self, ProviderAuthKind, RuntimeProviderId},
    providers::openai_compatible::OpenAiCompatibleDialect,
};

pub const OLLAMA_API_BASE: &str = "http://127.0.0.1:11434/v1";
pub const OLLAMA_CLOUD_API_BASE: &str = "https://ollama.com/v1";

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
    Some(match descriptor.runtime_id {
        RuntimeProviderId::Ollama => ProviderRuntime::OpenAiCompatible {
            dialect: OpenAiCompatibleDialect::Standard,
            default_api_base: OLLAMA_API_BASE,
        },
        RuntimeProviderId::OllamaCloud => ProviderRuntime::OpenAiCompatible {
            dialect: OpenAiCompatibleDialect::Standard,
            default_api_base: OLLAMA_CLOUD_API_BASE,
        },
        RuntimeProviderId::OpenAi => ProviderRuntime::OpenAi {
            auth_mode: match descriptor.auth_kind {
                ProviderAuthKind::ApiKey { .. } => AuthMode::ApiKey,
                ProviderAuthKind::CodexOAuth { .. } => AuthMode::Codex,
                ProviderAuthKind::None
                | ProviderAuthKind::GithubCopilotDevice { .. }
                | ProviderAuthKind::XaiOAuth { .. }
                | ProviderAuthKind::BearerCredential { .. }
                | ProviderAuthKind::KimiOAuth { .. }
                | ProviderAuthKind::OllamaDeviceKey { .. } => return None,
            },
        },
        RuntimeProviderId::Anthropic => ProviderRuntime::Anthropic,
        RuntimeProviderId::Google => ProviderRuntime::Google,
        RuntimeProviderId::GithubCopilot => ProviderRuntime::GithubCopilot,
        RuntimeProviderId::KimiCode => ProviderRuntime::OpenAiCompatible {
            dialect: OpenAiCompatibleDialect::KimiCode,
            default_api_base: "https://api.kimi.com/coding/v1",
        },
        RuntimeProviderId::Moonshot => ProviderRuntime::OpenAiCompatible {
            dialect: OpenAiCompatibleDialect::Moonshot,
            default_api_base: "https://api.moonshot.ai/v1",
        },
        RuntimeProviderId::Poolside => ProviderRuntime::OpenAiCompatible {
            dialect: OpenAiCompatibleDialect::Poolside,
            default_api_base: "https://inference.poolside.ai/v1",
        },
        RuntimeProviderId::OpenRouter => ProviderRuntime::OpenAiCompatible {
            dialect: OpenAiCompatibleDialect::OpenRouter,
            default_api_base: "https://openrouter.ai/api/v1",
        },
        RuntimeProviderId::Xai => ProviderRuntime::Xai {
            auth_mode: match descriptor.auth_kind {
                ProviderAuthKind::ApiKey { .. } => XaiAuthMode::ApiKey,
                ProviderAuthKind::XaiOAuth { .. } => XaiAuthMode::OAuth,
                ProviderAuthKind::None
                | ProviderAuthKind::CodexOAuth { .. }
                | ProviderAuthKind::GithubCopilotDevice { .. }
                | ProviderAuthKind::BearerCredential { .. }
                | ProviderAuthKind::KimiOAuth { .. }
                | ProviderAuthKind::OllamaDeviceKey { .. } => return None,
            },
        },
    })
}

pub fn missing_credential_error(message: &'static str) -> ModelError {
    ModelError::missing_credentials(message)
}

pub fn missing_credentials_error(provider_name: &str) -> ModelError {
    match provider::provider_descriptor(provider_name) {
        Some(descriptor) => match descriptor.auth_kind.missing_message() {
            Some(message) => ModelError::missing_credentials(message),
            None => ModelError::InvalidResponse(format!(
                "provider '{provider_name}' does not require credentials"
            )),
        },
        None => ModelError::UnsupportedProvider(provider_name.to_string()),
    }
}
