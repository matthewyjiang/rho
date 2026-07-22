use std::{
    fmt,
    future::Future,
    num::{NonZeroU64, NonZeroUsize},
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};

use crate::{
    model::{context::estimate_messages_tokens, Message, ModelUsage},
    CancellationToken, Error, Revision,
};

/// Persisted continuation state for prior compactions.
///
/// Counters accumulate across successful automatic and manual compactions so
/// hosts can report how much history, estimated context, and optional cost the
/// session has discarded. Token and cost fields are additive snapshot fields
/// with defaults so older schema-compatible snapshots remain readable.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionState {
    completed_compactions: u64,
    removed_messages: u64,
    #[serde(default)]
    removed_tokens: u64,
    #[serde(default)]
    removed_cost_usd_micros: u64,
    #[serde(default)]
    last_previous_tokens: Option<u64>,
    #[serde(default)]
    last_current_tokens: Option<u64>,
    last_revision: Option<Revision>,
}

impl CompactionState {
    /// Reconstructs persisted compaction continuation state in a storage adapter.
    pub const fn from_parts(
        completed_compactions: u64,
        removed_messages: u64,
        last_revision: Option<Revision>,
    ) -> Self {
        Self {
            completed_compactions,
            removed_messages,
            removed_tokens: 0,
            removed_cost_usd_micros: 0,
            last_previous_tokens: None,
            last_current_tokens: None,
            last_revision,
        }
    }

    /// Reconstructs full compaction accounting, including token and cost totals.
    pub const fn from_accounting(
        completed_compactions: u64,
        removed_messages: u64,
        removed_tokens: u64,
        removed_cost_usd_micros: u64,
        last_previous_tokens: Option<u64>,
        last_current_tokens: Option<u64>,
        last_revision: Option<Revision>,
    ) -> Self {
        Self {
            completed_compactions,
            removed_messages,
            removed_tokens,
            removed_cost_usd_micros,
            last_previous_tokens,
            last_current_tokens,
            last_revision,
        }
    }

    pub fn completed_compactions(&self) -> u64 {
        self.completed_compactions
    }

    pub fn removed_messages(&self) -> u64 {
        self.removed_messages
    }

    /// Cumulative estimated context tokens removed by successful compactions.
    pub fn removed_tokens(&self) -> u64 {
        self.removed_tokens
    }

    /// Cumulative cost charged by host-supplied compactors, when reported.
    pub fn removed_cost_usd_micros(&self) -> u64 {
        self.removed_cost_usd_micros
    }

    /// Estimated context tokens present immediately before the latest compaction.
    pub fn last_previous_tokens(&self) -> Option<u64> {
        self.last_previous_tokens
    }

    /// Estimated context tokens present immediately after the latest compaction.
    pub fn last_current_tokens(&self) -> Option<u64> {
        self.last_current_tokens
    }

    pub fn last_revision(&self) -> Option<Revision> {
        self.last_revision
    }

    pub(crate) fn record(
        &mut self,
        removed_messages: usize,
        previous_tokens: u64,
        current_tokens: u64,
        cost_usd_micros: Option<u64>,
        revision: Revision,
    ) {
        self.completed_compactions = self.completed_compactions.saturating_add(1);
        self.removed_messages = self
            .removed_messages
            .saturating_add(removed_messages as u64);
        self.removed_tokens = self
            .removed_tokens
            .saturating_add(previous_tokens.saturating_sub(current_tokens));
        if let Some(cost_usd_micros) = cost_usd_micros {
            self.removed_cost_usd_micros =
                self.removed_cost_usd_micros.saturating_add(cost_usd_micros);
        }
        self.last_previous_tokens = Some(previous_tokens);
        self.last_current_tokens = Some(current_tokens);
        self.last_revision = Some(revision);
    }
}

/// Threshold used to decide when automatic compaction is requested.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CompactionThreshold {
    Messages(NonZeroUsize),
    ContextTokens(NonZeroU64),
}

/// Policy deciding when automatic compaction is requested.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompactionPolicy {
    threshold: CompactionThreshold,
}

impl CompactionPolicy {
    pub fn after_messages(trigger_messages: NonZeroUsize) -> Self {
        Self {
            threshold: CompactionThreshold::Messages(trigger_messages),
        }
    }

    /// Requests compaction once the SDK's provider-neutral context estimate
    /// reaches the supplied token threshold.
    pub fn at_context_tokens(trigger_tokens: NonZeroU64) -> Self {
        Self {
            threshold: CompactionThreshold::ContextTokens(trigger_tokens),
        }
    }

    pub fn threshold(&self) -> CompactionThreshold {
        self.threshold
    }

    pub(crate) fn should_compact(&self, message_count: usize, context_tokens: u64) -> bool {
        match self.threshold {
            CompactionThreshold::Messages(trigger) => message_count >= trigger.get(),
            CompactionThreshold::ContextTokens(trigger) => context_tokens >= trigger.get(),
        }
    }
}

/// Owned input supplied to a compaction transport.
#[derive(Clone, Debug)]
pub struct CompactionRequest {
    messages: Vec<Message>,
    cancellation: CancellationToken,
    session_id: Option<crate::SessionId>,
    parent_session_id: Option<crate::SessionId>,
    run_id: Option<crate::RunId>,
    step_index: Option<usize>,
    workspace_path: Option<PathBuf>,
}

impl CompactionRequest {
    pub(crate) fn new(messages: Vec<Message>, cancellation: CancellationToken) -> Self {
        Self {
            messages,
            cancellation,
            session_id: None,
            parent_session_id: None,
            run_id: None,
            step_index: None,
            workspace_path: None,
        }
    }

    pub(crate) fn with_request_context(
        mut self,
        session_id: crate::SessionId,
        parent_session_id: Option<crate::SessionId>,
        run_id: crate::RunId,
        step_index: Option<usize>,
        workspace_path: Option<PathBuf>,
    ) -> Self {
        self.session_id = Some(session_id);
        self.parent_session_id = parent_session_id;
        self.run_id = Some(run_id);
        self.step_index = step_index;
        self.workspace_path = workspace_path;
        self
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn session_id(&self) -> Option<&crate::SessionId> {
        self.session_id.as_ref()
    }

    pub fn parent_session_id(&self) -> Option<&crate::SessionId> {
        self.parent_session_id.as_ref()
    }

    pub fn run_id(&self) -> Option<&crate::RunId> {
        self.run_id.as_ref()
    }

    pub fn step_index(&self) -> Option<usize> {
        self.step_index
    }

    pub fn workspace_path(&self) -> Option<&std::path::Path> {
        self.workspace_path.as_deref()
    }

    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }
}

/// Valid replacement history produced by a compaction transport.
#[derive(Clone, Debug, PartialEq)]
pub struct CompactionOutput {
    messages: Vec<Message>,
    usage: ModelUsage,
}

impl CompactionOutput {
    pub fn new(messages: Vec<Message>) -> Result<Self, Error> {
        Self::with_usage(messages, ModelUsage::default())
    }

    /// Creates replacement history and optional usage charged to the compactor.
    pub fn with_usage(messages: Vec<Message>, usage: ModelUsage) -> Result<Self, Error> {
        if messages.is_empty() {
            return Err(Error::InvalidHostResponse {
                message: "compaction replacement history must not be empty".into(),
            });
        }
        Ok(Self { messages, usage })
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn usage(&self) -> &ModelUsage {
        &self.usage
    }

    pub fn into_messages(self) -> Vec<Message> {
        self.messages
    }

    pub(crate) fn into_parts(self) -> (Vec<Message>, ModelUsage) {
        (self.messages, self.usage)
    }
}

/// Future returned by [`Compactor`] implementations.
pub type CompactionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<CompactionOutput, Error>> + Send + 'a>>;

/// Defines who cancels an in-flight compaction future.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CompactorCancellationMode {
    /// The SDK must drop the future when cancellation is requested.
    #[default]
    External,
    /// The compactor observes `CompactionRequest::cancellation` and resolves itself.
    Cooperative,
}

/// Transport-independent history compaction extension point.
///
/// Implementors may summarize through a model or apply another host policy, but
/// must return complete provider-neutral replacement history and cooperate with
/// cancellation. Session history mutation remains owned by the SDK.
pub trait Compactor: Send + Sync {
    fn compact<'a>(&'a self, request: CompactionRequest) -> CompactionFuture<'a>;

    /// Describes whether the compactor resolves its future after request cancellation.
    fn cancellation_mode(&self) -> CompactorCancellationMode {
        CompactorCancellationMode::External
    }
}

/// Cause of a compaction operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CompactionTrigger {
    Automatic,
    Manual,
}

/// Typed result of manual or automatic compaction.
#[derive(Clone)]
pub struct CompactionOutcome {
    previous_messages: usize,
    current_messages: usize,
    previous_tokens: u64,
    current_tokens: u64,
    cost_usd_micros: Option<u64>,
    revision: Revision,
    committed_snapshot: Option<Box<crate::SessionSnapshot>>,
}

impl std::fmt::Debug for CompactionOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CompactionOutcome")
            .field("previous_messages", &self.previous_messages)
            .field("current_messages", &self.current_messages)
            .field("previous_tokens", &self.previous_tokens)
            .field("current_tokens", &self.current_tokens)
            .field("cost_usd_micros", &self.cost_usd_micros)
            .field("revision", &self.revision)
            .finish_non_exhaustive()
    }
}

impl PartialEq for CompactionOutcome {
    fn eq(&self, other: &Self) -> bool {
        self.previous_messages == other.previous_messages
            && self.current_messages == other.current_messages
            && self.previous_tokens == other.previous_tokens
            && self.current_tokens == other.current_tokens
            && self.cost_usd_micros == other.cost_usd_micros
            && self.revision == other.revision
            && match (&self.committed_snapshot, &other.committed_snapshot) {
                (None, None) => true,
                (Some(left), Some(right)) => {
                    left.session_id() == right.session_id() && left.revision() == right.revision()
                }
                _ => false,
            }
    }
}

impl Eq for CompactionOutcome {}

impl CompactionOutcome {
    pub(crate) fn new(
        previous_messages: usize,
        current_messages: usize,
        previous_tokens: u64,
        current_tokens: u64,
        cost_usd_micros: Option<u64>,
        revision: Revision,
    ) -> Self {
        Self {
            previous_messages,
            current_messages,
            previous_tokens,
            current_tokens,
            cost_usd_micros,
            revision,
            committed_snapshot: None,
        }
    }

    pub(crate) fn with_committed_snapshot(mut self, snapshot: crate::SessionSnapshot) -> Self {
        self.committed_snapshot = Some(Box::new(snapshot));
        self
    }

    /// Exact committed state for an automatic compaction event.
    pub fn committed_snapshot(&self) -> Option<&crate::SessionSnapshot> {
        self.committed_snapshot.as_deref()
    }

    pub fn previous_messages(&self) -> usize {
        self.previous_messages
    }

    pub fn current_messages(&self) -> usize {
        self.current_messages
    }

    /// Estimated context tokens present before this compaction installed.
    pub fn previous_tokens(&self) -> u64 {
        self.previous_tokens
    }

    /// Estimated context tokens present after this compaction installed.
    pub fn current_tokens(&self) -> u64 {
        self.current_tokens
    }

    /// Estimated tokens removed by this compaction.
    pub fn removed_tokens(&self) -> u64 {
        self.previous_tokens.saturating_sub(self.current_tokens)
    }

    /// Optional cost charged by the host-supplied compactor for this operation.
    pub fn cost_usd_micros(&self) -> Option<u64> {
        self.cost_usd_micros
    }

    pub fn revision(&self) -> Revision {
        self.revision
    }
}

/// Deterministic compactor for downstream tests and examples.
#[derive(Clone)]
pub struct ScriptedCompactor {
    outputs: Arc<Mutex<Vec<Result<CompactionOutput, String>>>>,
}

impl ScriptedCompactor {
    pub fn new(outputs: impl IntoIterator<Item = CompactionOutput>) -> Self {
        let mut outputs = outputs.into_iter().map(Ok).collect::<Vec<_>>();
        outputs.reverse();
        Self {
            outputs: Arc::new(Mutex::new(outputs)),
        }
    }

    pub fn failing(message: impl Into<String>) -> Self {
        Self {
            outputs: Arc::new(Mutex::new(vec![Err(message.into())])),
        }
    }
}

impl Compactor for ScriptedCompactor {
    fn compact<'a>(&'a self, request: CompactionRequest) -> CompactionFuture<'a> {
        Box::pin(async move {
            if request.cancellation.is_cancelled() {
                return Err(Error::Cancelled);
            }
            match self
                .outputs
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .pop()
            {
                Some(Ok(output)) => Ok(output),
                Some(Err(message)) => Err(Error::Provider(crate::ProviderError::new(
                    crate::ProviderErrorKind::Other,
                    message,
                    crate::Retryability::Permanent,
                ))),
                None => Err(Error::InvalidHostResponse {
                    message: "scripted compactor has no remaining output".into(),
                }),
            }
        })
    }
}

impl fmt::Debug for ScriptedCompactor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScriptedCompactor")
            .finish_non_exhaustive()
    }
}

pub(crate) fn estimate_history_tokens(messages: &[Message]) -> u64 {
    estimate_messages_tokens(messages)
}

#[cfg(test)]
#[path = "compaction_tests.rs"]
mod tests;
