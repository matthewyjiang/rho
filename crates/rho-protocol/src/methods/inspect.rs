//! On-demand reports about a loaded session, for a frontend's overlays.
//!
//! [`SESSION_INSPECT`] answers a report as presentation data, and
//! [`SESSION_REPORT`] the same report as typed data ([`SessionReportData`]),
//! defined in one module per report.

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
    InvalidHooksFile, SessionHooksReport, SkippedHooksFile,
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
    /// The session's hooks files read again, checked against the hooks the
    /// session runs, and their recent activity. Reading it leaves the
    /// session's hooks as they are; [`super::SESSION_HOOKS_RELOAD`] reloads
    /// them.
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
/// NEXT_MAJOR(rho-protocol): make rho/session/inspect answer the typed SessionReportData, render rho's TUI from it, and remove rho/session/report.
///
/// `SessionInspectResponse` cannot take a field or change its field's type
/// in a minor release, so until then the typed reports come from
/// [`SESSION_REPORT`] alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionInspectResponse {
    /// The report as rho's own frontends show it. Presentation data, not a
    /// machine-readable contract: its shape follows the rho version. Read
    /// [`SESSION_REPORT`] for the report as typed data.
    pub report: serde_json::Value,
}

/// Read a report about a loaded session as typed data, between its turns:
/// [`SESSION_INSPECT`]'s report, which the daemon answers with a
/// [`SessionReportData`]. It is refused as [`SESSION_INSPECT`] is: an
/// unloaded session is [`crate::ErrorCode::SessionNotLoaded`], and one with a
/// turn or a change under way [`crate::ErrorCode::SessionBusy`]. A daemon
/// that predates it answers [`crate::ErrorCode::MethodNotFound`].
pub const SESSION_REPORT: &str = "rho/session/report";

/// Params of [`SESSION_REPORT`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SessionReportParams {
    pub session: SessionId,
    pub report: SessionReport,
}

impl SessionReportParams {
    /// Builds it from each of its fields.
    pub fn new(session: SessionId, report: SessionReport) -> Self {
        Self { session, report }
    }
}

/// Result of [`SESSION_REPORT`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SessionReportResponse {
    /// The report the params asked for.
    pub report: SessionReportData,
}

impl SessionReportResponse {
    /// Builds it from each of its fields.
    pub fn new(report: SessionReportData) -> Self {
        Self { report }
    }
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
