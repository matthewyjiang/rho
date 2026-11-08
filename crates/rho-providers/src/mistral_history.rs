//! Mistral chat history rewrites applied after the shared OpenAI conversion.
//!
//! Mistral rejects undeclared request fields with `422 extra_forbidden`, and
//! assistant messages do not declare `reasoning_content`. Same-model reasoning
//! is replayed as a leading `thinking` content chunk instead, which is the
//! shape Mistral streams it in and asks clients to send back.

use serde_json::{json, Value};

use crate::protocol::openai_chat::OpenAiMessage;

/// Moves replayed reasoning in `messages` into thinking content chunks.
pub(super) fn normalize(messages: &mut [OpenAiMessage]) {
    for message in messages {
        if let Some(reasoning) = message.reasoning_content.take() {
            message.content = Some(with_thinking_chunk(reasoning, message.content.take()));
        }
    }
}

/// Prepends a `thinking` chunk to assistant content, keeping any text as a
/// trailing `text` chunk.
fn with_thinking_chunk(reasoning: String, content: Option<Value>) -> Value {
    let mut chunks = vec![json!({
        "type": "thinking",
        "thinking": [{ "type": "text", "text": reasoning }],
    })];
    match content {
        Some(Value::String(text)) if !text.is_empty() => {
            chunks.push(json!({ "type": "text", "text": text }));
        }
        Some(Value::Array(parts)) => chunks.extend(parts),
        Some(_) | None => {}
    }
    Value::Array(chunks)
}

#[cfg(test)]
#[path = "mistral_history_tests.rs"]
mod tests;
