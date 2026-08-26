pub mod catalog;
pub mod context;
mod contract;
pub mod display_name;
pub mod favorites;
pub mod handoff;
pub mod image;
pub(crate) mod inclusive_prompt;
pub mod models_dev;
pub mod provider_models;
mod reasoning_capabilities;
pub mod registry;
mod transport;

pub use crate::providers::UnavailableProvider;
pub use context::{ContextUsage, ContextUsageSource};
pub use contract::AbortedAssistant;
pub use contract::{
    AssistantMessage, ContentBlock, ImageContent, Message, ModelError, ModelEvent, ModelIdentity,
    ModelRequest, ModelResponse, ModelUsage, PartialToolCall, ProviderContextBlock,
    ProviderReportedErrorKind, ToolCall, ToolResult, ToolSpec,
};
pub use display_name::{
    ensure_model_catalog_names, model_display_name, model_reference_with_display_name,
};
pub use image::image_summary;
pub use models_dev::{
    ensure_models_dev_catalog, force_refresh_models_dev_catalog, CatalogLookupMiss, ModelMetadata,
};
pub use reasoning_capabilities::{
    ReasoningCapabilities, ReasoningLevelSet, ReasoningRequestSource, ReasoningResolution,
};
pub use transport::{TransportError, TransportFailureKind};

impl From<crate::credentials::CredentialError> for ModelError {
    fn from(error: crate::credentials::CredentialError) -> Self {
        Self::credentials(error)
    }
}

impl From<reqwest::Error> for ModelError {
    fn from(error: reqwest::Error) -> Self {
        Self::from_reqwest(error)
    }
}
