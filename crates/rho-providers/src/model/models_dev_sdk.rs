/// HTTP adapter implied by a models.dev AI SDK package name.
///
/// # Next major
///
/// NEXT_MAJOR(rho-providers): mark CatalogSdkAdapter `#[non_exhaustive]` so new adapters are minor-compatible.
///
/// Adapters land in minor releases, as `GeminiGenerateContent` did; an
/// exhaustive match outside this crate breaks on each one until then.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
