//! The JSON a hook program writes back.
//!
//! Handler stdout is untrusted input. It is read under a byte bound, parsed
//! strictly, and any failure to produce a valid decision is treated as a denial
//! by the blocking path.

use serde::Deserialize;

use super::config::HOOKS_SCHEMA_VERSION;

/// Largest decision document the runtime will read from a handler.
pub const MAX_DECISION_BYTES: usize = 8 * 1024;

/// What a `before_tool_use` handler asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HandlerDecision {
    Continue,
    Deny { reason: String },
}

/// Longest denial reason surfaced to the model and the user.
const MAX_REASON_BYTES: usize = 1024;

#[derive(Debug, Deserialize)]
struct RawDecision {
    version: u32,
    decision: String,
    #[serde(default)]
    reason: Option<String>,
}

/// Why handler output could not be turned into a decision.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DecisionError {
    #[error("handler wrote no decision on stdout")]
    Empty,
    #[error("handler decision is not valid JSON: {0}")]
    Malformed(String),
    #[error(
        "handler decision reports schema version {found}; this build speaks {HOOKS_SCHEMA_VERSION}"
    )]
    SchemaMismatch { found: u32 },
    #[error("handler decision '{0}' is not 'continue' or 'deny'")]
    UnknownDecision(String),
    #[error("handler denied without a reason")]
    MissingReason,
    #[error("handler wrote more than {MAX_DECISION_BYTES} bytes on stdout")]
    TooLarge,
}

/// Parses one handler decision.
///
/// Unknown keys are ignored so a newer handler can add fields without breaking
/// an older Rho. An unknown schema version is not ignored: a handler speaking a
/// protocol this build does not understand must not be read as consent.
pub fn parse_decision(stdout: &[u8]) -> Result<HandlerDecision, DecisionError> {
    if stdout.len() > MAX_DECISION_BYTES {
        return Err(DecisionError::TooLarge);
    }
    let text = String::from_utf8_lossy(stdout);
    if text.trim().is_empty() {
        return Err(DecisionError::Empty);
    }
    let raw: RawDecision = serde_json::from_str(text.trim())
        .map_err(|error| DecisionError::Malformed(error.to_string()))?;
    if raw.version != HOOKS_SCHEMA_VERSION {
        return Err(DecisionError::SchemaMismatch { found: raw.version });
    }
    match raw.decision.as_str() {
        "continue" => Ok(HandlerDecision::Continue),
        "deny" => {
            let reason = raw
                .reason
                .map(|reason| reason.trim().to_owned())
                .filter(|reason| !reason.is_empty())
                .ok_or(DecisionError::MissingReason)?;
            Ok(HandlerDecision::Deny {
                reason: truncate_on_char_boundary(reason, MAX_REASON_BYTES),
            })
        }
        other => Err(DecisionError::UnknownDecision(other.to_owned())),
    }
}

fn truncate_on_char_boundary(mut value: String, limit: usize) -> String {
    if value.len() <= limit {
        return value;
    }
    let mut boundary = limit;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
    value
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;
