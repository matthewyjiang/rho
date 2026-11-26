//! On-demand reports about a loaded session, for a frontend's overlays.
//!
//! [`SessionInspectResponse::typed_report`] carries each report as typed
//! data ([`SessionReportData`]), defined in one module per report.

mod compaction;
mod context;
mod hooks;
mod provider_context;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use compaction::{
    CompactionCheck, CompactionContextSize, CompactionExtent, CompactionReread, CompactionRun,
    CompactionRunOutcome, CompactionRunTrigger, CompactionSize, CompactionSkipReason,
    CompactionThreshold, CompactionTier, CompletedCompactions, SessionCompactionReport,
    SummaryRequestPath,
};
pub use context::{
    ContextBasis, ContextMessageKind, ContextMessagePart, ContextPromptPart, ContextToolResultPart,
    ContextToolSchemaPart, PromptSourceKind, SessionContextReport, ToolSchemaOrigin,
};
pub use hooks::{
    HookActivityRecord, HookContract, HookEvent, HookOutcome, HookStatus, HookTools, HookWarning,
    SessionHooksReport, SkippedHooksFile,
};
pub use provider_context::{ProviderContextBlockId, SessionProviderContextReport};

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
///
/// # Next major
///
/// NEXT_MAJOR(rho-protocol): replace the untyped SessionInspectResponse::report with the typed SessionReportData, render rho's TUI from it, and remove typed_report.
///
/// Changing the type of `report` breaks the public struct, so a minor adds
/// the typed data beside it. Until the major, read `typed_report`, and treat
/// its absence as a daemon that predates it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionInspectResponse {
    /// The report as rho's own frontends show it. Presentation data, not a
    /// machine-readable contract: its shape follows the rho version. Read
    /// [`Self::typed_report`] instead.
    pub report: serde_json::Value,
    /// The report as typed data, of the kind the params asked for. Absent
    /// from a daemon that predates it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typed_report: Option<SessionReportData>,
}

/// One report's data, tagged by the [`SessionReport`] it answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SessionReportData {
    /// [`SessionReport::Context`].
    Context(SessionContextReport),
    /// [`SessionReport::Hooks`].
    Hooks(SessionHooksReport),
    /// [`SessionReport::Compaction`].
    // Boxed: it is several times the size of the other reports.
    Compaction(Box<SessionCompactionReport>),
    /// [`SessionReport::ProviderContext`].
    ProviderContext(SessionProviderContextReport),
    /// A report a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

#[cfg(test)]
#[path = "inspect_tests.rs"]
mod tests;
