//! Typed Claude stream-json protocol messages.
//!
//! Decode is two-step: parse NDJSON to [`serde_json::Value`], then classify by
//! top-level `type`. Known kinds become structured variants; unknown and
//! heartbeat frames stay explicit so schema drift is visible and control noise
//! stays silent.

use serde::Deserialize;
use serde_json::Value;

use super::{format::RawUsage, types::RateLimitInfo};

/// One decoded stream-json line.
#[derive(Debug)]
pub(super) enum ClaudeStreamMessage {
    Assistant(AssistantMessage),
    User(UserMessage),
    Result(ResultMessage),
    System(SystemMessage),
    RateLimit(RateLimitMessage),
    StreamEvent(StreamEventMessage),
    Error(ErrorMessage),
    /// Documented no-op frames: progress heartbeats and control channel.
    ProtocolControl,
    Unknown {
        kind: String,
    },
}

#[derive(Debug, Deserialize)]
pub(super) struct AssistantMessage {
    #[serde(default)]
    pub(super) session_id: Option<String>,
    #[serde(default)]
    pub(super) message: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub(super) struct UserMessage {
    #[serde(default)]
    pub(super) message: Option<Value>,
    #[serde(default)]
    pub(super) tool_use_id: Option<String>,
    #[serde(default)]
    pub(super) content: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ResultMessage {
    #[serde(default)]
    pub(super) subtype: Option<String>,
    #[serde(default)]
    pub(super) session_id: Option<String>,
    #[serde(default)]
    pub(super) is_error: Option<bool>,
    #[serde(default)]
    pub(super) result: Option<String>,
    #[serde(default)]
    pub(super) num_turns: Option<u64>,
    #[serde(default)]
    pub(super) total_cost_usd: Option<f64>,
    #[serde(default)]
    pub(super) stop_reason: Option<String>,
    #[serde(default)]
    pub(super) usage: Option<RawUsage>,
    #[serde(default, rename = "modelUsage")]
    pub(super) model_usage: Option<Value>,
    #[serde(default)]
    pub(super) permission_denials: Option<Vec<Value>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct SystemMessage {
    #[serde(default)]
    pub(super) subtype: Option<String>,
    #[serde(default)]
    pub(super) session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct RateLimitMessage {
    #[serde(default)]
    pub(super) rate_limit_info: Option<RateLimitInfo>,
}

#[derive(Debug, Deserialize)]
pub(super) struct StreamEventMessage {
    #[serde(default)]
    pub(super) event: Option<Value>,
    #[serde(default)]
    pub(super) content_block: Option<Value>,
    #[serde(default)]
    pub(super) delta: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ErrorMessage {
    #[serde(default)]
    pub(super) subtype: Option<String>,
    #[serde(default)]
    pub(super) session_id: Option<String>,
    #[serde(default)]
    pub(super) result: Option<String>,
}

/// Parse one NDJSON line into a typed protocol message.
pub(super) fn decode_line(line: &str) -> Result<ClaudeStreamMessage, serde_json::Error> {
    let value: Value = serde_json::from_str(line)?;
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Ok(match kind.as_str() {
        "assistant" => ClaudeStreamMessage::Assistant(serde_json::from_value(value)?),
        "user" => ClaudeStreamMessage::User(serde_json::from_value(value)?),
        "result" => ClaudeStreamMessage::Result(serde_json::from_value(value)?),
        "system" => ClaudeStreamMessage::System(serde_json::from_value(value)?),
        "rate_limit_event" => ClaudeStreamMessage::RateLimit(serde_json::from_value(value)?),
        "stream_event" => ClaudeStreamMessage::StreamEvent(serde_json::from_value(value)?),
        "error" => ClaudeStreamMessage::Error(serde_json::from_value(value)?),
        "tool_progress" | "status" | "keep_alive" | "control_request" | "control_response" => {
            ClaudeStreamMessage::ProtocolControl
        }
        other => ClaudeStreamMessage::Unknown {
            kind: other.to_string(),
        },
    })
}
