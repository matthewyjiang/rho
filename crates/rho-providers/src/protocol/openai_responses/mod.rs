//! OpenAI Responses protocol boundary.

#[cfg(test)]
pub(crate) use super::openai_shared::stream::extract_sse_text;

pub(crate) use super::openai_shared::{
    codex_sse::{
        collect_codex_sse_response, handle_codex_sse_line, handle_codex_sse_value,
        is_codex_turn_complete, CodexSseResponse, CodexSseState, CodexTransport,
    },
    compact::{parse_compact_response, retained_system_messages, CompactUserRetention},
    convert::{
        codex_input_items, codex_input_items_for_target, codex_reasoning_param,
        lower_codex_history_message, to_responses_lite_tool, to_responses_tool, ToolStrictness,
    },
};
