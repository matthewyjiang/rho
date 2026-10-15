//! Keeps the provider prompt cache warm while the model idles on a
//! `process start` that asked for `keep_warm`.
//!
//! Policy lives here; the request itself is built by
//! `InteractiveRuntime::cache_warm_ping`. [`CacheWarmer`] is a small state
//! machine the idle loop steps: it pings once per TTL (minus a margin) after
//! the last request that read the cached prefix, pauses while a turn or
//! compaction owns the session, and halts on its budget, a missed ping, a ping
//! the host did not cap, or a failed one. The hold itself lives on the process
//! record, so it ends when the process exits or is stopped.

use std::time::{Duration, Instant};

use rho_providers::model::{ModelMetadata, ModelUsage};
use tokio::task::JoinError;

use super::{
    background_tasks::{OnCancel, TaskId, UiOutput},
    usage_cost::resolved_usage_cost_usd_micros,
    App, Entry, InteractiveRuntime, StatusSource,
};
use crate::app::interactive_runtime::{CacheWarmOutcome, PING_OUTPUT_TOKENS};

#[cfg(test)]
#[path = "cache_warm_tests.rs"]
mod tests;

/// Lead time before TTL expiry at which a ping is sent.
///
/// Receipt: five capped cached pings through the Responses route took
/// 2.54-4.76 s end to end (`/tmp/spike_latency.log`). 30 s covers six times the
/// slowest round trip plus idle-loop wake latency.
pub(super) const PING_MARGIN: Duration = Duration::from_secs(30);

/// Pings per idle stretch when the model's cache prices are unknown.
///
/// Receipt: [`ping_budget`] at Anthropic's Opus/Sonnet list prices (cache read
/// 0.1x input, 5m cache write 1.25x): (1.25 - 0.1) / (2 * 0.1) = 5.
pub(super) const FALLBACK_PING_BUDGET: u32 = 5;

/// Pings one idle stretch (between real requests) may spend.
///
/// A ping re-reads the prefix at the cache-read price. Letting the cache lapse
/// makes the next turn rewrite it at the cache-write price (the input price on
/// hosts with no write premium) instead of reading it. Warming breaks even at
/// `(write - read) / read` pings; the budget is half that, so the worst case
/// costs half of one avoided miss. GPT-5.6 lands at 5, GPT-5.5 (no write
/// premium) at 4, and GPT-6.1 Sol (0.05x reads) at 12. Unknown read prices use
/// [`FALLBACK_PING_BUDGET`].
pub(super) fn ping_budget(metadata: Option<&ModelMetadata>) -> u32 {
    let prices = metadata
        .and_then(|metadata| metadata.cost_default)
        .and_then(|cost| {
            let read = cost.cache_read_micros_per_m.filter(|read| *read > 0)?;
            let write = cost.cache_write_micros_per_m.or(cost.input_micros_per_m)?;
            Some((read, write))
        });
    match prices {
        Some((read, write)) => {
            u32::try_from(write.saturating_sub(read) / read.saturating_mul(2)).unwrap_or(u32::MAX)
        }
        None => FALLBACK_PING_BUDGET,
    }
}

/// The route's cache lifetime and the current model's ping budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct WarmLimits {
    pub(super) ttl: Duration,
    pub(super) max_pings: u32,
}

/// What the idle loop allows the warmer to do right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum WarmGate {
    /// No live hold, or the user turned warming off.
    #[default]
    Released,
    /// A turn or compaction owns the session; its own request refreshes the cache.
    Paused,
    /// A hold exists but the model route cannot send a capped request.
    Unsupported,
    Holding,
}

/// Why warming stopped for the current hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WarmHalt {
    Budget,
    Miss,
    /// The host's usage did not confirm the ping's output cap.
    Uncapped,
    /// The model's cache prices give a zero ping budget.
    Unprofitable,
    Unsupported,
    Failed,
}

/// The idle loop's next action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WarmStep {
    Rest,
    /// Nothing to do until `next_wake()`.
    Waiting,
    Ping,
    /// Warming just stopped; report it once.
    Halted(WarmHalt),
}

/// How one ping went, judged from its reported usage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PingResult {
    /// Read the cached prefix; the TTL restarts.
    Read,
    /// Too much of the prefix was uncached or rewritten to repeat the ping.
    Miss,
    /// Read the prefix, but cannot prove the host applied the output cap, so
    /// repeating it may bill a full response each time.
    Uncapped,
    /// Failed before reporting usage.
    Failed,
}

impl PingResult {
    /// A capped ping usually ends as an `incomplete` error after reporting
    /// usage, so usage decides before the error does.
    pub(super) fn classify(outcome: &CacheWarmOutcome) -> Self {
        let usage = &outcome.usage;
        // Use the measured cache-breakpoint noise floor, not any nonzero hit:
        // a short cached head with a large rewritten suffix is still a miss.
        let read_prefix = usage.cache_read_tokens.is_some_and(|read| {
            read > 0
                && usage.inclusive_prompt_tokens().is_some_and(|prompt| {
                    prompt.saturating_sub(read) <= super::cache_stats::CACHE_MISS_NOISE_FLOOR_TOKENS
                })
        });
        // A custom host may accept the cap field and ignore it, so only
        // reported output within the cap proves it.
        let capped = usage
            .output_tokens
            .is_some_and(|output| output <= u64::from(PING_OUTPUT_TOKENS.get()));
        if read_prefix && capped {
            Self::Read
        } else if read_prefix {
            Self::Uncapped
        } else if outcome.error.is_some()
            && usage.cache_read_tokens.is_none()
            && usage.cache_write_tokens.is_none()
        {
            Self::Failed
        } else {
            // Includes a route that reports no cache accounting: a ping that
            // cannot prove it read the cache is not worth repeating.
            Self::Miss
        }
    }
}

#[derive(Default)]
pub(super) struct CacheWarmer {
    pings: u32,
    in_flight: bool,
    /// Bumped whenever an in-flight ping stops describing the current hold
    /// or route. A finished ping from an older generation still counts its
    /// spend but cannot change scheduling.
    generation: u64,
    halt: Option<WarmHalt>,
    wake: Option<Instant>,
    gate: WarmGate,
}

impl CacheWarmer {
    /// The deadline the idle loop should wake for, if any.
    pub(super) fn next_wake(&self) -> Option<Instant> {
        self.wake
    }

    /// A real request established the prefix; a new idle stretch
    /// begins with a fresh budget. A miss, an uncapped ping, or a zero budget
    /// stays sticky until the hold ends, since the next stretch would repeat it.
    pub(super) fn record_request(&mut self) {
        self.pings = 0;
        if matches!(self.halt, Some(WarmHalt::Budget | WarmHalt::Failed)) {
            self.halt = None;
        }
    }

    /// New conversation or new route: drop scheduler state and orphan any
    /// in-flight ping.
    pub(super) fn reset(&mut self) {
        *self = Self {
            generation: self.generation.wrapping_add(1),
            ..Self::default()
        };
    }

    /// Generation a ping started now belongs to.
    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether a ping from `generation` still describes the current hold.
    pub(super) fn is_current(&self, generation: u64) -> bool {
        self.generation == generation
    }

    pub(super) fn step(
        &mut self,
        now: Instant,
        gate: WarmGate,
        last_prefix_read: Option<Instant>,
        limits: WarmLimits,
    ) -> WarmStep {
        let WarmLimits { ttl, max_pings } = limits;
        self.wake = None;
        self.gate = gate;
        match gate {
            WarmGate::Released => {
                // An in-flight ping belongs to the ended hold: orphan it so
                // its result cannot halt or block a later hold.
                if self.in_flight {
                    self.in_flight = false;
                    self.generation = self.generation.wrapping_add(1);
                }
                self.pings = 0;
                self.halt = None;
                return WarmStep::Rest;
            }
            WarmGate::Paused => return WarmStep::Rest,
            WarmGate::Unsupported => return self.halt_once(WarmHalt::Unsupported),
            WarmGate::Holding => {}
        }
        if self.in_flight || self.halt.is_some() {
            return WarmStep::Rest;
        }
        if max_pings == 0 {
            // Report it now rather than showing a 0/0 counter until the first
            // ping is due; sticky so each real request does not repeat it.
            return self.halt_once(WarmHalt::Unprofitable);
        }
        let Some(read_at) = last_prefix_read else {
            return WarmStep::Rest;
        };
        if now >= read_at + ttl {
            // Already cold: a ping would only pay to rewrite it.
            return WarmStep::Rest;
        }
        // A TTL shorter than twice the margin still waits half of it, so a
        // tiny configured TTL cannot ping back to back.
        let due = read_at + ttl.saturating_sub(PING_MARGIN).max(ttl / 2);
        if now < due {
            self.wake = Some(due);
            return WarmStep::Waiting;
        }
        if self.pings >= max_pings {
            return self.halt_once(WarmHalt::Budget);
        }
        self.in_flight = true;
        WarmStep::Ping
    }

    /// Applies a finished ping; returns a new halt to report.
    pub(super) fn ping_finished(&mut self, result: PingResult) -> Option<WarmHalt> {
        self.in_flight = false;
        match result {
            PingResult::Read => {
                self.pings += 1;
                None
            }
            PingResult::Miss => self.set_halt(WarmHalt::Miss),
            PingResult::Uncapped => self.set_halt(WarmHalt::Uncapped),
            PingResult::Failed => self.set_halt(WarmHalt::Failed),
        }
    }

    fn halt_once(&mut self, halt: WarmHalt) -> WarmStep {
        match self.set_halt(halt) {
            Some(halt) => WarmStep::Halted(halt),
            None => WarmStep::Rest,
        }
    }

    fn set_halt(&mut self, halt: WarmHalt) -> Option<WarmHalt> {
        if self.halt == Some(halt) {
            return None;
        }
        self.halt = Some(halt);
        Some(halt)
    }

    fn status_text(
        &self,
        now: Instant,
        last_prefix_read: Option<Instant>,
        limits: WarmLimits,
    ) -> Option<String> {
        (self.gate == WarmGate::Holding
            && self.halt.is_none()
            && last_prefix_read.is_some_and(|read| now < read + limits.ttl))
        .then(|| format!("warming cache · {}/{}", self.pings, limits.max_pings))
    }
}

fn halt_notice(halt: WarmHalt, detail: Option<&str>, max_pings: u32) -> String {
    match halt {
        WarmHalt::Budget => format!(
            "cache warm stopped after {max_pings} pings; the prompt cache will expire"
        ),
        WarmHalt::Miss => "cache warm stopped: a ping missed the prompt cache".to_string(),
        WarmHalt::Unprofitable => {
            "cache warm skipped: at this model's cache prices a ping costs more than the miss it prevents"
                .to_string()
        }
        WarmHalt::Uncapped => {
            "cache warm stopped: the host did not confirm the ping's output cap".to_string()
        }
        WarmHalt::Unsupported => {
            "cache warm unavailable: this model route cannot send capped requests".to_string()
        }
        WarmHalt::Failed => format!(
            "could not warm prompt cache: {}",
            detail.unwrap_or("request failed")
        ),
    }
}

impl App {
    /// Idle-loop step: start a due ping or report a halt. Returns whether the
    /// screen changed.
    pub(super) fn poll_cache_warm(&mut self, agent: &InteractiveRuntime) -> bool {
        let gate = if !self.info.runtime.keep_cache_warm || !agent.has_keep_warm_hold() {
            WarmGate::Released
        } else if agent.is_session_busy() || self.is_ui_busy() {
            WarmGate::Paused
        } else if !agent.supports_cache_warm() {
            WarmGate::Unsupported
        } else {
            WarmGate::Holding
        };
        let limits = self.cache_warm_limits();
        let changed = match self.usage.cache_warm.step(
            Instant::now(),
            gate,
            self.usage.cache_stats.last_prefix_read(),
            limits,
        ) {
            WarmStep::Rest | WarmStep::Waiting => false,
            WarmStep::Ping => {
                let ping = agent.cache_warm_ping();
                self.spawn_cache_warm_ping(async move {
                    match ping {
                        Ok(ping) => ping.send().await,
                        Err(error) => CacheWarmOutcome {
                            usage: ModelUsage::default(),
                            error: Some(error.to_string()),
                        },
                    }
                });
                false
            }
            WarmStep::Halted(halt) => {
                self.insert_entry(&Entry::Notice(halt_notice(halt, None, limits.max_pings)));
                true
            }
        };
        self.sync_cache_warm_status() || changed
    }

    /// Tracks a ping as run-to-completion. The provider bills the prefix read
    /// once it accepts the request, so a reset never cancels the send; it
    /// still records that usage in the ledger.
    fn spawn_cache_warm_ping(
        &mut self,
        ping: impl std::future::Future<Output = CacheWarmOutcome> + Send + 'static,
    ) {
        let sent_at = Instant::now();
        let generation = self.usage.cache_warm.generation();
        self.tasks.track(
            TaskId::CacheWarm,
            tokio::spawn(ping),
            OnCancel::RunToCompletion,
            move |result| {
                UiOutput::CacheWarm {
                    generation,
                    sent_at,
                    result,
                }
                .into()
            },
        );
    }

    /// Applies a finished ping. Its spend always counts toward this
    /// conversation; an orphaned ping changes nothing else. Returns whether
    /// the screen changed.
    pub(super) fn apply_cache_warm(
        &mut self,
        generation: u64,
        sent_at: Instant,
        result: Result<CacheWarmOutcome, JoinError>,
    ) -> bool {
        let outcome = result.unwrap_or_else(|error| CacheWarmOutcome {
            usage: ModelUsage::default(),
            error: Some(error.to_string()),
        });
        if let Some(cost) =
            resolved_usage_cost_usd_micros(&outcome.usage, self.model_metadata.as_ref())
        {
            self.usage.extra.cache_warm = self.usage.extra.cache_warm.saturating_add(cost);
        }
        if !self.usage.cache_warm.is_current(generation) {
            return false;
        }
        let result = PingResult::classify(&outcome);
        if result == PingResult::Read {
            self.usage.cache_stats.record_prefix_read(sent_at);
        }
        let changed = match self.usage.cache_warm.ping_finished(result) {
            None => false,
            Some(halt) => {
                let max_pings = self.cache_warm_limits().max_pings;
                self.insert_entry(&Entry::Notice(halt_notice(
                    halt,
                    outcome.error.as_deref(),
                    max_pings,
                )));
                true
            }
        };
        self.sync_cache_warm_status() || changed
    }

    /// Drops warming state for a new conversation and stops observing any
    /// in-flight ping. The ping still finishes, so its spend reaches the
    /// usage ledger, but neither its cost nor its result touches the new
    /// conversation, whose totals `reset_usage` just cleared.
    pub(super) fn reset_cache_warm(&mut self) {
        self.usage.cache_warm.reset();
        self.tasks.abort(|id| matches!(id, TaskId::CacheWarm));
        self.sync_cache_warm_status();
    }

    /// The bound provider changed. An in-flight ping describes the old route,
    /// so orphan it (its spend still counts in this conversation and the
    /// ledger), and wait for a real request on the new route before warming
    /// again.
    pub(super) fn cache_warm_provider_replaced(&mut self) {
        self.usage.cache_stats.forget_prefix_read();
        self.usage.cache_warm.reset();
        self.sync_cache_warm_status();
    }

    /// A reasoning-only provider rebuild. Where effort changes keep the cached
    /// prefix, warming carries on; elsewhere effort is part of the prefix and
    /// this resets like a model switch.
    pub(super) fn cache_warm_reasoning_changed(&mut self, agent: &InteractiveRuntime) {
        if !rho_providers::providers::openai::reasoning_change_preserves_prefix(
            &agent.provider_identity(),
        ) {
            self.cache_warm_provider_replaced();
        }
    }

    fn cache_warm_limits(&self) -> WarmLimits {
        WarmLimits {
            ttl: self.info.runtime.prompt_cache_ttl,
            max_pings: ping_budget(self.model_metadata.as_ref()),
        }
    }

    /// Shows the ping counter unless another status is up. Returns whether
    /// the status changed.
    fn sync_cache_warm_status(&mut self) -> bool {
        let Some(status) = self.usage.cache_warm.status_text(
            Instant::now(),
            self.usage.cache_stats.last_prefix_read(),
            self.cache_warm_limits(),
        ) else {
            return self.clear_cache_warm_status();
        };
        let owned = self.status_source == StatusSource::CacheWarm;
        if !owned && !self.status().is_empty() {
            return false;
        }
        if owned && self.status() == status {
            return false;
        }
        self.set_status_quiet(status);
        self.status_source = StatusSource::CacheWarm;
        true
    }

    /// Clears the ping counter if it is still the visible status.
    fn clear_cache_warm_status(&mut self) -> bool {
        if self.status_source != StatusSource::CacheWarm {
            return false;
        }
        self.set_status_quiet("");
        true
    }
}
