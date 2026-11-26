//! [`super::SessionReport::ProviderContext`]: what of a conversation its
//! model cannot be sent.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::methods::ModelRef;

/// The provider-native context blocks in the session's conversation that
/// its model cannot be sent, because another model produced them. A
/// frontend asks before a resume or a prompt sends the conversation
/// without them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionProviderContextReport {
    /// The model the session's next request goes to.
    pub target: ModelRef,
    /// How many blocks `target` cannot use; 0 when nothing is left out.
    pub omitted_blocks: u64,
    /// The provider's kinds of those blocks, sorted, each once.
    pub omitted_kinds: Vec<String>,
    /// One id per distinct omitted block, sorted, so a frontend can tell
    /// whether a later report omits a block an earlier one did not.
    pub omitted_block_ids: Vec<ProviderContextBlockId>,
    /// The model that produced the first omitted block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ModelRef>,
    /// Whether compacting would summarize any of the conversation first.
    /// Compacting does not make native blocks sendable.
    pub can_compact: bool,
    /// Whether the session may switch to `source` on an auth it has
    /// credentials for.
    pub source_selectable: bool,
}

/// Identifies one provider-native block within one daemon build: compare
/// ids from the same daemon only. A collision could only skip or repeat a
/// warning, never change what is sent.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct ProviderContextBlockId(pub u64);
