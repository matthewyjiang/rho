//! Chat-completions assistant `content` in either wire shape.
//!
//! OpenAI sends a plain string. Mistral reasoning models send a list of
//! chunks instead: `{"type": "text", "text": ...}` for answer text and
//! `{"type": "thinking", "thinking": [{"type": "text", "text": ...}]}` for
//! reasoning. A streamed delta can switch shape mid-response, and one list can
//! close the thinking and open the answer, so every chunk must be read.

use serde_json::Value;

/// One piece of assistant content, in wire order.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ContentPart<'a> {
    Text(&'a str),
    Thinking(String),
}

/// Splits `content` into non-empty text and thinking parts. Unknown chunk
/// types are skipped.
pub(crate) fn content_parts(content: &Value) -> Vec<ContentPart<'_>> {
    match content {
        Value::String(text) => non_empty_text(text).into_iter().collect(),
        Value::Array(chunks) => chunks.iter().filter_map(chunk_part).collect(),
        _ => Vec::new(),
    }
}

fn chunk_part(chunk: &Value) -> Option<ContentPart<'_>> {
    match chunk.get("type").and_then(Value::as_str)? {
        "text" => chunk
            .get("text")
            .and_then(Value::as_str)
            .and_then(non_empty_text),
        "thinking" => {
            let thinking = match chunk.get("thinking")? {
                Value::String(text) => text.clone(),
                Value::Array(parts) => parts
                    .iter()
                    .filter_map(|part| part.get("text").and_then(Value::as_str))
                    .collect(),
                _ => return None,
            };
            (!thinking.is_empty()).then_some(ContentPart::Thinking(thinking))
        }
        _ => None,
    }
}

fn non_empty_text(text: &str) -> Option<ContentPart<'_>> {
    (!text.is_empty()).then_some(ContentPart::Text(text))
}
