//! What of a conversation its model cannot be sent: the provider-native
//! context blocks another model produced.
//!
//! A daemon session reports it (`SessionReport::ProviderContext`), so the
//! TUI that follows one asks before a resume or a send omits the blocks.

use std::collections::BTreeSet;
use std::hash::{DefaultHasher, Hash, Hasher};

use rho_sdk::model::{
    handoff::{self, HandoffReport},
    Message, ModelIdentity, ProviderContextBlock,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ProviderContextReport {
    /// The model the session's next request goes to.
    pub(crate) target: ModelIdentity,
    /// How many blocks `target` cannot use.
    pub(crate) omitted: usize,
    /// The kinds of those blocks, sorted, each once.
    pub(crate) omitted_kinds: Vec<String>,
    /// One fingerprint per distinct omitted block, which says whether a
    /// later report omits a block this one did not.
    pub(crate) blocks: BTreeSet<u64>,
    /// The model that produced the first omitted block.
    pub(crate) source: Option<ModelIdentity>,
    /// Whether compacting would summarize any of the conversation.
    pub(crate) can_compact: bool,
    /// Whether the session may switch to `source` on an auth it has
    /// credentials for. A daemon of an older build does not say, and its
    /// TUI does not offer the switch.
    #[serde(default)]
    pub(crate) source_selectable: bool,
}

impl ProviderContextReport {
    /// The report on `history` for `target`, where `selectable` says
    /// whether the session may switch to a model.
    pub(crate) fn new(
        history: &[Message],
        target: ModelIdentity,
        can_compact: bool,
        selectable: impl FnOnce(&ModelIdentity) -> bool,
    ) -> Self {
        let omissions = handoff::report_message_omissions(history, &target);
        let mut omitted = handoff::omitted_message_context(history, &target).peekable();
        let source = omitted.peek().map(|block| block.identity.clone());
        let blocks = omitted.map(block_fingerprint).collect();
        let source_selectable = source.as_ref().is_some_and(selectable);
        Self {
            target,
            omitted: omissions.omitted_provider_context,
            omitted_kinds: omissions.omitted_kinds,
            blocks,
            source,
            can_compact,
            source_selectable,
        }
    }

    /// The omissions as a model handoff reports them.
    pub(crate) fn omissions(&self) -> HandoffReport {
        HandoffReport {
            omitted_provider_context: self.omitted,
            omitted_kinds: self.omitted_kinds.clone(),
        }
    }
}

/// Identity of one native block within one rho build: the TUI and the daemon
/// it follows run the same one. A collision, or a daemon of another build,
/// could only skip or repeat a warning, never change what is sent.
pub(crate) fn block_fingerprint(block: &ProviderContextBlock) -> u64 {
    let mut hasher = DefaultHasher::new();
    block.identity.hash(&mut hasher);
    block.kind.hash(&mut hasher);
    block.position.hash(&mut hasher);
    block.data.to_string().hash(&mut hasher);
    hasher.finish()
}
