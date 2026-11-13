//! The status text and `/info` spend of the keep-warm policy in
//! [`crate::cache_warm`]. The daemon session's host steps the warmer, sends
//! each ping, and reports the counter and the spend, which this module only
//! shows.

use rho_providers::model::ModelUsage;

use super::{App, StatusSource};
use crate::cache_warm::WarmProgress;

#[cfg(test)]
#[path = "cache_warm_tests.rs"]
mod tests;

impl App {
    /// Idle-loop step. The daemon steps its own warmer; only the status
    /// needs a look, since another status may have cleared since its last
    /// report. Returns whether the screen changed.
    pub(super) fn poll_cache_warm(&mut self) -> bool {
        self.sync_cache_warm_status()
    }

    /// Drops warming state for a new conversation.
    pub(super) fn reset_cache_warm(&mut self) {
        self.usage.daemon_cache_warm = None;
        self.sync_cache_warm_status();
    }

    /// Shows what the followed session's host reports of its warming: the
    /// counter, and what its pings have cost since the daemon loaded the
    /// session.
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

    /// Derives the followed session's main cost from what its host last
    /// reported. The host's spend counts its keep-warm pings, which the
    /// totals add as their own row, so they come out of it here, whichever
    /// of the two reports came last.
    pub(super) fn sync_daemon_main_cost(&mut self) {
        let Some(spend) = self.usage.daemon_spend else {
            return;
        };
        let pings = self.usage.extra.cache_warm;
        let cost_usd_micros = spend
            .map(|spend| spend.saturating_sub(pings))
            // Spend that was all pings is no main cost.
            .filter(|main| *main > 0 || pings == 0);
        self.usage.cumulative_usage = Some(ModelUsage {
            cost_usd_micros,
            ..ModelUsage::default()
        });
    }

    /// Shows the ping counter unless another status is up. Returns whether
    /// the status changed.
    fn sync_cache_warm_status(&mut self) -> bool {
        let Some(status) = self.usage.daemon_cache_warm.map(WarmProgress::status_text) else {
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
