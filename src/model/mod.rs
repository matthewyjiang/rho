pub mod catalog;
pub mod context;
mod contract;
pub mod favorites;
pub mod handoff;
pub mod image;
pub mod models_dev;
pub mod provider_models;
pub mod registry;

#[allow(unused_imports)]
pub(crate) use crate::providers::build_sdk_provider;
pub(crate) use crate::providers::{build_provider, UnavailableProvider};
pub use context::{ContextUsage, ContextUsageSource};
pub use contract::{
    AbortedAssistant, AssistantMessage, ContentBlock, DynModelProvider, ImageContent, Message,
    ModelError, ModelEvent, ModelIdentity, ModelProvider, ModelRequest, ModelResponse, ModelUsage,
    PartialToolCall, ProviderContextBlock, ToolCall, ToolResult, ToolSpec,
};
pub use handoff::HandoffReport;
pub use image::image_summary;
pub use models_dev::ModelMetadata;

impl From<crate::credentials::CredentialError> for ModelError {
    fn from(error: crate::credentials::CredentialError) -> Self {
        Self::credentials(error)
    }
}
