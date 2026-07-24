//! Stream-json effect types shared by the Claude mapper and session sink.

use serde::{Deserialize, Serialize};

use rho_sdk::model::{ContextUsage, ModelUsage};

use crate::{subagent::RunState, tui::AttachmentEvent};

/// Soft cap for a single assistant text or reasoning delta retained/displayed.
pub(crate) const MAX_TEXT_DELTA_CHARS: usize = 32 * 1024;

/// Soft cap for terminal `result` text retained on status/attachments.
pub(crate) const MAX_RESULT_CHARS: usize = 64 * 1024;

/// Soft cap for tool input/result payload text shown in display lines.
pub(crate) const MAX_TOOL_PAYLOAD_CHARS: usize = 16 * 1024;

/// One presentation or status update produced from a stream-json line.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum StreamEffect {
    Attachment(AttachmentEvent),
    Status(StatusPatch),
    RateLimit(RateLimitInfo),
    /// Parsed terminal `result` message. Does **not** set run state or emit
    /// Completed/Failed by itself; session combines with process exit.
    Terminal(TerminalResult),
}

/// Incremental status fields. Terminal Ok/Error is not applied here from a
/// `result` message; session combines [`TerminalResult`] with process exit.
/// `last_activity` may say "result received" while the run is still pending.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct StatusPatch {
    pub(crate) state: Option<RunState>,
    pub(crate) turns: Option<u64>,
    pub(crate) input_tokens: Option<u64>,
    pub(crate) output_tokens: Option<u64>,
    pub(crate) last_activity: Option<String>,
    pub(crate) append_text: Option<String>,
    pub(crate) result: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) claude_session_id: Option<String>,
    pub(crate) total_cost_usd: Option<f64>,
}

/// Explicit terminal classification. Missing `subtype` / `is_error` is never
/// treated as success.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TerminalClassification {
    Success {
        subtype: String,
    },
    Failure {
        subtype: String,
        is_error: bool,
    },
    /// Schema drift or incomplete terminal fields.
    Invalid {
        reason: String,
    },
}

impl TerminalClassification {
    pub(crate) fn is_success(&self) -> bool {
        matches!(self, Self::Success { .. })
    }

    pub(crate) fn is_failure(&self) -> bool {
        matches!(self, Self::Failure { .. })
    }

    pub(crate) fn is_invalid(&self) -> bool {
        matches!(self, Self::Invalid { .. })
    }
}

/// Terminal `result` payload for the session layer to combine with child exit.
///
/// `ok` is a derived convenience equal to `classification.is_success()`. Session
/// must still combine this with process exit and must not treat stream success
/// alone as final run success when exit fails.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TerminalResult {
    pub(crate) classification: TerminalClassification,
    /// Derived from [`TerminalClassification::is_success`]. Prefer
    /// `classification` in new code.
    pub(crate) ok: bool,
    pub(crate) result_text: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) num_turns: Option<u64>,
    pub(crate) usage: Option<ModelUsage>,
    pub(crate) context: Option<ContextUsage>,
    pub(crate) total_cost_usd: Option<f64>,
    pub(crate) permission_denials: Vec<String>,
    pub(crate) stop_reason: Option<String>,
    pub(crate) subtype: Option<String>,
    pub(crate) is_error: Option<bool>,
}

/// Latest subscription rate-limit observation from a Claude stream.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RateLimitInfo {
    #[serde(default)]
    pub(crate) status: Option<String>,
    #[serde(default)]
    pub(crate) rate_limit_type: Option<String>,
    #[serde(default)]
    pub(crate) resets_at: Option<i64>,
    #[serde(default)]
    pub(crate) overage_status: Option<String>,
    #[serde(default)]
    pub(crate) overage_resets_at: Option<i64>,
    #[serde(default)]
    pub(crate) is_using_overage: Option<bool>,
}

/// Classify a terminal result from required `subtype` + `is_error` fields.
///
/// Matrix:
/// - `success` + `is_error=false` → Success
/// - `success` + `is_error=true` → Failure (contradiction treated as failure)
/// - non-success subtype + any `is_error` → Failure
/// - missing subtype and/or missing `is_error` → Invalid (never default success)
pub(crate) fn classify_terminal_result(
    subtype: Option<&str>,
    is_error: Option<bool>,
) -> TerminalClassification {
    let Some(subtype) = subtype.filter(|value| !value.is_empty()) else {
        return TerminalClassification::Invalid {
            reason: match is_error {
                Some(true) => "claude result missing subtype (is_error=true)".into(),
                Some(false) => "claude result missing subtype (is_error=false)".into(),
                None => "claude result missing subtype and is_error".into(),
            },
        };
    };
    let Some(is_error) = is_error else {
        return TerminalClassification::Invalid {
            reason: format!("claude result subtype `{subtype}` missing is_error"),
        };
    };
    if subtype == "success" && !is_error {
        TerminalClassification::Success {
            subtype: subtype.to_string(),
        }
    } else {
        TerminalClassification::Failure {
            subtype: subtype.to_string(),
            is_error,
        }
    }
}

pub(crate) fn describe_rate_limit(info: &RateLimitInfo) -> String {
    let window = info
        .rate_limit_type
        .as_deref()
        .map(humanize_rate_limit_type)
        .unwrap_or_else(|| "usage window".into());
    let status = info.status.as_deref().unwrap_or("unknown");
    let mut parts = vec![format!("{window}, {status}")];
    if let Some(resets_at) = info.resets_at {
        parts.push(format!("resets {}", format_unix_local(resets_at)));
    }
    if info.is_using_overage == Some(true) {
        parts.push("using overage".into());
    }
    parts.join(", ")
}

fn humanize_rate_limit_type(value: &str) -> String {
    value.replace('_', " ")
}

fn format_unix_local(unix: i64) -> String {
    chrono::DateTime::from_timestamp(unix, 0)
        .map(|value| {
            value
                .with_timezone(&chrono::Local)
                .format("%H:%M")
                .to_string()
        })
        .unwrap_or_else(|| format!("unix {unix}"))
}
