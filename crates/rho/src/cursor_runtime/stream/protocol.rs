//! Typed view of `cursor-agent --output-format stream-json` lines.
//!
//! Shapes were recorded from `cursor-agent 2026.08.25` (see `fixtures/`).
//! Unknown `type` / `subtype` pairs become [`CursorFrame::Unknown`] so schema
//! drift degrades to a notice rather than failing the run.

use serde::Deserialize;
use serde_json::Value;

/// One decoded stream-json line.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum CursorFrame {
    /// `{"type":"system","subtype":"init",...}`: session id, cwd, bound model.
    Init(InitFrame),
    /// `{"type":"user",...}`: echo of the prompt. Presentation ignores it.
    User,
    /// `{"type":"thinking","subtype":"delta","text":...}`.
    ThinkingDelta(String),
    /// `{"type":"thinking","subtype":"completed"}`.
    ThinkingCompleted,
    /// `{"type":"assistant","message":{"content":[{"type":"text","text":...}]}}`.
    ///
    /// Cursor emits incremental deltas, then repeats the whole segment as one
    /// cumulative frame at segment end. The mapper detects the snapshot by
    /// comparing its text to the deltas accumulated so far.
    Assistant(AssistantFrame),
    /// `{"type":"tool_call","subtype":"started"|"completed",...}`.
    ToolCall(ToolCallFrame),
    /// `{"type":"result",...}`: terminal usage and concatenated text.
    Result(ResultFrame),
    /// Any other `type`, or a known type with an unexpected subtype.
    Unknown {
        kind: String,
        subtype: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(super) struct InitFrame {
    #[serde(default)]
    pub(super) session_id: Option<String>,
    #[serde(default)]
    pub(super) cwd: Option<String>,
    /// Display name of the bound model (`"Composer 2.5"`), not the `--model` id.
    #[serde(default)]
    pub(super) model: Option<String>,
    #[serde(default, rename = "permissionMode")]
    pub(super) permission_mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct AssistantFrame {
    /// Concatenated `content[].text`.
    pub(super) text: String,
    /// True when the frame lacks `timestamp_ms`. Observed only on the final
    /// cumulative snapshot of a run; mid-turn snapshots (before a tool call)
    /// carry `model_call_id` instead. The mapper still confirms by equality.
    pub(super) has_timestamp: bool,
    pub(super) has_model_call_id: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolCallPhase {
    Started,
    Completed,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ToolCallFrame {
    pub(super) phase: ToolCallPhase,
    pub(super) call_id: String,
    /// Wire key naming the tool (`readToolCall`, `shellToolCall`, ...).
    pub(super) tool_key: String,
    /// `args` object from the wire body.
    pub(super) args: Option<Value>,
    /// `result` object from the wire body. Present on `completed` only.
    pub(super) result: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(super) struct ResultFrame {
    #[serde(default)]
    pub(super) subtype: Option<String>,
    #[serde(default)]
    pub(super) is_error: Option<bool>,
    #[serde(default)]
    pub(super) duration_ms: Option<u64>,
    /// Every assistant text segment of the run, `\n`-joined.
    #[serde(default)]
    pub(super) result: Option<String>,
    #[serde(default)]
    pub(super) session_id: Option<String>,
    #[serde(default)]
    pub(super) request_id: Option<String>,
    #[serde(default)]
    pub(super) usage: Option<RawUsage>,
}

/// Cursor's camelCase usage block on the terminal frame.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RawUsage {
    #[serde(default)]
    pub(super) input_tokens: Option<u64>,
    #[serde(default)]
    pub(super) output_tokens: Option<u64>,
    #[serde(default)]
    pub(super) cache_read_tokens: Option<u64>,
    #[serde(default)]
    pub(super) cache_write_tokens: Option<u64>,
}

impl RawUsage {
    pub(super) fn to_model(&self) -> rho_sdk::model::ModelUsage {
        rho_sdk::model::ModelUsage {
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_read_tokens: self.cache_read_tokens,
            cache_write_tokens: self.cache_write_tokens,
            total_tokens: None,
            context_window: None,
            cost_usd_micros: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(super) enum DecodeError {
    #[error("not a JSON object")]
    NotObject,
    #[error("missing `type`")]
    MissingType,
    #[error("{kind}: {detail}")]
    Shape { kind: String, detail: String },
}

/// Decode one already-parsed JSON line.
pub(super) fn decode_frame(value: Value) -> Result<CursorFrame, DecodeError> {
    let Value::Object(map) = value else {
        return Err(DecodeError::NotObject);
    };
    let kind = map
        .get("type")
        .and_then(Value::as_str)
        .ok_or(DecodeError::MissingType)?
        .to_string();
    let subtype = map
        .get("subtype")
        .and_then(Value::as_str)
        .map(str::to_string);
    let value = Value::Object(map);

    let shape = |detail: &str| DecodeError::Shape {
        kind: kind.clone(),
        detail: detail.to_string(),
    };

    Ok(match (kind.as_str(), subtype.as_deref()) {
        ("system", Some("init")) => CursorFrame::Init(
            serde_json::from_value(value).map_err(|error| shape(&error.to_string()))?,
        ),
        ("user", _) => CursorFrame::User,
        ("thinking", Some("delta")) => CursorFrame::ThinkingDelta(
            value
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ),
        ("thinking", Some("completed")) => CursorFrame::ThinkingCompleted,
        ("assistant", _) => CursorFrame::Assistant(AssistantFrame {
            text: content_text(value.get("message")),
            has_timestamp: value.get("timestamp_ms").is_some(),
            has_model_call_id: value.get("model_call_id").is_some(),
        }),
        ("tool_call", Some(phase @ ("started" | "completed"))) => {
            let phase = if phase == "started" {
                ToolCallPhase::Started
            } else {
                ToolCallPhase::Completed
            };
            let call_id = value
                .get("call_id")
                .and_then(Value::as_str)
                .ok_or_else(|| shape("missing call_id"))?
                .to_string();
            let body = value
                .get("tool_call")
                .and_then(Value::as_object)
                .ok_or_else(|| shape("missing tool_call object"))?;
            // The body is `{ "<tool>ToolCall": {args, result}, toolCallId, ... }`.
            let (tool_key, inner) = body
                .iter()
                .find(|(key, _)| key.ends_with("ToolCall"))
                .ok_or_else(|| shape("tool_call has no *ToolCall key"))?;
            CursorFrame::ToolCall(ToolCallFrame {
                phase,
                call_id,
                tool_key: tool_key.clone(),
                args: inner.get("args").cloned(),
                result: inner.get("result").cloned(),
            })
        }
        ("result", _) => CursorFrame::Result(
            serde_json::from_value(value).map_err(|error| shape(&error.to_string()))?,
        ),
        _ => CursorFrame::Unknown { kind, subtype },
    })
}

/// Join every `content[].text` of an assistant message.
fn content_text(message: Option<&Value>) -> String {
    message
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<String>()
        })
        .unwrap_or_default()
}
