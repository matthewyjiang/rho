//! Shared pieces of the `grep` and `glob` workspace search tools.
//!
//! Both tools resolve one read root, run a bounded [`crate::workspace_walk`],
//! and render text plus a summary of why the search stopped. This module owns
//! everything that is identical between them so each tool only supplies its
//! pattern semantics and output layout.

use std::{path::Path, time::Duration};

use serde_json::Value;

use crate::{hashline::SnapshotStore, tool::ToolError, workspace_walk::WalkStop};

/// Default number of results returned by one search call.
pub(crate) const DEFAULT_MAX_RESULTS: usize = 200;
/// Hard ceiling so callers cannot request unbounded output.
pub(crate) const MAX_RESULTS_CEILING: usize = 1_000;
/// Wall-clock bound for one search call.
pub(crate) const SEARCH_DEADLINE: Duration = Duration::from_secs(15);

/// Applies a caller-supplied bound, falling back to `default` and never
/// exceeding `ceiling`.
pub(crate) fn clamp_limit(value: Option<usize>, default: usize, ceiling: usize) -> usize {
    value.unwrap_or(default).clamp(1, ceiling)
}

/// Why a search returned less than the whole answer.
///
/// Kept as data rather than prose so callers can ask about a specific reason
/// without matching on rendered text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StopReason {
    ResultLimit,
    PerFileLimit { files: usize },
    ScanLimit,
    Deadline,
    Cancelled,
}

/// How a tool phrases "make the search smaller", e.g. `"the pattern or path"`.
///
/// `grep` and `glob` accept different narrowing arguments, so the advice that
/// accompanies a limit differs even though the limits themselves do not.
#[derive(Clone, Copy)]
pub(crate) struct NarrowHint(pub(crate) &'static str);

impl StopReason {
    fn describe(self, narrow: NarrowHint) -> String {
        let NarrowHint(narrow) = narrow;
        match self {
            Self::ResultLimit => format!("result limit reached; narrow {narrow}"),
            Self::PerFileLimit { files } => format!(
                "{files} files truncated by max_per_file; raise max_per_file or narrow the pattern"
            ),
            Self::ScanLimit => format!("scan limit reached; narrow {narrow}"),
            Self::Deadline => "time limit reached".to_string(),
            Self::Cancelled => "cancelled".to_string(),
        }
    }
}

/// Translates how the walk ended into the reason list shown to the model.
///
/// `per_file_truncated` counts files whose match lines were cut by
/// `max_per_file`; it is zero for tools without that bound.
pub(crate) fn stop_reasons(walk_stop: WalkStop, per_file_truncated: usize) -> Vec<StopReason> {
    let mut reasons = Vec::new();
    match walk_stop {
        WalkStop::ResultLimit => reasons.push(StopReason::ResultLimit),
        WalkStop::EntryLimit => reasons.push(StopReason::ScanLimit),
        WalkStop::Deadline => reasons.push(StopReason::Deadline),
        WalkStop::Cancelled => reasons.push(StopReason::Cancelled),
        WalkStop::Completed => {}
    }
    if per_file_truncated > 0 {
        reasons.push(StopReason::PerFileLimit {
            files: per_file_truncated,
        });
    }
    reasons
}

/// Appends the stop reasons to a summary line, or leaves it alone when the
/// search ran to completion.
pub(crate) fn with_reasons(counts: String, reasons: &[StopReason], narrow: NarrowHint) -> String {
    if reasons.is_empty() {
        return counts;
    }
    let rendered: Vec<String> = reasons
        .iter()
        .map(|reason| reason.describe(narrow))
        .collect();
    format!("{counts} ({})", rendered.join("; "))
}

/// One workspace search tool, as seen by the SDK adapter.
///
/// Implementors own the arguments they accept and the text they render; the
/// adapter owns capability requests, resource declarations, cancellation, and
/// output truncation. Adding a search tool means adding an implementation
/// here, not another adapter.
pub(crate) trait WorkspaceSearch: Send + Sync + 'static {
    /// Validated arguments, built before any capability is requested so an
    /// invalid pattern cannot cost an authorization round trip.
    type Request: Send + 'static;

    /// Tool name, used for the capability source and error messages.
    const NAME: &'static str;

    fn spec() -> crate::tool::ToolSpec;

    fn parse(arguments: Value) -> Result<Self::Request, ToolError>;

    /// The workspace-relative root the request searches under.
    fn root(request: &Self::Request) -> &str;

    /// Runs the search on a blocking thread. `display_root` is the root as it
    /// should appear in output; `cancelled` is polled between files.
    fn run(
        root: &Path,
        display_root: &str,
        request: &Self::Request,
        cancelled: &dyn Fn() -> bool,
        snapshots: Option<&SnapshotStore>,
    ) -> Result<String, ToolError>;
}
