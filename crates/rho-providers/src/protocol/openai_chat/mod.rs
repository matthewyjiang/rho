mod types;

pub(crate) use super::openai_shared::{
    convert::{
        convert_openai_response, response_without_stream_context, to_openai_message_for_target,
        to_openai_tool,
    },
    stream::{line_decode_error, ChatStreamAccumulator},
    tool_calls::ChatToolCallPolicy,
    usage::HiddenReasoningRisk,
};
pub(crate) use line_decode_error as invalid_stream_utf8;
pub(crate) use types::{
    ChatRequest, ChatResponse, ChatStreamOptions, ChatTemplateKwargs, OpenAiFunctionCall,
    OpenAiMessage, OpenAiReasoning, OpenAiThinking, OpenAiTool, OpenAiToolCall, OpenAiToolFunction,
};
