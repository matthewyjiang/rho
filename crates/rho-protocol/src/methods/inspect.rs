//! On-demand reports about a loaded session, for a frontend's overlays.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::SessionId;

/// Read a report about a loaded session, between its turns. An unloaded
/// session is [`crate::ErrorCode::SessionNotLoaded`]; one with a turn or a
/// change under way is [`crate::ErrorCode::SessionBusy`], since a report
/// reads what a turn is changing.
pub const SESSION_INSPECT: &str = "rho/session/inspect";

/// Params of [`SESSION_INSPECT`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionInspectParams {
    pub session: SessionId,
    pub report: SessionReport,
}

/// A report [`SESSION_INSPECT`] reads. Protocol minors may add reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SessionReport {
    /// What fills the model's context window on the next request, by
    /// source.
    Context,
    /// The session's hooks, read again from their files, and their recent
    /// activity.
    Hooks,
    /// The session's compaction settings as token counts, what its
    /// compactions removed, and what the latest one did and cost.
    Compaction,
    /// The provider-native context in the session's conversation that its
    /// model cannot be sent, the model that produced it, and whether
    /// compacting could summarize the conversation first.
    ProviderContext,
}

/// Result of [`SESSION_INSPECT`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionInspectResponse {
    /// The report as rho's own frontends show it. Presentation data, not a
    /// machine-readable contract: its shape follows the rho version.
    pub report: serde_json::Value,
}
