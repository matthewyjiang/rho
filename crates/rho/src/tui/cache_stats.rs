//! Prompt-cache miss detection and session re-bill totals for the interactive TUI.
//!
//! One completed model call is one sample. The SDK already names that boundary
//! with `rho_sdk::RunEvent::ModelCallCompleted`, which it emits only
//! for the attempt that actually returned output, so failed and retried
//! attempts never reach this tracker and no retry bookkeeping is needed here.
//!
//! Each sample is compared to the previous one: prompt tokens that the previous
//! request already established as a prefix, but that this request did not read
//! from cache, were re-billed. Feature policy stays here: `/info` reads
//! [`CacheStatsTracker::rebilled`], and completed turns drain
//! [`CacheMissNotice`]s when the user opts into notices.

use std::time::{Duration, Instant};

use rho_providers::model::{ModelMetadata, ModelUsage};
use rho_sdk::{ModelCallMetrics, ModelCallProfile};

use super::usage_cost::{cost_component, format_token_count, format_usd};

/// Misses at or below this are cache-breakpoint granularity, not a real miss.
///
/// Receipt: Pi `packages/coding-agent/src/core/cache-stats.ts` uses 1024
/// because Anthropic cache breakpoints sit on ~1K-token alignment. Smaller
/// gaps are noise.
pub(super) const CACHE_MISS_NOISE_FLOOR_TOKENS: u64 = 1024;

/// Token tripwire for a transcript notice after a counted miss.
///
/// Receipt: Pi's significant-miss notice threshold. Initial Rho value until we
/// measure real sessions; treat as a named tripwire, not a guess in call sites.
pub(super) const SIGNIFICANT_MISS_TOKENS: u64 = 20_000;

/// Extra-cost tripwire for a transcript notice after a counted miss ($0.10).
///
/// Receipt: Pi's alternative notice threshold (`missedCost >= 0.1`). Stored in
/// USD micros to match [`ModelUsage::cost_usd_micros`].
pub(super) const SIGNIFICANT_MISS_EXTRA_COST_USD_MICROS: u64 = 100_000;

/// Idle gap that is worth naming as a likely TTL expiry.
///
/// Receipt: Anthropic's default prompt-cache TTL is 5 minutes. Used only to
/// attribute a cause, never to suppress a miss.
pub(super) const PROVIDER_CACHE_TTL_HINT: Duration = Duration::from_secs(300);

/// Session-level tokens and dollars re-billed by counted cache misses.
///
/// `/info` copies this snapshot and renders nothing while `miss_count` is zero,
/// so `extra_cost_usd_micros` needs no separate "unknown" state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct CacheRebilled {
    pub missed_tokens: u64,
    pub miss_count: u64,
    pub extra_cost_usd_micros: u64,
}

/// Why a counted miss happened, when the tracker can observe a cause.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CacheMissCause {
    ModelSwitch,
    Idle(Duration),
    Unattributed,
}

/// A significant miss ready to render as a transcript notice.
///
/// Drained at turn end. Only completed main-agent turns insert these.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CacheMissNotice {
    pub missed_tokens: u64,
    pub extra_cost_usd_micros: Option<u64>,
    pub cause: CacheMissCause,
}

/// Provider and model that served one completed request.
///
/// Taken from the SDK's [`ModelCallProfile`] so a model switch is named from
/// the model that actually served the call, not the currently selected one.
/// Reasoning level and service tier are deliberately excluded: they do not
/// change the prompt prefix.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ModelKey {
    provider: String,
    model: String,
}

impl ModelKey {
    fn from_profile(profile: &ModelCallProfile) -> Self {
        Self {
            provider: profile.provider.clone(),
            model: profile.model.clone(),
        }
    }
}

struct CompletedRequest {
    prompt_tokens: u64,
    model: ModelKey,
    completed_at: Instant,
}

/// In-memory miss detector for the main-agent request stream.
///
/// Holds the previous completed request, the usage delta reported for the
/// in-flight one, and the session re-bill totals shown by `/info`.
#[derive(Default)]
pub(super) struct CacheStatsTracker {
    previous: Option<CompletedRequest>,
    /// Latest per-step usage delta, consumed by the next completed model call.
    ///
    /// Taken (not copied) at record time so a request that never reported usage
    /// cannot be sampled twice from a stale delta.
    reported_usage: Option<ModelUsage>,
    rebilled: CacheRebilled,
    turn_notices: Vec<CacheMissNotice>,
}

impl CacheStatsTracker {
    /// Hold the latest per-step usage delta for the in-flight request.
    pub(super) fn usage_updated(&mut self, step_delta: &ModelUsage) {
        self.reported_usage = Some(step_delta.clone());
    }

    /// Sample one completed model call against the previous one.
    pub(super) fn record_request(
        &mut self,
        profile: &ModelCallProfile,
        metrics: ModelCallMetrics,
        metadata: Option<&ModelMetadata>,
        completed_at: Instant,
    ) {
        let Some(usage) = self.reported_usage.take() else {
            return;
        };
        let Some(prompt_tokens) = usage.total_input_tokens().filter(|tokens| *tokens > 0) else {
            return;
        };

        let model = ModelKey::from_profile(profile);
        if let Some(previous) = self.previous.take().filter(|_| reports_cache(&usage)) {
            let started_at = completed_at
                .checked_sub(metrics.total_latency)
                .unwrap_or(completed_at);
            self.count_miss(
                &usage,
                prompt_tokens,
                &model,
                started_at,
                &previous,
                metadata,
            );
        }

        self.previous = Some(CompletedRequest {
            prompt_tokens,
            model,
            completed_at,
        });
    }

    /// Compaction rewrites the prompt prefix, so the next request has nothing
    /// to hit. Session totals stay.
    pub(super) fn prompt_prefix_reset(&mut self) {
        self.previous = None;
        self.reported_usage = None;
    }

    /// Clear everything. Matches `/clear`, tree checkout, and new session.
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(super) fn take_turn_notices(&mut self) -> Vec<CacheMissNotice> {
        std::mem::take(&mut self.turn_notices)
    }

    pub(super) fn rebilled(&self) -> &CacheRebilled {
        &self.rebilled
    }

    fn count_miss(
        &mut self,
        usage: &ModelUsage,
        prompt_tokens: u64,
        model: &ModelKey,
        started_at: Instant,
        previous: &CompletedRequest,
        metadata: Option<&ModelMetadata>,
    ) {
        let cache_read = usage.cache_read_tokens.unwrap_or(0);
        // A shrunken prompt can only re-bill what it actually sent.
        let missed = previous
            .prompt_tokens
            .min(prompt_tokens)
            .saturating_sub(cache_read);
        if missed <= CACHE_MISS_NOISE_FLOOR_TOKENS {
            return;
        }

        let extra_cost = extra_cost_usd_micros(missed, prompt_tokens, metadata);
        self.rebilled.missed_tokens = self.rebilled.missed_tokens.saturating_add(missed);
        self.rebilled.miss_count = self.rebilled.miss_count.saturating_add(1);
        self.rebilled.extra_cost_usd_micros = self
            .rebilled
            .extra_cost_usd_micros
            .saturating_add(extra_cost.unwrap_or(0));

        if is_significant_miss(missed, extra_cost) {
            self.turn_notices.push(CacheMissNotice {
                missed_tokens: missed,
                extra_cost_usd_micros: extra_cost,
                cause: miss_cause(model, started_at, previous),
            });
        }
    }
}

/// Whether the provider reports prompt-cache accounting at all.
///
/// Field presence, not a positive count: a provider that never populates these
/// fields (local models, plain OpenAI-compatible hosts) reports zero cache
/// reads on every request and must not be billed a miss for it. A provider that
/// does report cache can legitimately send `Some(0)` on a genuine full miss.
fn reports_cache(usage: &ModelUsage) -> bool {
    usage.cache_read_tokens.is_some() || usage.cache_write_tokens.is_some()
}

fn miss_cause(
    model: &ModelKey,
    started_at: Instant,
    previous: &CompletedRequest,
) -> CacheMissCause {
    if model != &previous.model {
        return CacheMissCause::ModelSwitch;
    }
    let gap = started_at.saturating_duration_since(previous.completed_at);
    if gap >= PROVIDER_CACHE_TTL_HINT {
        CacheMissCause::Idle(gap)
    } else {
        CacheMissCause::Unattributed
    }
}

fn extra_cost_usd_micros(
    missed: u64,
    prompt_tokens: u64,
    metadata: Option<&ModelMetadata>,
) -> Option<u64> {
    let cost = metadata?.cost_for_input_tokens(prompt_tokens)?;
    let input = cost.input_micros_per_m?;
    let cache_read = cost.cache_read_micros_per_m?;
    let premium_per_m = input.saturating_sub(cache_read);
    Some(cost_component(missed, Some(premium_per_m)).min(u64::MAX as u128) as u64)
}

fn is_significant_miss(missed: u64, extra_cost: Option<u64>) -> bool {
    missed >= SIGNIFICANT_MISS_TOKENS
        || extra_cost.is_some_and(|cost| cost >= SIGNIFICANT_MISS_EXTRA_COST_USD_MICROS)
}

/// Transcript line for one significant miss. Used by the completed-turn path.
pub(super) fn notice_text(notice: &CacheMissNotice) -> String {
    let mut text = match notice.cause {
        CacheMissCause::ModelSwitch => "cache miss after model switch".to_string(),
        CacheMissCause::Idle(gap) => format!(
            "cache miss after {}m idle (cache TTL is about {}m)",
            whole_minutes(gap),
            whole_minutes(PROVIDER_CACHE_TTL_HINT),
        ),
        CacheMissCause::Unattributed => "cache miss".to_string(),
    };
    text.push_str(": ");
    text.push_str(&format_token_count(notice.missed_tokens));
    text.push_str(" tokens re-billed");
    if let Some(cost) = notice.extra_cost_usd_micros {
        text.push_str(" (~");
        text.push_str(&format_usd(cost));
        text.push(')');
    }
    text
}

fn whole_minutes(gap: Duration) -> u64 {
    (gap.as_secs().saturating_add(30) / 60).max(1)
}

#[cfg(test)]
#[path = "cache_stats_tests.rs"]
mod tests;
