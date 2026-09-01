//! Stream-json effect types shared by the Claude mapper and session sink.

use serde::{Deserialize, Serialize};

use rho_sdk::model::{ContextUsage, ModelUsage};

use crate::{run_artifacts::AttachmentEvent, subagent::RunState};

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
    /// Concrete model Claude reported running, from the `init` frame. Rho
    /// passes `--model` through untouched, so this is the only report of what
    /// an alias such as `opus` actually resolved to.
    pub(crate) claude_model: Option<String>,
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

#[cfg(test)]
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
/// Session must still combine this with process exit and must not treat stream
/// success alone as final run success when exit fails.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TerminalResult {
    pub(crate) classification: TerminalClassification,
    pub(crate) result_text: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) num_turns: Option<u64>,
    pub(crate) usage: Option<ModelUsage>,
    pub(crate) context: Option<ContextUsage>,
    pub(crate) total_cost_usd: Option<f64>,
    pub(crate) permission_denials: Vec<String>,
    pub(crate) stop_reason: Option<String>,
}

/// Latest subscription rate-limit observation from a Claude stream.
///
/// One event is one window (`five_hour`, `seven_day`, …). `utilization` is an
/// optional used fraction in `0.0..=1.0`; Claude often omits it while status is
/// plain `allowed`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RateLimitInfo {
    #[serde(default)]
    pub(crate) status: Option<String>,
    #[serde(default)]
    pub(crate) rate_limit_type: Option<String>,
    #[serde(default)]
    pub(crate) resets_at: Option<i64>,
    /// Used fraction in `0.0..=1.0` when Claude includes it.
    #[serde(default)]
    pub(crate) utilization: Option<f64>,
    #[serde(default)]
    pub(crate) overage_status: Option<String>,
    #[serde(default)]
    pub(crate) overage_resets_at: Option<i64>,
    #[serde(default)]
    pub(crate) is_using_overage: Option<bool>,
}

impl RateLimitInfo {
    /// Stable key for merging multi-window state. Missing type shares one bucket.
    pub(crate) fn window_key(&self) -> &str {
        self.rate_limit_type
            .as_deref()
            .filter(|value| !value.is_empty())
            .unwrap_or("usage_window")
    }

    /// Remaining percent derived from utilization, when present.
    pub(crate) fn remaining_percent(&self) -> Option<f64> {
        let used = self.utilization?;
        if !used.is_finite() {
            return None;
        }
        Some(((1.0 - used) * 100.0).clamp(0.0, 100.0))
    }

    /// Human window label (`five_hour` → `5-hour`, `seven_day` → `Weekly`).
    pub(crate) fn window_label(&self) -> String {
        crate::claude_runtime::window_kind::WindowKind::from_key(self.window_key()).label()
    }
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
    let mut parts = vec![info.window_label()];
    if let Some(remaining) = info.remaining_percent() {
        parts.push(format!("{}% left", remaining.round() as u8));
    }
    if let Some(status) = notable_rate_limit_status(info.status.as_deref()) {
        parts.push(status);
    }
    if let Some(resets_at) = info.resets_at {
        parts.push(format!("resets {}", format_unix_local(resets_at)));
    }
    if info.is_using_overage == Some(true) {
        parts.push("using overage".into());
    }
    parts.join(", ")
}

/// Status text worth showing. Plain `allowed` is noise and is omitted.
pub(crate) fn notable_rate_limit_status(status: Option<&str>) -> Option<String> {
    let status = status?.trim();
    if status.is_empty() {
        return None;
    }
    match RateLimitStatusKind::parse(status) {
        RateLimitStatusKind::Allowed => None,
        RateLimitStatusKind::AllowedWarning => Some("warning".into()),
        RateLimitStatusKind::Rejected => Some("limited".into()),
        RateLimitStatusKind::Other(other) => Some(other.replace('_', " ")),
    }
}

/// Wire status values Claude may report on a rate-limit window.
enum RateLimitStatusKind<'a> {
    Allowed,
    AllowedWarning,
    Rejected,
    Other(&'a str),
}

impl<'a> RateLimitStatusKind<'a> {
    fn parse(status: &'a str) -> Self {
        match status {
            "allowed" => Self::Allowed,
            "allowed_warning" => Self::AllowedWarning,
            "rejected" => Self::Rejected,
            other => Self::Other(other),
        }
    }
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
