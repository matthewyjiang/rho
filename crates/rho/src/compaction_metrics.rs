//! Per-compaction metrics: what each compaction cost, and what followed it.
//!
//! Records token counts, timings, labels, and hashed tool-call identities
//! only. Never conversation text, tool arguments, or provider payloads.
//!
//! A record is born when the compactor finishes. After the SDK commits it, the
//! host feeds two follow-up signals: the first provider-reported prompt size,
//! and the next [`REREAD_WINDOW_TOOL_CALLS`] tool calls, counting calls that
//! repeat a `read_file` or shell command whose result the compaction removed.

use std::{
    collections::HashSet,
    hash::{DefaultHasher, Hash, Hasher},
};

use rho_sdk::{
    model::{ContentBlock, Message, ModelUsage, ToolCall},
    CompactionTrigger,
};
use serde::Serialize;

/// How many tool calls after a compaction are checked for repeats.
///
/// Measured over 135 local sessions (5,824 tool calls): the gap between a
/// `read_file`/shell call and its repeat was p50 8, p75 24, p90 48 calls. 24
/// catches most natural repeats; the rate is a comparison between tiers, so
/// the baseline cancels out. Shown in diagnostics as `reread.window`.
pub(crate) const REREAD_WINDOW_TOOL_CALLS: u32 = 24;

/// Which compaction tier produced the result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CompactionTier {
    /// Tool-result elision alone reached the target; no model request was made.
    Elision,
    /// Provider server-side compaction.
    Native,
    /// Portable text-summary compaction.
    TextSummary,
    /// Nothing was compactable; history was returned unchanged.
    Unchanged,
}

impl CompactionTier {
    /// Matches the serialized name.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Elision => "elision",
            Self::Native => "native",
            Self::TextSummary => "text_summary",
            Self::Unchanged => "unchanged",
        }
    }
}

/// Which request wrote a text summary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SummaryRequestPath {
    /// The session model on the session's own history, reusing its prompt cache.
    SessionHistory,
    /// The session model on a rendered transcript.
    Transcript,
    /// The configured summarizer model on a rendered transcript.
    Summarizer,
}

impl SummaryRequestPath {
    /// Matches the serialized name.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::SessionHistory => "session_history",
            Self::Transcript => "transcript",
            Self::Summarizer => "summarizer",
        }
    }
}

/// How the compactor call ended. `Completed` means the compactor returned a
/// replacement, not that the SDK committed it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CompactionRunOutcome {
    Completed,
    Failed,
    Cancelled,
}

impl CompactionRunOutcome {
    pub(crate) fn of<T>(result: &Result<T, rho_sdk::Error>) -> Self {
        match result {
            Ok(_) => Self::Completed,
            Err(rho_sdk::Error::Cancelled) => Self::Cancelled,
            Err(_) => Self::Failed,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Why the compaction ran. Idle pre-prompt auto-compaction goes through the
/// manual SDK entry point and records `manual`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CompactionTriggerKind {
    Automatic,
    Manual,
    ContextOverflow,
    /// A trigger added to the SDK after this host was built.
    Other,
}

impl CompactionTriggerKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::Manual => "manual",
            Self::ContextOverflow => "context_overflow",
            Self::Other => "other",
        }
    }
}

impl From<CompactionTrigger> for CompactionTriggerKind {
    fn from(trigger: CompactionTrigger) -> Self {
        match trigger {
            CompactionTrigger::Automatic => Self::Automatic,
            CompactionTrigger::Manual => Self::Manual,
            CompactionTrigger::ContextOverflow => Self::ContextOverflow,
            _ => Self::Other,
        }
    }
}

/// Repeated tool calls after a committed compaction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct RereadStats {
    /// Tool calls this record watches, [`REREAD_WINDOW_TOOL_CALLS`].
    pub window: u32,
    /// Tool calls seen so far, at most `window`. Lower than `window` when a
    /// later compaction, a session change, or process exit cut the window short.
    pub tool_calls: u32,
    /// Seen calls that repeat a removed `read_file` path or shell command.
    pub repeated: u32,
    /// Distinct removed `read_file` paths and shell commands being watched.
    pub tracked: u32,
}

/// Where a record's ledger row belongs. Not shown in diagnostics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RecordIdentity {
    pub event_id: String,
    pub occurred_at_ms: i64,
    pub session_id: Option<String>,
    pub parent_session_id: Option<String>,
    pub run_id: Option<String>,
    pub workspace_path: Option<String>,
}

/// Metrics for one compactor call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct CompactionRecord {
    #[serde(skip)]
    pub identity: RecordIdentity,
    pub outcome: CompactionRunOutcome,
    pub trigger: CompactionTriggerKind,
    /// The tier that produced the result, or the one that failed. `None` when
    /// the call ended before any tier ran.
    pub tier: Option<CompactionTier>,
    /// Set when a text-summary request was sent: the last one tried.
    pub request_path: Option<SummaryRequestPath>,
    /// `provider/model` that served the last model request, if any.
    pub model: Option<String>,
    /// Tool results replaced with recall stubs in the committed history.
    pub elided_tool_results: usize,
    /// Calibrated context tokens of the history handed to the compactor.
    pub context_tokens: u64,
    /// Provider-reported usage summed over every compaction request,
    /// including failed native and summary attempts.
    pub prompt_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cost_usd_micros: Option<u64>,
    pub latency_ms: u64,
    /// First provider-reported prompt size after the commit. For a commit
    /// that left history unchanged, the surviving measurement.
    pub next_prompt_tokens: Option<u64>,
    /// `None` until the compaction commits in a host that watches tool calls.
    pub reread: Option<RereadStats>,
}

impl CompactionRecord {
    /// Copies the usage fields this record keeps.
    pub(crate) fn with_usage(mut self, usage: &ModelUsage) -> Self {
        self.prompt_tokens = usage.inclusive_prompt_tokens();
        self.output_tokens = usage.output_tokens;
        self.cache_read_tokens = usage.cache_read_tokens;
        self.cost_usd_micros = usage.cost_usd_micros;
        self
    }
}

/// Hashed identity of a `read_file` path or a shell command. The hash is
/// process-local and never persisted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ToolFingerprint(u64);

impl ToolFingerprint {
    /// `read_file` by path and shell tools by command. Other tools, and calls
    /// without that argument, return `None`.
    pub(crate) fn of(call: &ToolCall) -> Option<Self> {
        let (kind, field) = match call.name.as_str() {
            "read_file" => ("read", "path"),
            "bash" | "powershell" => ("run", "command"),
            _ => return None,
        };
        let value = call.arguments.get(field)?.as_str()?.trim();
        let mut hasher = DefaultHasher::new();
        (kind, value).hash(&mut hasher);
        Some(Self(hasher.finish()))
    }
}

/// Fingerprints of `before` calls whose tool result is not in `after`
/// verbatim: dropped by a summary or replaced by an elision stub.
pub(crate) fn removed_tool_calls(
    before: &[Message],
    after: &[Message],
) -> HashSet<ToolFingerprint> {
    let kept = after
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some((result.id.as_str(), result.content.as_str())),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let removed = before
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult(result)
                if !kept.contains(&(result.id.as_str(), result.content.as_str())) =>
            {
                Some(result.id.as_str())
            }
            _ => None,
        })
        .collect::<HashSet<_>>();
    before
        .iter()
        .filter_map(Message::completed_assistant_content)
        .flatten()
        .filter_map(|block| match block {
            ContentBlock::ToolCall(call) if removed.contains(call.id.as_str()) => {
                ToolFingerprint::of(call)
            }
            ContentBlock::Text(_) | ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Phase {
    /// No record, or its follow-up is finished or will never start.
    #[default]
    Idle,
    /// The compactor returned a replacement the SDK has not committed yet.
    AwaitingCommit,
    /// Committed; collecting follow-up signals.
    Following,
}

/// The latest record and its follow-up state. Every method that finishes or
/// abandons a follow-up returns the record so the caller can persist it.
#[derive(Clone, Debug, Default)]
pub(crate) struct CompactionMetrics {
    last: Option<CompactionRecord>,
    removed: HashSet<ToolFingerprint>,
    phase: Phase,
}

impl CompactionMetrics {
    pub(crate) fn last(&self) -> Option<&CompactionRecord> {
        self.last.as_ref()
    }

    /// Replaces the latest record. Returns the previous one if its follow-up
    /// was cut short.
    pub(crate) fn record(
        &mut self,
        record: CompactionRecord,
        removed: HashSet<ToolFingerprint>,
    ) -> Option<CompactionRecord> {
        let superseded = self.take_unfinished();
        self.phase = match record.outcome {
            CompactionRunOutcome::Completed => Phase::AwaitingCommit,
            CompactionRunOutcome::Failed | CompactionRunOutcome::Cancelled => Phase::Idle,
        };
        self.last = Some(record);
        self.removed = removed;
        superseded
    }

    /// Starts the follow-up once the SDK committed the latest replacement.
    pub(crate) fn committed(&mut self) {
        if self.phase != Phase::AwaitingCommit {
            return;
        }
        let Some(record) = self.last.as_mut() else {
            return;
        };
        record.reread = Some(RereadStats {
            window: REREAD_WINDOW_TOOL_CALLS,
            tracked: u32::try_from(self.removed.len()).unwrap_or(u32::MAX),
            ..RereadStats::default()
        });
        self.phase = Phase::Following;
    }

    /// Takes the first provider-reported prompt size after the commit.
    ///
    /// Returns a record to save as soon as the size is known, so it survives
    /// an exit before the re-read window ends. That early copy leaves the
    /// re-read fields empty: the ledger keeps each follow-up column's first
    /// non-null value, and a partial count must not stick.
    pub(crate) fn observe_prompt_tokens(
        &mut self,
        tokens: Option<u64>,
    ) -> Option<CompactionRecord> {
        let record = self.following()?;
        if record.next_prompt_tokens.is_some() || tokens.is_none() {
            return None;
        }
        record.next_prompt_tokens = tokens;
        let early = CompactionRecord {
            reread: None,
            ..record.clone()
        };
        self.take_finished().or(Some(early))
    }

    pub(crate) fn observe_tool_call(&mut self, call: &ToolCall) -> Option<CompactionRecord> {
        let repeated = ToolFingerprint::of(call).is_some_and(|print| self.removed.contains(&print));
        let reread = self.following()?.reread.as_mut()?;
        if reread.tool_calls < reread.window {
            reread.tool_calls += 1;
            reread.repeated += u32::from(repeated);
        }
        self.take_finished()
    }

    /// Ends any follow-up in progress. Returns the record when its follow-up
    /// was cut short, so a session change or process exit can save the partial
    /// counts.
    pub(crate) fn take_unfinished(&mut self) -> Option<CompactionRecord> {
        let unfinished = (self.phase == Phase::Following)
            .then(|| self.last.clone())
            .flatten();
        self.phase = Phase::Idle;
        self.removed.clear();
        unfinished
    }

    fn following(&mut self) -> Option<&mut CompactionRecord> {
        (self.phase == Phase::Following)
            .then_some(self.last.as_mut())
            .flatten()
    }

    fn take_finished(&mut self) -> Option<CompactionRecord> {
        let record = self.last.as_ref()?;
        let done = record.next_prompt_tokens.is_some()
            && record
                .reread
                .is_some_and(|reread| reread.tool_calls >= reread.window);
        if !done {
            return None;
        }
        self.phase = Phase::Idle;
        self.removed.clear();
        Some(record.clone())
    }
}

#[cfg(test)]
#[path = "compaction_metrics_tests.rs"]
mod tests;
