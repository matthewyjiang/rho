//! Typed Claude `stream_event` payloads.
//!
//! Nested `event` / `content_block` / `delta` values stay raw on the envelope
//! until decode, so top-level and nested shapes can share one fallback path.

use serde_json::Value;

use super::presentation::ContentBlockKind;
use super::protocol::StreamEventMessage;

/// One decoded partial-stream event from a `stream_event` envelope.
#[derive(Debug)]
pub(super) enum StreamEventPayload {
    MessageStart {
        message_id: Option<String>,
    },
    ContentBlockStart {
        /// Embedded `message.id` when Claude nested it on this event.
        message_id: Option<String>,
        index: Option<usize>,
        block: ContentBlockStart,
    },
    ContentBlockDelta {
        /// Embedded `message.id` when Claude nested it on this event.
        message_id: Option<String>,
        index: Option<usize>,
        delta: ContentDelta,
    },
    ContentBlockStop {
        /// Real stream index when Claude provided one (unused for presentation).
        #[allow(dead_code)]
        index: Option<usize>,
    },
    /// Stop-reason / usage metadata; presentation ignores it.
    MessageDelta,
    MessageStop,
    Unknown {
        kind: String,
    },
}

/// Body of a `content_block_start` event.
#[derive(Debug)]
pub(super) enum ContentBlockStart {
    Text {
        text: String,
    },
    Thinking {
        text: String,
    },
    ToolUse {
        id: Option<String>,
        name: Option<String>,
        input: Option<Value>,
    },
    Other {
        type_name: String,
        raw: Value,
    },
}

impl ContentBlockStart {
    pub(super) fn kind(&self) -> ContentBlockKind {
        match self {
            Self::Text { .. } => ContentBlockKind::Text,
            Self::Thinking { .. } => ContentBlockKind::Reasoning,
            Self::ToolUse { .. } => ContentBlockKind::Tool,
            Self::Other { .. } => ContentBlockKind::Other,
        }
    }

    /// Rebuild a JSON object for tool presentation helpers that still take
    /// `Value` (name / id / input rendering).
    pub(super) fn tool_block_value(&self) -> Value {
        match self {
            Self::ToolUse { id, name, input } => {
                let mut object = serde_json::Map::new();
                object.insert("type".into(), Value::String("tool_use".into()));
                if let Some(id) = id {
                    object.insert("id".into(), Value::String(id.clone()));
                }
                if let Some(name) = name {
                    object.insert("name".into(), Value::String(name.clone()));
                }
                if let Some(input) = input {
                    object.insert("input".into(), input.clone());
                }
                Value::Object(object)
            }
            Self::Text { text } => serde_json::json!({ "type": "text", "text": text }),
            Self::Thinking { text } => {
                serde_json::json!({ "type": "thinking", "thinking": text })
            }
            Self::Other { type_name, raw } => {
                let mut object = match raw {
                    Value::Object(map) => map.clone(),
                    _ => serde_json::Map::new(),
                };
                object
                    .entry("type")
                    .or_insert_with(|| Value::String(type_name.clone()));
                Value::Object(object)
            }
        }
    }
}

/// Body of a `content_block_delta` event.
#[derive(Debug)]
pub(super) enum ContentDelta {
    Text {
        text: String,
    },
    Thinking {
        text: String,
    },
    /// Tool-input JSON fragments; presentation does not surface them.
    InputJson {
        #[allow(dead_code)]
        partial_json: String,
    },
    /// Signature fragments; presentation ignores them.
    Signature,
    Other {
        type_name: String,
    },
}

/// Decode nested stream-event fields into a typed payload.
///
/// Claude may put `content_block` / `delta` on the envelope and/or under
/// `event`; envelope fields win only as fallbacks when the nested event omits
/// them (same order as the previous stringly mapper).
pub(super) fn decode_stream_event(message: StreamEventMessage) -> Option<StreamEventPayload> {
    let event = message.event?;
    let kind = event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let index = event
        .get("index")
        .and_then(Value::as_u64)
        .map(|index| index as usize);
    let message_id = embedded_message_id(&event);

    Some(match kind.as_str() {
        "message_start" => StreamEventPayload::MessageStart { message_id },
        "content_block_start" => {
            let raw = event
                .get("content_block")
                .cloned()
                .or(message.content_block)
                .unwrap_or(Value::Null);
            StreamEventPayload::ContentBlockStart {
                message_id,
                index,
                block: decode_content_block_start(raw),
            }
        }
        "content_block_delta" => {
            let raw = match event.get("delta").cloned().or(message.delta) {
                Some(raw) => raw,
                None => {
                    return Some(StreamEventPayload::ContentBlockDelta {
                        message_id,
                        index,
                        delta: ContentDelta::Other {
                            type_name: String::new(),
                        },
                    });
                }
            };
            StreamEventPayload::ContentBlockDelta {
                message_id,
                index,
                delta: decode_content_delta(raw),
            }
        }
        "content_block_stop" => StreamEventPayload::ContentBlockStop { index },
        "message_delta" => StreamEventPayload::MessageDelta,
        "message_stop" => StreamEventPayload::MessageStop,
        other if !other.is_empty() => StreamEventPayload::Unknown {
            kind: other.to_string(),
        },
        _ => StreamEventPayload::Unknown {
            kind: String::new(),
        },
    })
}

fn decode_content_block_start(raw: Value) -> ContentBlockStart {
    let type_name = raw
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    match type_name.as_str() {
        "text" => ContentBlockStart::Text {
            text: raw
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        },
        "thinking" => ContentBlockStart::Thinking {
            text: raw
                .get("thinking")
                .or_else(|| raw.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        },
        "tool_use" => ContentBlockStart::ToolUse {
            id: raw
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|id| !id.is_empty()),
            name: raw
                .get("name")
                .and_then(Value::as_str)
                .or_else(|| raw.get("tool_name").and_then(Value::as_str))
                .map(str::to_string),
            input: raw.get("input").cloned(),
        },
        other => ContentBlockStart::Other {
            type_name: other.to_string(),
            raw,
        },
    }
}

fn decode_content_delta(raw: Value) -> ContentDelta {
    let type_name = raw
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    match type_name.as_str() {
        "text_delta" => ContentDelta::Text {
            text: raw
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        },
        "thinking_delta" | "reasoning_delta" => ContentDelta::Thinking {
            text: raw
                .get("thinking")
                .or_else(|| raw.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        },
        "input_json_delta" => ContentDelta::InputJson {
            partial_json: raw
                .get("partial_json")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        },
        "signature_delta" => ContentDelta::Signature,
        other => ContentDelta::Other {
            type_name: other.to_string(),
        },
    }
}

fn embedded_message_id(event: &Value) -> Option<String> {
    event
        .get("message")
        .and_then(|value| value.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}
