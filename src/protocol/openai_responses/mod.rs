//! OpenAI Responses protocol boundary.

#[cfg(test)]
pub(crate) use super::openai_shared::stream::extract_sse_text;

pub(crate) use super::openai_shared::{
    convert::{
        codex_input_items, codex_reasoning_param, to_responses_lite_tool, to_responses_tool,
    },
    stream::{collect_codex_sse_response, handle_codex_sse_line, CodexSseResponse, CodexSseState},
};
