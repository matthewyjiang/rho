pub(crate) mod http_error;
/// Shared streaming line decoder used by provider HTTP streams and local process
/// NDJSON (Claude CLI). Hidden from rustdoc; not a stable public product API.
#[doc(hidden)]
pub mod line_decoder;
pub(crate) mod stream_timeout;

pub use crate::model::{
    ContentBlock, ImageContent, Message, ModelError, ModelEvent, ModelRequest, ModelResponse,
    ModelUsage, ToolCall, ToolResult, ToolSpec,
};
