//! Effect types shared by the ACP event renderer and the session sink.
//!
//! Protocol-neutral presentation and terminal contracts. Rate-limit display
//! policy stays with the runtime that observes the limits.

use serde::{Deserialize, Serialize};

use crate::{run_artifacts::AttachmentEvent, subagent::RunState};

/// Soft cap for a single assistant text or reasoning delta retained/displayed.
pub(crate) const MAX_TEXT_DELTA_CHARS: usize = 32 * 1024;

/// Soft cap for terminal `result` text retained on status/attachments.
pub(crate) const MAX_RESULT_CHARS: usize = 64 * 1024;

/// Soft cap for tool input/result payload text shown in display lines.
pub(crate) const MAX_TOOL_PAYLOAD_CHARS: usize = 16 * 1024;

/// One presentation or status update produced from an agent event.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum StreamEffect {
    Attachment(AttachmentEvent),
    Status(StatusPatch),
    RateLimit(RateLimitInfo),
}

/// Incremental status fields. The terminal state comes from the sink's finish
/// methods, never from a patch.
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
    // NEXT_MAJOR(result.json): rename claude_session_id/claude_model to runtime_session_id/runtime_model; readers branch on runtime.
    pub(crate) claude_session_id: Option<String>,
    /// Concrete model Claude reported running, from usage updates. Rho
    /// passes the model through untouched, so this is the only report of what
    /// an alias such as `opus` actually resolved to.
    // NEXT_MAJOR(result.json): rename claude_session_id/claude_model to runtime_session_id/runtime_model; readers branch on runtime.
    pub(crate) claude_model: Option<String>,
    pub(crate) total_cost_usd: Option<f64>,
}

/// What a finished turn hands the sink alongside its terminal state.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TerminalResult {
    pub(crate) result_text: String,
    pub(crate) session_id: Option<String>,
    pub(crate) turns: u64,
}

/// Latest subscription rate-limit observation from a CLI stream.
///
/// One event is one window (`five_hour`, `seven_day`, …). `utilization` is an
/// optional used fraction in `0.0..=1.0`; the provider often omits it while
/// status is plain `allowed`. Field names follow the on-disk cache contract.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RateLimitInfo {
    #[serde(default)]
    pub(crate) status: Option<String>,
    #[serde(default)]
    pub(crate) rate_limit_type: Option<String>,
    #[serde(default)]
    pub(crate) resets_at: Option<i64>,
    /// Used fraction in `0.0..=1.0` when the provider includes it.
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
}
