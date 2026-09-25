//! Recognizes provider rejections caused by an oversized request.
//!
//! Providers report context overflow as ordinary invalid-request errors, so
//! classification matches the stable codes and message fragments each wire
//! protocol uses. Matching is case-insensitive over the error code, message,
//! or bounded HTTP body.

/// Lowercase fragments that identify a context-window rejection.
///
/// Sources, by provider:
/// - OpenAI and OpenRouter: `context_length_exceeded` code,
///   "This model's maximum context length is ...".
/// - OpenAI Responses and Codex: "Your input exceeds the context window of this model".
/// - Anthropic: "prompt is too long: N tokens > M maximum".
/// - xAI: "This model's maximum prompt length is ...".
/// - vLLM: "This model's maximum context length is ...".
/// - llama.cpp server: `exceed_context_size_error`.
/// - Gemini: "The input token count (N) exceeds the maximum number of tokens allowed (M)".
const CONTEXT_OVERFLOW_FRAGMENTS: &[&str] = &[
    "context_length_exceeded",
    "maximum context length",
    "exceeds the context window",
    "prompt is too long",
    "maximum prompt length",
    "exceed_context_size_error",
    "exceeds the maximum number of tokens allowed",
];

/// True when provider error text describes a request larger than the model window.
pub(crate) fn mentions_context_overflow(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    CONTEXT_OVERFLOW_FRAGMENTS
        .iter()
        .any(|fragment| text.contains(fragment))
}
