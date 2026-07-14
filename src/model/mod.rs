pub mod catalog;
pub mod context;
pub mod favorites;
pub mod github_copilot;
pub mod image;
pub mod models_dev;
pub mod openai;
pub mod provider;
pub mod provider_models;
pub mod xai;

pub mod registry;

pub use crate::provider_backend::{
    AbortedAssistant, AnthropicProvider, ContentBlock, DynModelProvider, ImageContent, Message,
    ModelError, ModelEvent, ModelProvider, ModelRequest, ModelResponse, ModelUsage,
    PartialToolCall,
};
pub use context::{ContextUsage, ContextUsageSource};
pub use github_copilot::GitHubCopilotProvider;
pub use image::image_summary;
pub use models_dev::ModelMetadata;
pub use openai::OpenAiProvider;
pub use provider::{build_provider, UnavailableProvider};
pub use xai::XaiProvider;

impl From<crate::credentials::CredentialError> for ModelError {
    fn from(error: crate::credentials::CredentialError) -> Self {
        Self::credentials(error)
    }
}
