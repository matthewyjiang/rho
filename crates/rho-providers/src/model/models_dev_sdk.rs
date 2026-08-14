use serde_json::Value;

/// HTTP adapter implied by a models.dev AI SDK package name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogSdkAdapter {
    OpenAiCompatible,
    OpenAiResponses,
    AnthropicMessages,
}

impl CatalogSdkAdapter {
    pub fn from_sdk_package(package: Option<&str>) -> Self {
        match package {
            Some("@ai-sdk/openai") => Self::OpenAiResponses,
            Some("@ai-sdk/anthropic") => Self::AnthropicMessages,
            _ => Self::OpenAiCompatible,
        }
    }
}

/// models.dev provider `npm`, overridden by per-model `provider.npm` or `npm`.
pub(super) fn resolved_sdk_package(provider: Option<&Value>, model: &Value) -> Option<String> {
    model
        .get("provider")
        .and_then(|provider| provider.get("npm"))
        .and_then(Value::as_str)
        .or_else(|| model.get("npm").and_then(Value::as_str))
        .or_else(|| {
            provider
                .and_then(|provider| provider.get("npm"))
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .filter(|package| !package.is_empty())
        .map(str::to_string)
}
