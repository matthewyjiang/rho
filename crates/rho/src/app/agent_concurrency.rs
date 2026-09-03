//! Live-resizable concurrency pools for delegated agents.
//!
//! One global pool covers every delegated run (Rho and Claude, foreground and
//! background). Claude-cli runs also take a nested Claude permit first so queued
//! Claude work cannot occupy spare global slots.

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use tokio::sync::Notify;

use {crate::config::MAX_AGENT_CONCURRENCY, rho_tools::cancellation::RunCancellation};

use super::agent_binding::CapacityClass;

/// Default Claude-specific concurrency. Always nested under the total pool so
/// Claude fan-out cannot exceed this even when Rho capacity remains.
const DEFAULT_CLAUDE_CONCURRENCY: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ConcurrencyLimits {
    pub(crate) total: usize,
    /// Nested Claude cap before clamping to `total`.
    pub(crate) claude_requested: usize,
}

impl ConcurrencyLimits {
    fn claude_nested(self) -> usize {
        self.claude_requested.min(self.total)
    }
}

/// Shared, live-resizable capacity for delegated runs.
#[derive(Clone)]
pub(crate) struct AgentConcurrency {
    total: Arc<AdjustablePool>,
    claude: Arc<AdjustablePool>,
    claude_requested: Arc<AtomicUsize>,
}

pub(crate) struct RuntimePermits {
    _total: PoolPermit,
    _claude: Option<PoolPermit>,
}

struct AdjustablePool {
    limit: AtomicUsize,
    active: AtomicUsize,
    notify: Notify,
}

struct PoolPermit {
    pool: Arc<AdjustablePool>,
}

impl Drop for PoolPermit {
    fn drop(&mut self) {
        self.pool.active.fetch_sub(1, Ordering::Release);
        self.pool.notify.notify_waiters();
    }
}

impl AdjustablePool {
    fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            limit: AtomicUsize::new(limit),
            active: AtomicUsize::new(0),
            notify: Notify::new(),
        })
    }

    fn limit(&self) -> usize {
        self.limit.load(Ordering::Acquire)
    }

    fn active(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }

    #[cfg(test)]
    fn available(&self) -> usize {
        self.limit().saturating_sub(self.active())
    }

    fn set_limit(&self, limit: usize) {
        self.limit.store(limit, Ordering::Release);
        self.notify.notify_waiters();
    }

    fn try_acquire(self: &Arc<Self>) -> Option<PoolPermit> {
        loop {
            let limit = self.limit();
            let active = self.active();
            if active >= limit {
                return None;
            }
            if self
                .active
                .compare_exchange(active, active + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Some(PoolPermit {
                    pool: Arc::clone(self),
                });
            }
        }
    }

    async fn acquire(self: &Arc<Self>, cancellation: &RunCancellation) -> Option<PoolPermit> {
        loop {
            if cancellation.is_cancelled() {
                return None;
            }
            if let Some(permit) = self.take_unless_cancelled(cancellation) {
                return Some(permit);
            }
            let notified = self.notify.notified();
            tokio::pin!(notified);
            if let Some(permit) = self.take_unless_cancelled(cancellation) {
                return Some(permit);
            }
            tokio::select! {
                biased;
                () = cancellation.cancelled() => return None,
                () = notified => {}
            }
        }
    }

    fn take_unless_cancelled(
        self: &Arc<Self>,
        cancellation: &RunCancellation,
    ) -> Option<PoolPermit> {
        let permit = self.try_acquire()?;
        if cancellation.is_cancelled() {
            None
        } else {
            Some(permit)
        }
    }
}

impl AgentConcurrency {
    pub(crate) fn new(limits: ConcurrencyLimits) -> Self {
        Self {
            total: AdjustablePool::new(limits.total),
            claude: AdjustablePool::new(limits.claude_nested()),
            claude_requested: Arc::new(AtomicUsize::new(limits.claude_requested)),
        }
    }

    pub(crate) fn from_config_and_env(configured_total: usize) -> Self {
        Self::new(concurrency_limits(configured_total))
    }

    /// Replace the global cap. In-flight runs keep their slots; a lower cap
    /// only delays new acquires until `active` falls under the new limit.
    /// Claude nested capacity is `min(requested, total)`.
    pub(crate) fn set_total(&self, total: usize) {
        let total = total.clamp(1, MAX_AGENT_CONCURRENCY);
        self.total.set_limit(total);
        let claude = self.claude_requested.load(Ordering::Acquire).min(total);
        self.claude.set_limit(claude);
    }

    pub(crate) fn total_limit(&self) -> usize {
        self.total.limit()
    }

    #[cfg(test)]
    pub(crate) fn available_total(&self) -> usize {
        self.total.available()
    }

    #[cfg(test)]
    pub(crate) fn available_claude(&self) -> usize {
        self.claude.available()
    }

    /// Acquire concurrency for a delegated run in runtime-aware order.
    ///
    /// - Rho: one global permit only.
    /// - Claude: Claude nested permit first, then one global permit.
    ///
    /// Claude-first ordering keeps queued Claude tasks off the global pool until
    /// Claude capacity exists, so spare global slots stay available for Rho.
    /// Cancellation at either wait stage returns `Ok(None)` and drops any permit
    /// already held so capacity cannot leak.
    pub(crate) async fn acquire(
        &self,
        capacity_class: CapacityClass,
        cancellation: &RunCancellation,
    ) -> Option<RuntimePermits> {
        let claude = match capacity_class {
            CapacityClass::Claude => {
                let Some(permit) = self.claude.acquire(cancellation).await else {
                    return None;
                };
                Some(permit)
            }
            CapacityClass::Rho => None,
        };

        let Some(total) = self.total.acquire(cancellation).await else {
            return None;
        };

        Some(RuntimePermits {
            _total: total,
            _claude: claude,
        })
    }
}

/// Resolve startup limits from config plus optional env overrides.
///
/// `RHO_AGENT_CONCURRENCY` overrides the configured total when it is a positive
/// integer. `RHO_CLAUDE_AGENT_CONCURRENCY` overrides the nested Claude cap.
/// Both are clamped to [`MAX_AGENT_CONCURRENCY`], and Claude is nested under
/// the resolved total so overrides cannot open a 2N fan-out window.
pub(crate) fn concurrency_limits(configured_total: usize) -> ConcurrencyLimits {
    concurrency_limits_from_env(
        std::env::var("RHO_AGENT_CONCURRENCY").ok().as_deref(),
        std::env::var("RHO_CLAUDE_AGENT_CONCURRENCY")
            .ok()
            .as_deref(),
        configured_total,
    )
}

pub(crate) fn concurrency_limits_from_env(
    total_raw: Option<&str>,
    claude_raw: Option<&str>,
    configured_total: usize,
) -> ConcurrencyLimits {
    let total = parse_concurrency_or(total_raw, configured_total);
    let claude_requested = parse_concurrency_or(claude_raw, DEFAULT_CLAUDE_CONCURRENCY);
    ConcurrencyLimits {
        total,
        claude_requested,
    }
}

fn parse_concurrency_or(raw: Option<&str>, fallback: usize) -> usize {
    parse_positive_concurrency(raw).unwrap_or(clamp_concurrency(fallback))
}

fn parse_positive_concurrency(raw: Option<&str>) -> Option<usize> {
    raw.and_then(|value| value.parse().ok())
        .filter(|limit: &usize| *limit > 0)
        .map(clamp_concurrency)
}

fn clamp_concurrency(value: usize) -> usize {
    value.clamp(1, MAX_AGENT_CONCURRENCY)
}

#[cfg(test)]
#[path = "agent_concurrency_tests.rs"]
mod tests;
