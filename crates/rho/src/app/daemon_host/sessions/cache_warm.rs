//! Keeps a session's provider prompt cache warm while its model idles on a
//! `process start` that asked for `keep_warm`, as the in-process TUI does
//! for its own session. The policy is [`crate::cache_warm`]'s; this is the
//! daemon's host of it.
//!
//! A session whose `process` tool offers `keep_warm` has one watcher,
//! started as the session is added. It steps the session's
//! [`CacheWarmer`], sends the pings, and tells the session's clients what
//! the in-process TUI shows: the ping counter and the pings' cost as a
//! `cache_warm` event, and why warming stopped as a notice. The warming is
//! the daemon's, so it goes on with no client attached.
//!
//! The watcher holds the session weakly: a process that holds the cache
//! keeps the session loaded (see `unload`), and the watcher ends once the
//! session is dropped.

use std::{
    sync::{Arc, Mutex, MutexGuard, PoisonError, Weak},
    time::{Duration, Instant},
};

use rho_protocol as wire;
use rho_providers::model::{models_dev::cached_model_metadata, ModelMetadata, ModelUsage};
use rho_sdk::{CancellationToken, ModelCallMetrics};
use tokio::sync::Notify;

use super::{cost, LoadedSession, Sessions};
use crate::{
    app::session_runtime::SessionRuntime,
    cache_warm::{
        halt_notice, ping_budget, CacheWarmOutcome, CacheWarmPing, CacheWarmer, PingResult,
        WarmGate, WarmHalt, WarmLimits, WarmProgress, WarmStep, FALLBACK_PING_BUDGET,
    },
    config::{Config, ProviderConfigs},
    tools::process::{CacheWarmRoute, ProcessManager},
    usage::pricing::catalog_cost_usd_micros,
};

/// How often the watcher looks again while a process holds the cache: for
/// the turn that paused the warming to end, the setting to change, or the
/// process to exit.
///
/// Receipt: the in-process idle loop steps its warmer every 500 ms while a
/// process is live (`tui/app_loop.rs`), so a hold is picked up, paused, and
/// released as promptly here.
const HOLD_POLL: Duration = Duration::from_millis(500);

/// A session's side of its cache warmer.
pub(super) struct KeepWarm {
    /// Whether the session's model can be pinged, and whether it still
    /// holds the cached prefix; the session's runtime keeps it up to date.
    route: CacheWarmRoute,
    warming: Mutex<Warming>,
    /// Wakes the watcher when a hold may have started.
    changed: Arc<Notify>,
    /// Cancelled when the session is dropped.
    dropped: CancellationToken,
}

struct Warming {
    warmer: CacheWarmer,
    /// When a request of the session, or a ping, last read the cached
    /// prefix on the current route.
    last_prefix_read: Option<Instant>,
    /// The route's [`CacheWarmRoute::prefix_epoch`] the fields above are of.
    prefix_epoch: u64,
    /// The limits of the model the warmer last held on.
    limits: WarmLimits,
    /// That model's catalog row, kept so a hold does not read the catalog
    /// at every step. A model with no row is looked up again, since the
    /// catalog may gain one.
    priced: Option<PricedModel>,
    /// What the session's pings have cost since it was loaded, or since its
    /// conversation last moved.
    cost_usd_micros: Option<u64>,
    /// How many times the conversation moved. A ping sent before a move is
    /// the old conversation's spend, not this one's.
    conversation: u64,
    /// What the session's clients were last told.
    reported: Report,
}

/// What clients are told of the warming.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Report {
    warming: Option<WarmProgress>,
    cost_usd_micros: Option<u64>,
}

impl Report {
    fn event(self) -> wire::SessionEvent {
        wire::SessionEvent::CacheWarm {
            warming: self.warming.map(|progress| wire::CacheWarmProgress {
                pings: progress.pings,
                max_pings: progress.max_pings,
            }),
            cost_usd_micros: self.cost_usd_micros,
        }
    }
}

impl KeepWarm {
    /// The warmer of the session `runtime` runs, when its host warms it.
    pub(super) fn of(runtime: &SessionRuntime) -> Option<Self> {
        let route = runtime.cache_warm_route()?.clone();
        Some(Self {
            warming: Mutex::new(Warming {
                warmer: CacheWarmer::default(),
                last_prefix_read: None,
                prefix_epoch: route.prefix_epoch(),
                // Replaced before the warmer first holds.
                limits: WarmLimits {
                    ttl: ProviderConfigs::DEFAULT_PROMPT_CACHE_TTL,
                    max_pings: FALLBACK_PING_BUDGET,
                },
                priced: None,
                cost_usd_micros: None,
                conversation: 0,
                reported: Report::default(),
            }),
            route,
            changed: Arc::new(Notify::new()),
            dropped: CancellationToken::new(),
        })
    }

    /// The session's run started: its turn owns the cache from here.
    pub(super) fn run_started(&self) {
        self.changed.notify_one();
    }

    /// The session's turn ended, however it did: a process it started may
    /// hold the cache though no model call followed the start, as when the
    /// turn was cancelled or its next call failed.
    pub(super) fn turn_ended(&self) {
        self.changed.notify_one();
    }

    /// A model call of the session's turn completed with `metrics`: it read
    /// the prefix when it was sent, and a new idle stretch starts after it.
    pub(super) fn model_call_completed(&self, metrics: &ModelCallMetrics) {
        let now = now();
        let sent_at = now.checked_sub(metrics.total_latency).unwrap_or(now);
        let mut warming = self.warming();
        warming.record_prefix_read(sent_at);
        warming.warmer.record_request();
        drop(warming);
        // The call may have followed a `process start` that holds the cache.
        self.changed.notify_one();
    }

    /// A compaction rewrote the prompt prefix, so the cache holds nothing of
    /// it until the next request.
    pub(super) fn prefix_rewritten(&self) {
        let mut warming = self.warming();
        warming.last_prefix_read = None;
        warming.warmer.prefix_rewritten();
    }

    /// The warming state, brought up to date with the route: a route that
    /// dropped the prefix says nothing of what the last request cached, and
    /// a ping still in flight describes the old one.
    fn warming(&self) -> MutexGuard<'_, Warming> {
        // Each change completes before the lock is released.
        let mut warming = self.warming.lock().unwrap_or_else(PoisonError::into_inner);
        let epoch = self.route.prefix_epoch();
        if warming.prefix_epoch != epoch {
            warming.prefix_epoch = epoch;
            warming.last_prefix_read = None;
            warming.warmer.reset();
        }
        warming
    }
}

impl Drop for KeepWarm {
    fn drop(&mut self) {
        self.dropped.cancel();
    }
}

impl Warming {
    fn record_prefix_read(&mut self, sent_at: Instant) {
        self.last_prefix_read = Some(
            self.last_prefix_read
                .map_or(sent_at, |read| read.max(sent_at)),
        );
    }
}

/// A model's catalog row, which gives its ping budget and prices a ping
/// the provider put no cost on.
struct PricedModel {
    provider: String,
    model: String,
    metadata: ModelMetadata,
}

impl Warming {
    /// Takes the limits of `runtime`'s model, which the warmer is about to
    /// hold on, and returns its catalog row.
    fn hold_on(&mut self, runtime: &SessionRuntime, config: &Config) -> Option<ModelMetadata> {
        let current = runtime.current_model();
        let known = self.priced.as_ref().is_some_and(|priced| {
            priced.provider == current.provider && priced.model == current.model
        });
        if !known {
            self.priced =
                cached_model_metadata(&current.provider, &current.model).map(|metadata| {
                    PricedModel {
                        provider: current.provider.clone(),
                        model: current.model.clone(),
                        metadata,
                    }
                });
        }
        let metadata = self.priced.as_ref().map(|priced| &priced.metadata);
        self.limits = WarmLimits {
            ttl: config
                .providers
                .resolved_prompt_cache_ttl(&current.provider),
            max_pings: ping_budget(metadata),
        };
        metadata.cloned()
    }
}

impl Sessions {
    /// Starts `session`'s cache warmer, if its host warms it. Call once, as
    /// the session is added.
    pub(super) fn watch_cache_warm(&self, session: &Arc<LoadedSession>) {
        let Some(keep) = &session.cache_warm else {
            return;
        };
        let watcher = watch(Arc::downgrade(session), Arc::clone(&keep.changed));
        let dropped = keep.dropped.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = dropped.cancelled() => {}
                () = watcher => {}
            }
        });
    }
}

/// Steps `session`'s warmer whenever a hold may have started, and, while a
/// process holds the cache, at each ping's due time and every [`HOLD_POLL`],
/// until the session is gone.
async fn watch(session: Weak<LoadedSession>, changed: Arc<Notify>) {
    loop {
        let next = {
            let Some(session) = session.upgrade() else {
                return;
            };
            session.step_cache_warm().await
        };
        match next {
            Some(next) => {
                tokio::select! {
                    () = changed.notified() => {}
                    () = tokio::time::sleep_until(next.into()) => {}
                }
            }
            // Nothing holds the cache; only a turn starts a process.
            None => changed.notified().await,
        }
    }
}

impl LoadedSession {
    /// Steps the session's warmer: starts a ping that is due, tells clients
    /// a halt, and reports the counter. Returns when to step again, while a
    /// process holds the cache.
    async fn step_cache_warm(self: &Arc<Self>) -> Option<Instant> {
        let keep = self.cache_warm.as_ref()?;
        // Read live, so turning the setting off stops the warming at once.
        let config = self.settings.agent_config().await.ok();
        let enabled = config.as_ref().is_some_and(|config| config.keep_cache_warm);
        let held = self
            .processes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(ProcessManager::has_keep_warm_hold);
        let now = now();
        let warms = enabled && held;
        // Only a turn pauses the warming: its own requests refresh the cache.
        let turn_under_way = self.turn_under_way();
        // Asked only for an idle hold, so a session nothing warms never
        // contends for its runtime.
        let runtime = (warms && !turn_under_way)
            .then(|| self.runtime.try_lock().ok())
            .flatten();
        let idle_runtime = runtime.as_ref().and_then(|runtime| runtime.as_ref());
        if warms && !turn_under_way && (idle_runtime.is_none() || self.turn().hold.is_some()) {
            // Something else has the runtime for a moment: a turn that just
            // ended, a settings change, a client's request. That is no
            // pause, so the warmer and what clients were told stay as they
            // are, and the next look decides.
            return Some(now + HOLD_POLL);
        }
        let gate = if !warms {
            WarmGate::Released
        } else if turn_under_way {
            WarmGate::Paused
        } else if !keep.route.is_supported() {
            WarmGate::Unsupported
        } else {
            WarmGate::Holding
        };
        let mut warming = keep.warming();
        let metadata = match (gate, idle_runtime, &config) {
            (WarmGate::Holding, Some(runtime), Some(config)) => warming.hold_on(runtime, config),
            _ => None,
        };
        let limits = warming.limits;
        let last_prefix_read = warming.last_prefix_read;
        match warming.warmer.step(now, gate, last_prefix_read, limits) {
            WarmStep::Rest | WarmStep::Waiting => {}
            WarmStep::Ping => {
                let ping = idle_runtime
                    .ok_or_else(|| "the session is busy".to_owned())
                    .and_then(|runtime| {
                        runtime.cache_warm_ping().map_err(|error| error.to_string())
                    });
                self.send_cache_warm_ping(
                    ping,
                    SentPing {
                        conversation: warming.conversation,
                        generation: warming.warmer.generation(),
                        sent_at: now,
                        metadata,
                    },
                );
            }
            WarmStep::Halted(halt) => self.tell_cache_warm_halt(halt, None, limits.max_pings),
        }
        self.report_cache_warm(&mut warming, now);
        let next_ping = warming.warmer.next_wake();
        held.then(|| {
            let poll = now + HOLD_POLL;
            next_ping.map_or(poll, |ping| ping.min(poll))
        })
    }

    /// Sends `ping` in a task of its own, then applies how it went. Never
    /// cancelled: the provider bills the prefix read once it accepts the
    /// request, so the send finishes and reaches the usage ledger even when
    /// the hold ended meanwhile.
    fn send_cache_warm_ping(self: &Arc<Self>, ping: Result<CacheWarmPing, String>, sent: SentPing) {
        let session = Arc::downgrade(self);
        tokio::spawn(async move {
            let outcome = match ping {
                Ok(ping) => ping.send().await,
                Err(error) => CacheWarmOutcome {
                    usage: ModelUsage::default(),
                    error: Some(error),
                },
            };
            let Some(session) = session.upgrade() else {
                return;
            };
            // The ledger has the ping now, so the session's cost is told
            // again with it, ahead of the pings' own cost: clients take the
            // second out of the first. A turn under way tells its own.
            let usage = if session.turn_under_way() {
                None
            } else {
                session.live().streamed_usage()
            };
            let usage = cost::priced(usage.into_iter().collect(), &session.id).await;
            session.cache_warm_ping_finished(&sent, &outcome, usage);
        });
    }

    /// Applies a finished ping, telling clients `usage`, the session's cost
    /// with the ping in it, first. Its spend counts toward the conversation
    /// it was sent in; a ping the hold or the route no longer describes
    /// changes nothing else.
    fn cache_warm_ping_finished(
        &self,
        sent: &SentPing,
        outcome: &CacheWarmOutcome,
        usage: Vec<wire::SessionEvent>,
    ) {
        let Some(keep) = &self.cache_warm else {
            return;
        };
        let mut warming = keep.warming();
        if warming.conversation == sent.conversation {
            if let Some(cost) = ping_cost(&outcome.usage, sent.metadata.as_ref()) {
                warming.cost_usd_micros = Some(
                    warming
                        .cost_usd_micros
                        .unwrap_or_default()
                        .saturating_add(cost),
                );
            }
            self.feed.publish(usage);
        }
        if warming.warmer.is_current(sent.generation) {
            let result = PingResult::classify(outcome);
            if result == PingResult::Read {
                warming.record_prefix_read(sent.sent_at);
            }
            if let Some(halt) = warming.warmer.ping_finished(result) {
                self.tell_cache_warm_halt(halt, outcome.error.as_deref(), warming.limits.max_pings);
            }
        }
        self.report_cache_warm(&mut warming, now());
        drop(warming);
        // The next ping is due from this one's read.
        keep.changed.notify_one();
    }

    /// The session's conversation moved to another point of its tree, which
    /// is a new conversation to its clients: the warming starts over, and
    /// its spend with it, as the in-process TUI's totals do.
    pub(super) fn cache_warm_conversation_moved(&self) {
        let Some(keep) = &self.cache_warm else {
            return;
        };
        let mut warming = keep.warming();
        warming.warmer.reset();
        warming.last_prefix_read = None;
        warming.cost_usd_micros = None;
        warming.conversation = warming.conversation.wrapping_add(1);
        self.report_cache_warm(&mut warming, now());
    }

    /// Whether a turn of the session is queued, running, or waiting for
    /// input.
    fn turn_under_way(&self) -> bool {
        // The session's own job, not any job that writes it: a subagent's
        // job writes the subagent's session, and pauses nothing here.
        self.jobs
            .get(&self.job)
            .is_some_and(|job| job.state.in_progress())
    }

    fn tell_cache_warm_halt(&self, halt: WarmHalt, detail: Option<&str>, max_pings: u32) {
        self.feed.publish([wire::SessionEvent::Notice {
            text: halt_notice(halt, detail, max_pings),
        }]);
    }

    /// Tells clients the counter and the spend, when either changed. Under
    /// the warming's lock, so reports reach clients in the order they were
    /// made.
    fn report_cache_warm(&self, warming: &mut Warming, now: Instant) {
        let report = Report {
            warming: warming
                .warmer
                .progress(now, warming.last_prefix_read, warming.limits),
            cost_usd_micros: warming.cost_usd_micros,
        };
        if report != warming.reported {
            warming.reported = report;
            self.feed.publish([report.event()]);
        }
    }
}

/// A ping on its way: what applying its outcome needs.
struct SentPing {
    /// [`Warming::conversation`] when the ping started.
    conversation: u64,
    /// The warmer's generation when the ping started.
    generation: u64,
    sent_at: Instant,
    metadata: Option<ModelMetadata>,
}

/// What a ping cost: the provider's figure, else the catalog's price of its
/// usage, as the in-process TUI counts it.
fn ping_cost(usage: &ModelUsage, metadata: Option<&ModelMetadata>) -> Option<u64> {
    usage
        .cost_usd_micros
        .or_else(|| catalog_cost_usd_micros(usage, metadata?).filter(|micros| *micros > 0))
}

/// Now, on the clock the watcher sleeps on.
fn now() -> Instant {
    tokio::time::Instant::now().into_std()
}
