//! Token-only compaction diagnostics. Never retain prompts or provider payloads.
use rho_sdk::{CompactionDecision, ContextEstimate};
use serde::Serialize;

use crate::compaction::CompactionConfig;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CompactionContext {
    pub enabled: bool,
    pub context_tokens: u64,
    pub estimate: ContextEstimate,
    pub context_window: Option<u64>,
    pub threshold_tokens: Option<u64>,
    pub target_tokens: Option<u64>,
    pub estimated_target_tokens: Option<u64>,
}

impl CompactionContext {
    pub(crate) fn new(
        estimate: ContextEstimate,
        window: Option<u64>,
        config: &CompactionConfig,
    ) -> Self {
        let target_tokens = window.map(|window| config.target_tokens(window));
        Self {
            enabled: config.auto_compact,
            context_tokens: estimate.tokens(),
            estimate,
            context_window: window,
            threshold_tokens: window.and_then(|window| config.threshold_tokens(window)),
            target_tokens,
            estimated_target_tokens: target_tokens.map(|target| estimate.estimated_budget(target)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IdleCompactionReason {
    Disabled,
    UnknownContextWindow,
    SessionBusy,
    BelowThreshold,
    NoCompactableHistory,
    Ready,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct IdleCompactionCheck {
    pub context: CompactionContext,
    pub reason: IdleCompactionReason,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ProviderCompactionCheck {
    pub context_tokens: u64,
    #[serde(flatten)]
    pub decision: CompactionDecision,
}

impl From<CompactionDecision> for ProviderCompactionCheck {
    fn from(decision: CompactionDecision) -> Self {
        Self {
            context_tokens: decision.estimate().tokens(),
            decision,
        }
    }
}

/// Which compaction tier produced the last compactor result.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct CompactionTierReport {
    pub tier: CompactionTier,
    /// Tool results replaced with recall stubs before this tier ran.
    pub elided_tool_results: usize,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CompactionDiagnostics {
    pub completed: rho_sdk::CompactionState,
    pub current: CompactionContext,
    pub last_idle_check: Option<IdleCompactionCheck>,
    pub last_provider_check: Option<ProviderCompactionCheck>,
    pub last_tier: Option<CompactionTierReport>,
}
