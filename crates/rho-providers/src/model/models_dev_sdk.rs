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
