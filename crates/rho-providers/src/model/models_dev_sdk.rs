/// HTTP adapter implied by a models.dev AI SDK package name.
///
/// Non-exhaustive: new adapters land in minor releases.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CatalogSdkAdapter {
    OpenAiCompatible,
    OpenAiResponses,
    AnthropicMessages,
    GeminiGenerateContent,
}

impl CatalogSdkAdapter {
    pub fn from_sdk_package(package: Option<&str>) -> Self {
        match package {
            Some("@ai-sdk/openai") => Self::OpenAiResponses,
            Some("@ai-sdk/anthropic") => Self::AnthropicMessages,
            Some("@ai-sdk/google") => Self::GeminiGenerateContent,
            _ => Self::OpenAiCompatible,
        }
    }
}
