use std::{
    fmt,
    future::Future,
    num::{NonZeroU64, NonZeroUsize},
    pin::Pin,
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};

use crate::{model::Message, CancellationToken, Error, Revision};

/// Persisted continuation state for prior compactions.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionState {
    completed_compactions: u64,
    removed_messages: u64,
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
            last_revision,
        }
    }

    pub fn completed_compactions(&self) -> u64 {
        self.completed_compactions
    }

    pub fn removed_messages(&self) -> u64 {
        self.removed_messages
    }

    pub fn last_revision(&self) -> Option<Revision> {
        self.last_revision
    }

    pub(crate) fn record(&mut self, removed_messages: usize, revision: Revision) {
        self.completed_compactions = self.completed_compactions.saturating_add(1);
        self.removed_messages = self
            .removed_messages
            .saturating_add(removed_messages as u64);
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
}

impl CompactionRequest {
    pub(crate) fn new(messages: Vec<Message>, cancellation: CancellationToken) -> Self {
        Self {
            messages,
            cancellation,
        }
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }
}

/// Valid replacement history produced by a compaction transport.
#[derive(Clone, Debug, PartialEq)]
pub struct CompactionOutput {
    messages: Vec<Message>,
}

impl CompactionOutput {
    pub fn new(messages: Vec<Message>) -> Result<Self, Error> {
        if messages.is_empty() {
            return Err(Error::InvalidHostResponse {
                message: "compaction replacement history must not be empty".into(),
            });
        }
        Ok(Self { messages })
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn into_messages(self) -> Vec<Message> {
        self.messages
    }
}

/// Future returned by [`Compactor`] implementations.
pub type CompactionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<CompactionOutput, Error>> + Send + 'a>>;

/// Transport-independent history compaction extension point.
///
/// Implementors may summarize through a model or apply another host policy, but
/// must return complete provider-neutral replacement history and cooperate with
/// cancellation. Session history mutation remains owned by the SDK.
pub trait Compactor: Send + Sync {
    fn compact<'a>(&'a self, request: CompactionRequest) -> CompactionFuture<'a>;
}

/// Cause of a compaction operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CompactionTrigger {
    Automatic,
    Manual,
}

/// Typed result of manual or automatic compaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompactionOutcome {
    previous_messages: usize,
    current_messages: usize,
    revision: Revision,
}

impl CompactionOutcome {
    pub(crate) fn new(
        previous_messages: usize,
        current_messages: usize,
        revision: Revision,
    ) -> Self {
        Self {
            previous_messages,
            current_messages,
            revision,
        }
    }

    pub fn previous_messages(&self) -> usize {
        self.previous_messages
    }

    pub fn current_messages(&self) -> usize {
        self.current_messages
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

#[cfg(test)]
#[path = "compaction_tests.rs"]
mod tests;
