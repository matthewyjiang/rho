mod types;

#[cfg(test)]
pub(crate) use super::openai_shared::stream::generation_output_tokens_event;
pub(crate) use super::openai_shared::{
    convert::{
        convert_openai_response, response_without_stream_context, to_openai_message_for_target,
        to_openai_tool,
    },
    stream::{line_decode_error, ChatStreamAccumulator, HiddenReasoningRisk},
    tool_calls::ChatToolCallPolicy,
};
pub(crate) use line_decode_error as invalid_stream_utf8;
pub(crate) use types::{
    ChatRequest, ChatResponse, ChatStreamOptions, ChatTemplateKwargs, OpenAiFunctionCall,
    OpenAiMessage, OpenAiReasoning, OpenAiThinking, OpenAiTool, OpenAiToolCall, OpenAiToolFunction,
};
