pub mod catalog;
pub mod context;
mod contract;
pub mod favorites;
pub mod handoff;
pub mod image;
pub mod models_dev;
pub mod provider_models;
mod reasoning_capabilities;
pub mod registry;

pub use crate::providers::UnavailableProvider;
pub use context::{ContextUsage, ContextUsageSource};
pub use contract::AbortedAssistant;
pub use contract::{
    AssistantMessage, ContentBlock, ImageContent, Message, ModelError, ModelEvent, ModelIdentity,
    ModelRequest, ModelResponse, ModelUsage, PartialToolCall, ProviderContextBlock,
    ProviderReportedErrorKind, ToolCall, ToolResult, ToolSpec,
};
pub use image::image_summary;
pub use models_dev::ModelMetadata;
pub use reasoning_capabilities::{
    ReasoningCapabilities, ReasoningLevelSet, ReasoningRequestSource, ReasoningResolution,
};

impl From<crate::credentials::CredentialError> for ModelError {
    fn from(error: crate::credentials::CredentialError) -> Self {
        Self::credentials(error)
    }
}
