//! Keeps the provider prompt cache warm while the model idles on a
//! `process start` that asked for `keep_warm`.
//!
//! The policy both session hosts share. [`CacheWarmer`] is a small state
//! machine its host steps: it pings once per TTL (minus a margin) after the
//! last request that read the cached prefix, pauses while a turn or
//! compaction owns the session, and halts on its budget, a missed ping, a
//! ping the host did not cap, or a failed one. The hold itself lives on the
//! process record, so it ends when the process exits or is stopped.
//!
//! The host supplies the clock, the gate, and the prefix-read time, sends
//! each [`CacheWarmPing`], and shows [`WarmProgress`] and [`halt_notice`]:
//! the in-process TUI from its idle loop (`tui/cache_warm.rs`), the daemon
//! from a watcher per session (`app/daemon_host/sessions/cache_warm.rs`).

use std::time::{Duration, Instant};

use rho_providers::model::ModelMetadata;

mod ping;

pub(crate) use ping::{CacheWarmOutcome, CacheWarmPing, PING_OUTPUT_TOKENS};

#[cfg(test)]
#[path = "cache_warm_tests.rs"]
mod tests;

/// Misses at or below this are cache-breakpoint granularity, not a real miss.
///
/// Receipt: Pi `packages/coding-agent/src/core/cache-stats.ts` uses 1024
/// because Anthropic cache breakpoints sit on ~1K-token alignment. Smaller
/// gaps are noise.
pub(crate) const CACHE_MISS_NOISE_FLOOR_TOKENS: u64 = 1024;

/// Lead time before TTL expiry at which a ping is sent.
///
/// Receipt: five capped cached pings through the Responses route took
/// 2.54-4.76 s end to end (`/tmp/spike_latency.log`). 30 s covers six times the
/// slowest round trip plus idle-loop wake latency.
pub(crate) const PING_MARGIN: Duration = Duration::from_secs(30);

/// Pings per idle stretch when the model's cache prices are unknown.
///
/// Receipt: [`ping_budget`] at Anthropic's Opus/Sonnet list prices (cache read
/// 0.1x input, 5m cache write 1.25x): (1.25 - 0.1) / (2 * 0.1) = 5.
pub(crate) const FALLBACK_PING_BUDGET: u32 = 5;

/// Pings one idle stretch (between real requests) may spend.
///
/// A ping re-reads the prefix at the cache-read price. Letting the cache lapse
/// makes the next turn rewrite it at the cache-write price (the input price on
/// hosts with no write premium) instead of reading it. Warming breaks even at
/// `(write - read) / read` pings; the budget is half that, so the worst case
/// costs half of one avoided miss. GPT-5.6 lands at 5, GPT-5.5 (no write
/// premium) at 4, and GPT-6.1 Sol (0.05x reads) at 12. Unknown read prices use
/// [`FALLBACK_PING_BUDGET`].
pub(crate) fn ping_budget(metadata: Option<&ModelMetadata>) -> u32 {
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
pub(crate) struct WarmLimits {
    pub(crate) ttl: Duration,
    pub(crate) max_pings: u32,
}

/// What the idle loop allows the warmer to do right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum WarmGate {
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
pub(crate) enum WarmHalt {
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
pub(crate) enum WarmStep {
    Rest,
    /// Nothing to do until `next_wake()`.
    Waiting,
    Ping,
    /// Warming just stopped; report it once.
    Halted(WarmHalt),
}

/// How one ping went, judged from its reported usage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PingResult {
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
    pub(crate) fn classify(outcome: &CacheWarmOutcome) -> Self {
        let usage = &outcome.usage;
        // Use the measured cache-breakpoint noise floor, not any nonzero hit:
        // a short cached head with a large rewritten suffix is still a miss.
        let read_prefix = usage.cache_read_tokens.is_some_and(|read| {
            read > 0
                && usage.inclusive_prompt_tokens().is_some_and(|prompt| {
                    prompt.saturating_sub(read) <= CACHE_MISS_NOISE_FLOOR_TOKENS
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
pub(crate) struct CacheWarmer {
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
    pub(crate) fn next_wake(&self) -> Option<Instant> {
        self.wake
    }

    /// A real request established the prefix; a new idle stretch
    /// begins with a fresh budget. A miss, an uncapped ping, or a zero budget
    /// stays sticky until the hold ends, since the next stretch would repeat it.
    pub(crate) fn record_request(&mut self) {
        self.pings = 0;
        if matches!(self.halt, Some(WarmHalt::Budget | WarmHalt::Failed)) {
            self.halt = None;
        }
    }

    /// New conversation or new route: drop scheduler state and orphan any
    /// in-flight ping.
    pub(crate) fn reset(&mut self) {
        *self = Self {
            generation: self.generation.wrapping_add(1),
            ..Self::default()
        };
    }

    /// Generation a ping started now belongs to.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether a ping from `generation` still describes the current hold.
    pub(crate) fn is_current(&self, generation: u64) -> bool {
        self.generation == generation
    }

    pub(crate) fn step(
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
    pub(crate) fn ping_finished(&mut self, result: PingResult) -> Option<WarmHalt> {
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

    /// The ping counter to show, while the hold is warming a cache that is
    /// still alive.
    pub(crate) fn progress(
        &self,
        now: Instant,
        last_prefix_read: Option<Instant>,
        limits: WarmLimits,
    ) -> Option<WarmProgress> {
        (self.gate == WarmGate::Holding
            && self.halt.is_none()
            && last_prefix_read.is_some_and(|read| now < read + limits.ttl))
        .then_some(WarmProgress {
            pings: self.pings,
            max_pings: limits.max_pings,
        })
    }

    #[cfg(test)]
    pub(crate) fn halt(&self) -> Option<WarmHalt> {
        self.halt
    }
}

/// Pings sent this idle stretch, of the budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WarmProgress {
    pub(crate) pings: u32,
    pub(crate) max_pings: u32,
}

impl WarmProgress {
    /// The status-line text.
    pub(crate) fn status_text(self) -> String {
        format!("warming cache · {}/{}", self.pings, self.max_pings)
    }
}

/// The transcript notice for a halt. `detail` is a failed ping's error.
pub(crate) fn halt_notice(halt: WarmHalt, detail: Option<&str>, max_pings: u32) -> String {
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
