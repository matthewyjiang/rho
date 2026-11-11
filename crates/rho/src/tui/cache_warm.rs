//! The in-process host of the keep-warm policy in [`crate::cache_warm`], and
//! the status text and `/info` spend either host's warming shows.
//!
//! In process, the idle loop steps the [`CacheWarmer`] and sends each ping
//! `InteractiveRuntime::cache_warm_ping` builds. A daemon session's host does
//! both and reports the counter and the spend, which this module only shows.

use std::time::Instant;

use rho_providers::model::ModelUsage;
use tokio::task::JoinError;

use super::{
    background_tasks::{OnCancel, TaskId, UiOutput},
    usage_cost::resolved_usage_cost_usd_micros,
    App, Entry, InteractiveRuntime, StatusSource,
};
use crate::cache_warm::{
    halt_notice, ping_budget, CacheWarmOutcome, PingResult, WarmGate, WarmLimits, WarmProgress,
    WarmStep,
};

#[cfg(test)]
#[path = "cache_warm_tests.rs"]
mod tests;

impl App {
    /// Idle-loop step: start a due ping or report a halt. Returns whether the
    /// screen changed.
    pub(super) fn poll_cache_warm(&mut self, agent: &InteractiveRuntime) -> bool {
        if self.info.session.daemon.is_some() {
            // The daemon steps its own warmer; only the status needs a look,
            // since another status may have cleared since its last report.
            return self.sync_cache_warm_status();
        }
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
        self.usage.daemon_cache_warm = None;
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

    /// Shows what the followed daemon session's host reports of its
    /// warming: the counter, and what its pings have cost since the daemon
    /// loaded the session.
    pub(super) fn show_daemon_cache_warm(
        &mut self,
        warming: Option<WarmProgress>,
        cost_usd_micros: u64,
    ) {
        self.usage.daemon_cache_warm = warming;
        self.usage.extra.cache_warm = cost_usd_micros;
        self.sync_daemon_main_cost();
        self.sync_cache_warm_status();
    }

    /// Derives the followed daemon session's main cost from what its host
    /// last reported. The host's spend counts its keep-warm pings, which the
    /// totals add as their own row, as in process, so they come out of it
    /// here, whichever of the two reports came last.
    pub(super) fn sync_daemon_main_cost(&mut self) {
        let Some(spend) = self.usage.daemon_spend else {
            return;
        };
        let pings = self.usage.extra.cache_warm;
        let cost_usd_micros = spend
            .map(|spend| spend.saturating_sub(pings))
            // Spend that was all pings is no main cost, as in process.
            .filter(|main| *main > 0 || pings == 0);
        self.usage.cumulative_usage = Some(ModelUsage {
            cost_usd_micros,
            ..ModelUsage::default()
        });
    }

    /// The counter to show: the daemon's for a session it hosts, else this
    /// process's own warmer's.
    fn cache_warm_progress(&self) -> Option<WarmProgress> {
        if self.info.session.daemon.is_some() {
            return self.usage.daemon_cache_warm;
        }
        self.usage.cache_warm.progress(
            Instant::now(),
            self.usage.cache_stats.last_prefix_read(),
            self.cache_warm_limits(),
        )
    }

    /// Shows the ping counter unless another status is up. Returns whether
    /// the status changed.
    fn sync_cache_warm_status(&mut self) -> bool {
        let Some(status) = self.cache_warm_progress().map(WarmProgress::status_text) else {
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
