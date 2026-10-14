//! Keeps the provider prompt cache warm while the model idles on a
//! `process start` that asked for `keep_warm`.
//!
//! Policy lives here; the request itself is built by
//! `InteractiveRuntime::cache_warm_ping`. [`CacheWarmer`] is a small state
//! machine the idle loop steps: it pings once per TTL (minus a margin) after
//! the last request that read the cached prefix, pauses while a turn or
//! compaction owns the session, and halts on its budget, a missed ping, or a
//! failed one. The hold itself lives on the process record, so it ends when
//! the process exits or is stopped.

use std::time::{Duration, Instant};

use rho_providers::model::ModelUsage;
use tokio::task::JoinError;

use super::{
    background_tasks::{TaskId, UiOutput},
    usage_cost::resolved_usage_cost_usd_micros,
    App, Entry, InteractiveRuntime, StatusSource,
};
use crate::app::interactive_runtime::CacheWarmOutcome;

#[cfg(test)]
#[path = "cache_warm_tests.rs"]
mod tests;

/// Lead time before TTL expiry at which a ping is sent.
///
/// Receipt: five capped cached pings through the Responses route took
/// 2.54-4.76 s end to end (`/tmp/spike_latency.log`). 30 s covers six times the
/// slowest round trip plus idle-loop wake latency.
pub(super) const PING_MARGIN: Duration = Duration::from_secs(30);

/// Pings allowed per idle stretch (between real requests).
///
/// Receipt: a ping re-reads the prefix at 0.05x input (Opus/Sonnet 5.5; Fable
/// and Mythos 5.1 are cheaper at 0.025x), while a cold next turn re-writes it
/// at 1.25x (5m TTL) instead of reading it at 0.05x. Warming pays off below
/// 1.2 / 0.05 = 24 pings; 12 keeps the worst case at half of one miss.
pub(super) const MAX_PINGS_PER_IDLE: u32 = 12;

/// What the idle loop allows the warmer to do right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WarmGate {
    /// No live hold, or the user turned warming off.
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
    Unsupported,
    Failed,
}

/// The idle loop's next action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WarmStep {
    Rest,
    /// Nothing to do until this instant.
    WakeAt(Instant),
    Ping,
    /// Warming just stopped; report it once.
    Halted(WarmHalt),
}

/// How one ping went, judged from its reported usage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PingResult {
    /// Read the cached prefix; the TTL restarts.
    Read,
    /// Read nothing from cache, so it paid a full write.
    Miss,
    /// Failed before reporting usage.
    Failed,
}

impl PingResult {
    /// A capped ping usually ends as an `incomplete` error after reporting
    /// usage, so usage decides before the error does.
    pub(super) fn classify(usage: &ModelUsage, failed: bool) -> Self {
        if usage.cache_read_tokens.is_some_and(|tokens| tokens > 0) {
            Self::Read
        } else if failed && usage.cache_read_tokens.is_none() && usage.cache_write_tokens.is_none()
        {
            Self::Failed
        } else {
            // Includes a route that reports no cache accounting: a ping that
            // cannot prove it read the cache is not worth repeating.
            Self::Miss
        }
    }
}

pub(super) struct CacheWarmer {
    ttl: Duration,
    /// When the provider last read the cached prefix: a real request's start
    /// or a successful ping's send. `None` when there is nothing to keep warm.
    last_prefix_read: Option<Instant>,
    pings: u32,
    in_flight: bool,
    halt: Option<WarmHalt>,
    wake: Option<Instant>,
    /// Ping spend this conversation, folded into the session cost.
    cost_usd_micros: u64,
}

impl Default for CacheWarmer {
    fn default() -> Self {
        Self {
            ttl: super::cache_stats::DEFAULT_PROMPT_CACHE_TTL,
            last_prefix_read: None,
            pings: 0,
            in_flight: false,
            halt: None,
            wake: None,
            cost_usd_micros: 0,
        }
    }
}

impl CacheWarmer {
    pub(super) fn set_ttl(&mut self, ttl: Duration) {
        self.ttl = ttl;
    }

    pub(super) fn cost_usd_micros(&self) -> u64 {
        self.cost_usd_micros
    }

    /// The deadline the idle loop should wake for, if any.
    pub(super) fn next_wake(&self) -> Option<Instant> {
        self.wake
    }

    /// A real request read the prefix at `started_at`; a new idle stretch
    /// begins with a fresh budget. A miss stays sticky until the hold ends,
    /// since repeating a mismatched prefix would pay a write every stretch.
    pub(super) fn record_request(&mut self, started_at: Instant) {
        self.last_prefix_read = Some(
            self.last_prefix_read
                .map_or(started_at, |read| read.max(started_at)),
        );
        self.pings = 0;
        if matches!(self.halt, Some(WarmHalt::Budget | WarmHalt::Failed)) {
            self.halt = None;
        }
    }

    /// Compaction rewrote the prefix; nothing is cached until the next turn.
    pub(super) fn prefix_reset(&mut self) {
        self.last_prefix_read = None;
    }

    /// New conversation: drop all state except the configured TTL.
    pub(super) fn reset(&mut self) {
        *self = Self {
            ttl: self.ttl,
            ..Self::default()
        };
    }

    pub(super) fn step(&mut self, now: Instant, gate: WarmGate) -> WarmStep {
        self.wake = None;
        match gate {
            WarmGate::Released => {
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
        let Some(read_at) = self.last_prefix_read else {
            return WarmStep::Rest;
        };
        if now >= read_at + self.ttl {
            // Already cold: a ping would only pay to rewrite it.
            return WarmStep::Rest;
        }
        // A TTL shorter than twice the margin still waits half of it, so a
        // tiny configured TTL cannot ping back to back.
        let due = read_at + self.ttl.saturating_sub(PING_MARGIN).max(self.ttl / 2);
        if now < due {
            self.wake = Some(due);
            return WarmStep::WakeAt(due);
        }
        if self.pings >= MAX_PINGS_PER_IDLE {
            return self.halt_once(WarmHalt::Budget);
        }
        self.in_flight = true;
        WarmStep::Ping
    }

    /// Applies a finished ping sent at `sent_at`; returns a new halt to report.
    pub(super) fn ping_finished(
        &mut self,
        sent_at: Instant,
        result: PingResult,
    ) -> Option<WarmHalt> {
        self.in_flight = false;
        match result {
            PingResult::Read => {
                self.pings += 1;
                self.record_ping_read(sent_at);
                None
            }
            PingResult::Miss => self.set_halt(WarmHalt::Miss),
            PingResult::Failed => self.set_halt(WarmHalt::Failed),
        }
    }

    fn record_ping_read(&mut self, sent_at: Instant) {
        self.last_prefix_read = Some(
            self.last_prefix_read
                .map_or(sent_at, |read| read.max(sent_at)),
        );
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

    fn status(&self) -> String {
        format!("warming cache · {}/{MAX_PINGS_PER_IDLE}", self.pings)
    }
}

fn halt_notice(halt: WarmHalt, detail: Option<&str>) -> String {
    match halt {
        WarmHalt::Budget => format!(
            "cache warm stopped after {MAX_PINGS_PER_IDLE} pings; the prompt cache will expire"
        ),
        WarmHalt::Miss => "cache warm stopped: a ping missed the prompt cache".to_string(),
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
        let ttl = agent.prompt_cache_ttl();
        self.usage.cache_stats.set_ttl(ttl);
        self.usage
            .cache_warm
            .set_ttl(ttl.unwrap_or(super::cache_stats::DEFAULT_PROMPT_CACHE_TTL));
        let gate = if !self.info.runtime.keep_cache_warm || !agent.has_keep_warm_hold() {
            WarmGate::Released
        } else if agent.is_session_busy() || self.is_ui_busy() {
            WarmGate::Paused
        } else if !agent.supports_cache_warm() {
            WarmGate::Unsupported
        } else {
            WarmGate::Holding
        };
        match self.usage.cache_warm.step(Instant::now(), gate) {
            WarmStep::Rest => match gate {
                WarmGate::Released => self.clear_cache_warm_status(),
                WarmGate::Paused | WarmGate::Unsupported | WarmGate::Holding => false,
            },
            WarmStep::WakeAt(_) => self.show_cache_warm_status(),
            WarmStep::Ping => {
                let ping = agent.cache_warm_ping();
                let sent_at = Instant::now();
                self.tasks
                    .spawn(TaskId::CacheWarm, ping.send(), move |result| {
                        UiOutput::CacheWarm { sent_at, result }.into()
                    });
                false
            }
            WarmStep::Halted(halt) => {
                self.clear_cache_warm_status();
                self.insert_entry(&Entry::Notice(halt_notice(halt, None)));
                true
            }
        }
    }

    /// Applies a finished ping. Returns whether the screen changed.
    pub(super) fn apply_cache_warm(
        &mut self,
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
            self.usage.cache_warm.cost_usd_micros =
                self.usage.cache_warm.cost_usd_micros.saturating_add(cost);
        }
        let result = PingResult::classify(&outcome.usage, outcome.error.is_some());
        match self.usage.cache_warm.ping_finished(sent_at, result) {
            None => self.show_cache_warm_status(),
            Some(halt) => {
                self.clear_cache_warm_status();
                self.insert_entry(&Entry::Notice(halt_notice(halt, outcome.error.as_deref())));
                true
            }
        }
    }

    /// Drops warming state and any in-flight ping for a new conversation.
    pub(super) fn reset_cache_warm(&mut self) {
        self.usage.cache_warm.reset();
        self.tasks.abort(|id| matches!(id, TaskId::CacheWarm));
        self.clear_cache_warm_status();
    }

    /// Shows the ping counter unless another status is up. Returns whether
    /// the status changed.
    fn show_cache_warm_status(&mut self) -> bool {
        let owned = self.status_source == StatusSource::CacheWarm;
        if !owned && !self.status().is_empty() {
            return false;
        }
        let status = self.usage.cache_warm.status();
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
