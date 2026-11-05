//! Opaque provider-native context and the targets it may replay to.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{legacy_history, ModelIdentity};

pub(super) const PORTABLE_FALLBACK_CONTEXT_KIND: &str = "rho.sdk.portable_fallback.v1";

/// Which request targets may receive a [`ProviderContextBlock`] as
/// provider-native context. The producing adapter chooses it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ReplayScope {
    /// Only the exact producing provider, API, and model.
    #[default]
    ExactModel,
    /// Any model of the producing provider and API, for formats the API
    /// accepts across model names (for example Codex Responses compaction
    /// items and encrypted reasoning).
    ProviderApi,
    /// Never replayed: a provider note, bookkeeping the producing provider
    /// reads back from history for its own later requests (checking identity
    /// itself). Never counted as a handoff omission.
    Never,
}

/// Opaque provider-native data tagged with its producing model identity.
///
/// Serialization omits the default [`ReplayScope::ExactModel`]. Blocks saved
/// by rho-sdk 5.x carry no scope; loading derives it from their old encodings
/// (provider-note kind prefix, Codex cross-model formats).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "ProviderContextBlockRepr")]
pub struct ProviderContextBlock {
    pub identity: ModelIdentity,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<usize>,
    pub data: Value,
    /// Always serialized: a missing field means a 5.x block, which the
    /// deserializer upgrades.
    pub replay: ReplayScope,
}

impl ProviderContextBlock {
    /// Whether the target can replay this native format without rewriting it.
    ///
    /// Use this for both omission reporting and request filtering.
    pub fn is_replayable_to(&self, target: &ModelIdentity) -> bool {
        if self.is_portable_fallback()
            || self.identity.provider != target.provider
            || self.identity.api != target.api
        {
            return false;
        }
        match self.replay {
            ReplayScope::ExactModel => self.identity.model == target.model,
            ReplayScope::ProviderApi => true,
            ReplayScope::Never => false,
        }
    }

    pub(crate) fn is_portable_fallback(&self) -> bool {
        self.kind == PORTABLE_FALLBACK_CONTEXT_KIND
            && self.position.is_none()
            && self.data.is_string()
    }

    /// Portable fallback or provider note: never replayed as provider-native
    /// context, never counted as a handoff omission.
    pub(crate) fn is_sdk_metadata(&self) -> bool {
        self.is_portable_fallback() || self.replay == ReplayScope::Never
    }
}

#[derive(Deserialize)]
struct ProviderContextBlockRepr {
    identity: ModelIdentity,
    kind: String,
    #[serde(default)]
    position: Option<usize>,
    data: Value,
    #[serde(default)]
    replay: Option<ReplayScope>,
}

impl From<ProviderContextBlockRepr> for ProviderContextBlock {
    fn from(repr: ProviderContextBlockRepr) -> Self {
        let mut block = Self {
            identity: repr.identity,
            kind: repr.kind,
            position: repr.position,
            data: repr.data,
            replay: repr.replay.unwrap_or_default(),
        };
        if repr.replay.is_none() {
            legacy_history::upgrade_provider_context(&mut block);
        }
        block
    }
}
