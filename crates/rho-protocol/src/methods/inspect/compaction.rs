//! [`super::SessionReport::Compaction`]: the session's compaction settings
//! as token counts, what its compactions removed, and what the latest one
//! did and cost.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::methods::ModelRef;

/// The session's compaction settings against its context, and what its
/// compactions did. "Local" counts come from rho's own estimator; the
/// others are calibrated against what a provider reported, when one did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionCompactionReport {
    /// Whether the session compacts automatically.
    pub auto_compact: bool,
    /// How large the context is now.
    pub context: CompactionContextSize,
    /// The model's context window in tokens, when the session knows it.
    /// The threshold and targets below need it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_tokens: Option<u64>,
    /// The context size at which the session compacts automatically, in
    /// tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold_tokens: Option<u64>,
    /// The context size a compaction aims for, in tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_tokens: Option<u64>,
    /// That target in local tokens: what the compactor measures against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_target_tokens: Option<u64>,
    /// What the session's committed compactions removed.
    pub completed: CompletedCompactions,
    /// The latest compactor call, committed or not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<CompactionRun>,
    /// The latest check of whether to compact automatically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_check: Option<CompactionCheck>,
}

/// How large a context is, in tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompactionContextSize {
    /// The size, calibrated when a provider measured the prompt.
    pub tokens: u64,
    /// rho's local estimate of the messages, tools and request framing.
    pub local_tokens: u64,
    /// The prompt size the provider reported for the latest request it
    /// measured, when it still applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_reported_tokens: Option<u64>,
    /// The local estimate of that same request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_request_local_tokens: Option<u64>,
}

/// What a session's committed compactions removed, in total.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompletedCompactions {
    /// How many compactions committed.
    pub count: u64,
    /// The local tokens they removed.
    pub removed_local_tokens: u64,
    /// The context before and after the latest one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<CompactionSize>,
}

/// A context's size before and after one compaction, in local tokens. A
/// compaction can leave it larger.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompactionSize {
    pub before_local_tokens: u64,
    pub after_local_tokens: u64,
}

/// One compactor call: what it was, what it cost, and what followed once
/// it committed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompactionRun {
    pub trigger: CompactionRunTrigger,
    pub outcome: CompactionRunOutcome,
    /// The tier that produced the result, or the one that failed; absent
    /// when the call ended before any ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<CompactionTier>,
    /// The request that wrote a text summary, the last one tried; absent
    /// when none was sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_request: Option<SummaryRequestPath>,
    /// The model that served the last model request, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    /// Tool results replaced by recall stubs in the history it committed.
    pub elided_tool_results: u64,
    /// The calibrated context tokens of the history it was given.
    pub context_tokens: u64,
    /// Prompt tokens the provider reported over every compaction request,
    /// failed attempts included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u64>,
    /// Output tokens, summed the same way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    /// Prompt tokens read from the provider's cache, summed the same way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    /// What those requests cost, in millionths of a US dollar, when the
    /// session prices them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd_micros: Option<u64>,
    /// How long the call took, in milliseconds.
    pub latency_ms: u64,
    /// The first prompt size a provider reported after it committed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_prompt_tokens: Option<u64>,
    /// Tool calls after it that repeated what it removed; absent until it
    /// commits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reread: Option<CompactionReread>,
}

/// Why a compaction ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CompactionRunTrigger {
    /// The context reached its threshold during a turn.
    Automatic,
    /// A person asked, or the session compacted before a prompt.
    Manual,
    /// A request overflowed the model's context window.
    ContextOverflow,
    /// A trigger this crate does not know, including one the daemon's own
    /// rho does not name.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// How a compactor call ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CompactionRunOutcome {
    /// It returned a replacement history, which the session may still not
    /// have committed.
    Completed,
    Failed,
    Cancelled,
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// How a compaction shrank the history.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CompactionTier {
    /// Eliding tool results reached the target, without a model request.
    Elision,
    /// The provider compacted the history server-side.
    Native,
    /// A model summarized the history as text.
    TextSummary,
    /// Nothing could be compacted; the history stayed as it was.
    Unchanged,
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// Which request wrote a text summary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SummaryRequestPath {
    /// The session's model, on the session's own history, reusing its
    /// prompt cache.
    SessionHistory,
    /// The session's model, on a rendered transcript.
    Transcript,
    /// The configured summarizer model, on a rendered transcript.
    Summarizer,
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// The tool calls after a committed compaction that repeated a file read or
/// a shell command whose result it removed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompactionReread {
    /// How many tool calls after the compaction rho watches.
    pub window_tool_calls: u64,
    /// The tool calls seen so far, at most `window_tool_calls`; fewer when a
    /// later compaction, a session change, or an exit cut the window short.
    pub tool_calls: u64,
    /// The seen calls that repeated a removed read or command.
    pub repeated: u64,
    /// The distinct removed reads and commands watched for.
    pub tracked: u64,
}

/// One check of whether to compact automatically. A check that found
/// compaction due does not say that it succeeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompactionCheck {
    /// The calibrated context tokens it measured.
    pub context_tokens: u64,
    /// The threshold it compared against; absent when automatic compaction
    /// is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<CompactionThreshold>,
    /// Why it did not compact; absent when it found compaction due.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<CompactionSkipReason>,
    /// How much of the history a compaction could rewrite.
    pub extent: CompactionExtent,
}

/// When automatic compaction is due.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum CompactionThreshold {
    /// Once the conversation holds this many messages.
    Messages { messages: u64 },
    /// Once the context reaches this many tokens.
    ContextTokens { tokens: u64 },
    /// A threshold this crate does not know, including one the daemon's own
    /// rho does not name.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// Why a check did not compact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CompactionSkipReason {
    /// Automatic compaction is off.
    PolicyDisabled,
    /// The context is below the threshold.
    BelowThreshold,
    /// Running background tool calls left nothing before them to compact.
    PendingAsyncTools,
    /// A reason this crate does not know, including one the daemon's own
    /// rho does not name.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// How much of the history a compaction may rewrite.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CompactionExtent {
    /// All of it, except the newest input kept for the next request.
    History,
    /// Only what precedes the earliest running background tool call, so its
    /// late result still pairs with it.
    BeforePendingAsyncTools,
    /// An extent this crate does not know, including one the daemon's own
    /// rho does not name.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}
