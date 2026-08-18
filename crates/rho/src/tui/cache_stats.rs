//! Prompt-cache miss detection and session re-bill totals for the interactive TUI.
//!
//! The tracker consumes per-step usage deltas already computed by the TUI and
//! compares each completed request to the previous one. Feature policy stays
//! here: `/info` reads [`CacheStatsTracker::rebilled`], and completed turns
//! drain [`CacheMissNotice`]s when the user opts into notices.

use std::time::{Duration, Instant};

use rho_providers::model::{ModelMetadata, ModelUsage};

use super::usage_cost::{format_token_count, format_usd};

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

/// Provider and model that produced one committed request.
///
/// Compared across consecutive requests so a model switch can be named as a
/// miss cause without hooking the model-picker path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ModelKey {
    provider: String,
    model: String,
}

impl ModelKey {
    pub(super) fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
        }
    }
}

/// Session-level tokens and dollars re-billed by counted cache misses.
///
/// `/info` copies this snapshot. Cost stays `None` until at least one counted
/// miss had priced input and cache-read rates.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct CacheRebilled {
    pub missed_tokens: u64,
    pub miss_count: u64,
    pub extra_cost_usd_micros: Option<u64>,
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

struct CompletedRequest {
    prompt_tokens: u64,
    model: ModelKey,
    completed_at: Instant,
}

struct PendingSample {
    usage: ModelUsage,
    model: ModelKey,
    started_at: Instant,
    completed_at: Instant,
    retry_tainted: bool,
    metadata: Option<ModelMetadata>,
}

/// In-memory miss detector for the main-agent usage stream.
///
/// Owns the previous committed request, the in-flight step sample, and the
/// session re-bill totals shown by `/info`.
#[derive(Default)]
pub(super) struct CacheStatsTracker {
    previous: Option<CompletedRequest>,
    pending: Option<PendingSample>,
    /// Any request since the last reset reported cache read or write activity.
    segment_saw_cache: bool,
    rebilled: CacheRebilled,
    turn_notices: Vec<CacheMissNotice>,
}

impl CacheStatsTracker {
    /// Drop the in-flight sample and this run's notices. Call at `RunStarted`.
    pub(super) fn run_started(&mut self) {
        self.pending = None;
        self.turn_notices.clear();
    }

    /// Commit the previous step, then arm a sample for the new step's model.
    pub(super) fn step_started(
        &mut self,
        model: ModelKey,
        now: Instant,
        metadata: Option<&ModelMetadata>,
    ) {
        self.commit_pending();
        self.pending = Some(PendingSample {
            usage: ModelUsage::default(),
            model,
            started_at: now,
            completed_at: now,
            retry_tainted: false,
            metadata: metadata.cloned(),
        });
    }

    /// Replace the in-flight step's usage with the latest per-step delta.
    pub(super) fn usage_updated(&mut self, step_delta: &ModelUsage, now: Instant) {
        if let Some(pending) = &mut self.pending {
            pending.usage = step_delta.clone();
            pending.completed_at = now;
        }
    }

    /// Mark the in-flight sample so retry-merged token math is not counted.
    pub(super) fn attempt_restarted(&mut self) {
        if let Some(pending) = &mut self.pending {
            pending.retry_tainted = true;
        }
    }

    /// Commit the last step of a run. Call at every turn outcome.
    pub(super) fn run_finished(&mut self, metadata: Option<&ModelMetadata>, now: Instant) {
        if let Some(pending) = &mut self.pending {
            pending.completed_at = now;
            if metadata.is_some() {
                pending.metadata = metadata.cloned();
            }
        }
        self.commit_pending();
    }

    pub(super) fn take_turn_notices(&mut self) -> Vec<CacheMissNotice> {
        std::mem::take(&mut self.turn_notices)
    }

    pub(super) fn rebilled(&self) -> &CacheRebilled {
        &self.rebilled
    }

    /// Treat compaction as a new prompt prefix. Session totals stay.
    pub(super) fn compaction_reset(&mut self) {
        self.commit_pending();
        self.previous = None;
        self.pending = None;
        self.segment_saw_cache = false;
    }

    /// Clear everything. Matches `/clear`, tree checkout, and new session.
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    fn commit_pending(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        let Some(prompt_tokens) = pending
            .usage
            .total_input_tokens()
            .filter(|tokens| *tokens > 0)
        else {
            return;
        };

        let reports_cache = reports_cache_activity(&pending.usage);
        if let Some(previous) = &self.previous {
            let cache_read = pending.usage.cache_read_tokens.unwrap_or(0);
            let missed = previous
                .prompt_tokens
                .min(prompt_tokens)
                .saturating_sub(cache_read);
            let silent_uncached = !reports_cache && !self.segment_saw_cache;
            if missed > CACHE_MISS_NOISE_FLOOR_TOKENS && !pending.retry_tainted && !silent_uncached
            {
                let cause = miss_cause(&pending, previous);
                let extra_cost =
                    extra_cost_usd_micros(missed, prompt_tokens, pending.metadata.as_ref());
                self.rebilled.missed_tokens = self.rebilled.missed_tokens.saturating_add(missed);
                self.rebilled.miss_count = self.rebilled.miss_count.saturating_add(1);
                if let Some(extra) = extra_cost {
                    self.rebilled.extra_cost_usd_micros = Some(
                        self.rebilled
                            .extra_cost_usd_micros
                            .unwrap_or(0)
                            .saturating_add(extra),
                    );
                }
                if is_significant_miss(missed, extra_cost) {
                    self.turn_notices.push(CacheMissNotice {
                        missed_tokens: missed,
                        extra_cost_usd_micros: extra_cost,
                        cause,
                    });
                }
            }
        }

        self.segment_saw_cache |= reports_cache;
        self.previous = Some(CompletedRequest {
            prompt_tokens,
            model: pending.model,
            completed_at: pending.completed_at,
        });
    }
}

fn reports_cache_activity(usage: &ModelUsage) -> bool {
    usage.cache_read_tokens.unwrap_or(0) > 0 || usage.cache_write_tokens.unwrap_or(0) > 0
}

fn miss_cause(pending: &PendingSample, previous: &CompletedRequest) -> CacheMissCause {
    if pending.model != previous.model {
        return CacheMissCause::ModelSwitch;
    }
    let gap = pending
        .started_at
        .saturating_duration_since(previous.completed_at);
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
    let delta = input.saturating_sub(cache_read);
    Some((missed as u128 * delta as u128 / 1_000_000) as u64)
}

fn is_significant_miss(missed: u64, extra_cost: Option<u64>) -> bool {
    missed >= SIGNIFICANT_MISS_TOKENS
        || extra_cost.is_some_and(|cost| cost >= SIGNIFICANT_MISS_EXTRA_COST_USD_MICROS)
}

/// Transcript line for one significant miss. Used by the completed-turn path.
pub(super) fn notice_text(notice: &CacheMissNotice) -> String {
    let tokens = format_token_count(notice.missed_tokens);
    let mut text = match notice.cause {
        CacheMissCause::ModelSwitch => "cache miss after model switch".to_string(),
        CacheMissCause::Idle(gap) => {
            let minutes = idle_minutes(gap);
            format!("cache miss after {minutes}m idle (cache TTL is about 5m)")
        }
        CacheMissCause::Unattributed => "cache miss".to_string(),
    };
    text.push_str(": ");
    text.push_str(&tokens);
    text.push_str(" tokens re-billed");
    if let Some(cost) = notice.extra_cost_usd_micros {
        text.push_str(" (~");
        text.push_str(&format_usd(cost));
        text.push(')');
    }
    text
}

fn idle_minutes(gap: Duration) -> u64 {
    (gap.as_secs().saturating_add(30) / 60).max(1)
}

#[cfg(test)]
#[path = "cache_stats_tests.rs"]
mod tests;
