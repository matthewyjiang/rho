//! Live-resizable concurrency pools for delegated agents, granted in the
//! order runs joined them.
//!
//! One global pool (`behavior.agent_concurrency`) covers every delegated run
//! (Rho, Claude, Cursor, and Antigravity). Claude-cli runs also take a nested
//! Claude permit first (`RHO_CLAUDE_AGENT_CONCURRENCY`) so queued Claude work
//! cannot occupy spare global slots.
//!
//! A run joins its first queue when it is spawned ([`AgentConcurrency::join`]),
//! so runs get capacity in the order they joined each queue, and its
//! [`QueueWatch`] says where it stands while it waits: the budget, that
//! budget's limit, and its position (#1658).

use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex, MutexGuard, PoisonError,
    },
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

/// A budget delegated runs wait on, named for the setting that sizes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Budget {
    /// Every delegated run: `behavior.agent_concurrency`.
    AgentConcurrency,
    /// Claude-cli runs, nested under the global budget:
    /// `RHO_CLAUDE_AGENT_CONCURRENCY`.
    ClaudeAgentConcurrency,
}

/// Where a waiting run stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QueuePlace {
    pub(crate) budget: Budget,
    pub(crate) limit: usize,
    /// 1 for the run that takes the next free slot.
    pub(crate) position: usize,
}

/// Shared, live-resizable capacity for delegated runs.
#[derive(Clone)]
pub(crate) struct AgentConcurrency {
    total: Arc<Pool>,
    claude: Arc<Pool>,
    claude_requested: Arc<AtomicUsize>,
}

pub(crate) struct RuntimePermits {
    _total: PoolPermit,
    _claude: Option<PoolPermit>,
}

/// One budget's slots and the runs waiting for one, first come first served.
struct Pool {
    budget: Budget,
    state: Mutex<PoolState>,
}

struct PoolState {
    limit: usize,
    /// Slots granted and not yet returned. Above `limit` only after the
    /// limit was lowered under work already running.
    active: usize,
    /// Runs waiting, in the order they joined. Never holds a run while a
    /// slot is free: every change that frees one grants it to the front.
    queue: VecDeque<Arc<Waiter>>,
}

/// One run's wait in one pool's queue.
#[derive(Default)]
struct Waiter {
    /// Set, under the pool lock, once the pool grants this run a slot.
    granted: AtomicBool,
    notify: Notify,
}

/// A held slot; dropping it returns the slot to the pool.
struct PoolPermit {
    pool: Arc<Pool>,
}

impl Drop for PoolPermit {
    fn drop(&mut self) {
        let mut state = self.pool.state();
        state.active -= 1;
        state.grant_waiters();
    }
}

impl PoolState {
    /// Grants free slots to the runs at the front of the queue.
    fn grant_waiters(&mut self) {
        while self.active < self.limit {
            let Some(waiter) = self.queue.pop_front() else {
                break;
            };
            self.active += 1;
            waiter.granted.store(true, Ordering::Release);
            // Stores the wake when the run is not waiting yet, so a grant
            // between its check and its wait is not lost.
            waiter.notify.notify_one();
        }
    }
}

impl Pool {
    fn new(budget: Budget, limit: usize) -> Arc<Self> {
        Arc::new(Self {
            budget,
            state: Mutex::new(PoolState {
                limit,
                active: 0,
                queue: VecDeque::new(),
            }),
        })
    }

    fn state(&self) -> MutexGuard<'_, PoolState> {
        // Every update leaves the counts and the queue consistent before it
        // can panic, so a poisoned lock still guards valid state.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn limit(&self) -> usize {
        self.state().limit
    }

    #[cfg(test)]
    fn available(&self) -> usize {
        let state = self.state();
        state.limit.saturating_sub(state.active)
    }

    fn set_limit(&self, limit: usize) {
        let mut state = self.state();
        state.limit = limit;
        state.grant_waiters();
    }

    /// Joins the back of the queue; a run that finds a free slot and nobody
    /// ahead is granted it at once.
    fn join(self: &Arc<Self>) -> Waiting {
        let waiter = Arc::new(Waiter::default());
        let mut state = self.state();
        state.queue.push_back(Arc::clone(&waiter));
        state.grant_waiters();
        drop(state);
        Waiting {
            pool: Arc::clone(self),
            waiter,
            claimed: false,
        }
    }

    /// Where `waiter` stands, while it is still in the queue.
    fn place(&self, waiter: &Arc<Waiter>) -> Option<QueuePlace> {
        let state = self.state();
        let index = state
            .queue
            .iter()
            .position(|queued| Arc::ptr_eq(queued, waiter))?;
        Some(QueuePlace {
            budget: self.budget,
            limit: state.limit,
            position: index + 1,
        })
    }
}

/// A run's turn in one pool's queue. Dropped before its slot is claimed, it
/// leaves the queue, or hands a slot granted meanwhile to the next run.
struct Waiting {
    pool: Arc<Pool>,
    waiter: Arc<Waiter>,
    claimed: bool,
}

impl Waiting {
    /// Waits for the pool to grant the slot. `None` once `cancellation`
    /// trips, even when a slot is free.
    async fn acquire(mut self, cancellation: &RunCancellation) -> Option<PoolPermit> {
        loop {
            if cancellation.is_cancelled() {
                return None;
            }
            if self.waiter.granted.load(Ordering::Acquire) {
                self.claimed = true;
                return Some(PoolPermit {
                    pool: Arc::clone(&self.pool),
                });
            }
            tokio::select! {
                biased;
                () = cancellation.cancelled() => return None,
                () = self.waiter.notify.notified() => {}
            }
        }
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        if self.claimed {
            return;
        }
        let mut state = self.pool.state();
        // `granted` only changes under this lock, so it cannot flip here.
        if self.waiter.granted.load(Ordering::Acquire) {
            state.active -= 1;
        } else {
            state
                .queue
                .retain(|queued| !Arc::ptr_eq(queued, &self.waiter));
        }
        state.grant_waiters();
    }
}

/// Where one run waits, shared by the run and those watching it.
#[derive(Clone, Default)]
pub(crate) struct QueueWatch(Arc<Mutex<Option<WaitingIn>>>);

/// The pool a run waits in, and its wait there.
struct WaitingIn {
    pool: Arc<Pool>,
    waiter: Arc<Waiter>,
}

impl QueueWatch {
    fn waiting_on(&self, waiting: &Waiting) {
        *self.current() = Some(WaitingIn {
            pool: Arc::clone(&waiting.pool),
            waiter: Arc::clone(&waiting.waiter),
        });
    }

    fn clear(&self) {
        *self.current() = None;
    }

    /// Where the run stands while it waits for capacity; `None` once it
    /// holds every slot it needs, or stopped waiting.
    pub(crate) fn place(&self) -> Option<QueuePlace> {
        let current = self.current();
        let WaitingIn { pool, waiter } = current.as_ref()?;
        pool.place(waiter)
    }

    fn current(&self) -> MutexGuard<'_, Option<WaitingIn>> {
        // Only ever replaced whole.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A run in line for capacity since [`AgentConcurrency::join`]. Dropping
/// it gives up its place.
pub(crate) struct QueuedRun {
    line: Line,
    watch: QueueWatch,
}

/// The queues a run waits in, by its capacity class.
enum Line {
    /// Rho, Cursor, Antigravity: the global queue only. No nested pool:
    /// unlike Claude, there is no measured subscription fan-out limit to
    /// size one against.
    Global(Waiting),
    /// Claude: the Claude queue, then the global one, joined once the run
    /// has its Claude permit, so queued Claude work never holds a global
    /// slot and spare global slots stay available for Rho.
    ClaudeThenGlobal { claude: Waiting, global: Arc<Pool> },
}

impl QueuedRun {
    /// Where the run stands, for as long as it waits.
    pub(crate) fn watch(&self) -> QueueWatch {
        self.watch.clone()
    }

    /// Waits for every permit the run's line needs. Cancellation at either
    /// stage returns `None` and drops any permit already held, so capacity
    /// cannot leak.
    pub(crate) async fn acquire(self, cancellation: &RunCancellation) -> Option<RuntimePermits> {
        let Self { line, watch } = self;
        let permits = match line {
            Line::Global(global) => {
                global
                    .acquire(cancellation)
                    .await
                    .map(|total| RuntimePermits {
                        _total: total,
                        _claude: None,
                    })
            }
            Line::ClaudeThenGlobal { claude, global } => match claude.acquire(cancellation).await {
                None => None,
                Some(claude) => {
                    let global = global.join();
                    watch.waiting_on(&global);
                    global
                        .acquire(cancellation)
                        .await
                        .map(|total| RuntimePermits {
                            _total: total,
                            _claude: Some(claude),
                        })
                }
            },
        };
        watch.clear();
        permits
    }
}

impl AgentConcurrency {
    pub(crate) fn new(limits: ConcurrencyLimits) -> Self {
        Self {
            total: Pool::new(Budget::AgentConcurrency, limits.total),
            claude: Pool::new(Budget::ClaudeAgentConcurrency, limits.claude_nested()),
            claude_requested: Arc::new(AtomicUsize::new(limits.claude_requested)),
        }
    }

    pub(crate) fn from_config(configured_total: usize) -> Self {
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

    /// Puts a run of `class` in line for capacity now: runs get capacity in
    /// the order they joined each queue.
    pub(crate) fn join(&self, class: CapacityClass) -> QueuedRun {
        let line = match class {
            CapacityClass::Claude => Line::ClaudeThenGlobal {
                claude: self.claude.join(),
                global: Arc::clone(&self.total),
            },
            CapacityClass::Rho | CapacityClass::Cursor | CapacityClass::Antigravity => {
                Line::Global(self.total.join())
            }
        };
        let watch = QueueWatch::default();
        let (Line::Global(first) | Line::ClaudeThenGlobal { claude: first, .. }) = &line;
        watch.waiting_on(first);
        QueuedRun { line, watch }
    }
}

/// Resolve startup limits from config plus the optional nested Claude env.
///
/// The global cap is `behavior.agent_concurrency` only. `RHO_CLAUDE_AGENT_CONCURRENCY`
/// still overrides the nested Claude cap. Both are clamped to
/// [`MAX_AGENT_CONCURRENCY`], and Claude is nested under the resolved total so
/// the env cannot open a 2N fan-out window.
pub(crate) fn concurrency_limits(configured_total: usize) -> ConcurrencyLimits {
    concurrency_limits_from_claude_env(
        std::env::var("RHO_CLAUDE_AGENT_CONCURRENCY")
            .ok()
            .as_deref(),
        configured_total,
    )
}

pub(crate) fn concurrency_limits_from_claude_env(
    claude_raw: Option<&str>,
    configured_total: usize,
) -> ConcurrencyLimits {
    ConcurrencyLimits {
        total: clamp_concurrency(configured_total),
        claude_requested: parse_concurrency_or(claude_raw, DEFAULT_CLAUDE_CONCURRENCY),
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
