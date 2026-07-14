pub mod catalog;
pub mod context;
mod contract;
pub mod favorites;
pub mod image;
pub mod models_dev;
pub mod provider_models;
pub mod registry;

pub(crate) use crate::providers::{build_provider, UnavailableProvider};
pub use context::{ContextUsage, ContextUsageSource};
pub use contract::{
    AbortedAssistant, ContentBlock, DynModelProvider, ImageContent, Message, ModelError,
    ModelEvent, ModelProvider, ModelRequest, ModelResponse, ModelUsage, PartialToolCall, ToolCall,
    ToolResult, ToolSpec,
};
pub use image::image_summary;
pub use models_dev::ModelMetadata;

impl From<crate::credentials::CredentialError> for ModelError {
    fn from(error: crate::credentials::CredentialError) -> Self {
        Self::credentials(error)
    }
}
