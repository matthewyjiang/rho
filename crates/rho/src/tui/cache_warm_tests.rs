use std::time::{Duration, Instant};

use pretty_assertions::assert_eq;
use rho_providers::model::{models_dev::ModelCost, ModelMetadata, ModelUsage};

use super::{
    ping_budget, CacheWarmer, PingResult, WarmGate, WarmHalt, WarmLimits, WarmStep,
    FALLBACK_PING_BUDGET, PING_MARGIN,
};

const TTL: Duration = Duration::from_secs(300);
const MAX_PINGS: u32 = 3;

struct Warmer {
    scheduler: CacheWarmer,
    clock: crate::tui::cache_stats::CacheStatsTracker,
    limits: WarmLimits,
}

impl Default for Warmer {
    fn default() -> Self {
        Self {
            scheduler: CacheWarmer::default(),
            clock: Default::default(),
            limits: WarmLimits {
                ttl: TTL,
                max_pings: MAX_PINGS,
            },
        }
    }
}

impl Warmer {
    fn step(&mut self, now: Instant, gate: WarmGate) -> WarmStep {
        self.scheduler
            .step(now, gate, self.clock.last_prefix_read(), self.limits)
    }

    fn record_request(&mut self, at: Instant) {
        self.clock.record_prefix_read(at);
        self.scheduler.record_request();
    }

    fn next_wake(&self) -> Option<Instant> {
        self.scheduler.next_wake()
    }

    fn ping_finished(&mut self, at: Instant, result: PingResult) -> Option<WarmHalt> {
        if result == PingResult::Read {
            self.clock.record_prefix_read(at);
        }
        self.scheduler.ping_finished(result)
    }
}

fn warmer(t0: Instant) -> Warmer {
    let mut warmer = Warmer::default();
    warmer.record_request(t0);
    warmer
}

/// Steps at the due instant and completes the ping as `result`.
fn ping_at(warmer: &mut Warmer, at: Instant, result: PingResult) -> Option<WarmHalt> {
    assert_eq!(warmer.step(at, WarmGate::Holding), WarmStep::Ping);
    warmer.ping_finished(at, result)
}

#[test]
fn schedules_one_ping_per_ttl_from_the_last_prefix_read() {
    let t0 = Instant::now();
    let due = t0 + TTL - PING_MARGIN;
    let mut warmer = warmer(t0);

    assert_eq!(warmer.step(t0, WarmGate::Holding), WarmStep::Waiting);
    assert_eq!(warmer.next_wake(), Some(due));
    assert_eq!(warmer.step(due, WarmGate::Holding), WarmStep::Ping);
    // In flight: no second ping while the first is outstanding.
    assert_eq!(warmer.step(due, WarmGate::Holding), WarmStep::Rest);
    assert_eq!(warmer.ping_finished(due, PingResult::Read), None);
    // The successful ping restarts the TTL clock from when it was sent.
    assert_eq!(warmer.step(due, WarmGate::Holding), WarmStep::Waiting);
    assert_eq!(warmer.next_wake(), Some(due + TTL - PING_MARGIN));
}

#[test]
fn gates_suppress_pings_until_holding() {
    let t0 = Instant::now();
    let due = t0 + TTL - PING_MARGIN;
    let mut warmer = warmer(t0);

    // A running turn refreshes the cache itself; a released hold never pings.
    assert_eq!(warmer.step(due, WarmGate::Paused), WarmStep::Rest);
    assert_eq!(warmer.step(due, WarmGate::Released), WarmStep::Rest);
    assert_eq!(warmer.next_wake(), None);
    assert_eq!(warmer.step(due, WarmGate::Holding), WarmStep::Ping);
}

#[test]
fn a_cold_cache_is_not_rewarmed() {
    let t0 = Instant::now();
    let mut warmer = warmer(t0);

    assert_eq!(warmer.step(t0 + TTL, WarmGate::Holding), WarmStep::Rest);
    assert_eq!(warmer.next_wake(), None);
}

#[test]
fn compaction_waits_for_the_next_real_request() {
    let t0 = Instant::now();
    let mut warmer = warmer(t0);
    warmer.clock.prompt_prefix_reset();

    assert_eq!(
        warmer.step(t0 + TTL - PING_MARGIN, WarmGate::Holding),
        WarmStep::Rest
    );

    let t1 = t0 + Duration::from_secs(60);
    warmer.record_request(t1);
    assert_eq!(warmer.step(t1, WarmGate::Holding), WarmStep::Waiting);
    assert_eq!(warmer.next_wake(), Some(t1 + TTL - PING_MARGIN));
}

#[test]
fn a_ttl_shorter_than_twice_the_margin_waits_half_the_ttl() {
    let t0 = Instant::now();
    let ttl = PING_MARGIN;
    let mut warmer = Warmer::default();
    warmer.limits.ttl = ttl;
    warmer.record_request(t0);

    assert_eq!(warmer.step(t0, WarmGate::Holding), WarmStep::Waiting);
    assert_eq!(warmer.next_wake(), Some(t0 + ttl / 2));
}

#[test]
fn budget_halts_once_per_idle_stretch_and_a_real_request_renews_it() {
    let t0 = Instant::now();
    let interval = TTL - PING_MARGIN;
    let mut warmer = warmer(t0);
    let mut at = t0;
    for _ in 0..MAX_PINGS {
        at += interval;
        assert_eq!(ping_at(&mut warmer, at, PingResult::Read), None);
    }

    at += interval;
    assert_eq!(
        warmer.step(at, WarmGate::Holding),
        WarmStep::Halted(WarmHalt::Budget)
    );
    assert_eq!(warmer.step(at, WarmGate::Holding), WarmStep::Rest);

    warmer.record_request(at);
    assert_eq!(ping_at(&mut warmer, at + interval, PingResult::Read), None);
}

// Covers: a model whose cache prices make every ping a loss. The hold must
// report the skip at once instead of showing a 0/0 counter until the first
// ping is due, and a real request must not repeat the notice.
// Owner: cache-warm scheduler
#[test]
fn a_zero_budget_halts_at_once_and_stays_halted_across_requests() {
    let t0 = Instant::now();
    let mut warmer = warmer(t0);
    warmer.limits.max_pings = 0;

    assert_eq!(
        warmer.step(t0, WarmGate::Holding),
        WarmStep::Halted(WarmHalt::Unprofitable)
    );
    let t1 = t0 + Duration::from_secs(60);
    warmer.record_request(t1);
    assert_eq!(warmer.step(t1, WarmGate::Holding), WarmStep::Rest);
}

#[test]
fn a_missed_ping_stays_halted_until_the_hold_is_released() {
    let t0 = Instant::now();
    let interval = TTL - PING_MARGIN;
    let mut warmer = warmer(t0);

    assert_eq!(
        ping_at(&mut warmer, t0 + interval, PingResult::Miss),
        Some(WarmHalt::Miss)
    );
    // A real turn does not lift it: the same prefix would miss again.
    let t1 = t0 + interval * 2;
    warmer.record_request(t1);
    assert_eq!(
        warmer.step(t1 + interval, WarmGate::Holding),
        WarmStep::Rest
    );

    // A new hold starts clean.
    assert_eq!(
        warmer.step(t1 + interval, WarmGate::Released),
        WarmStep::Rest
    );
    assert_eq!(
        warmer.step(t1 + interval, WarmGate::Holding),
        WarmStep::Ping
    );
}

#[test]
fn an_unsupported_route_is_reported_once() {
    let t0 = Instant::now();
    let mut warmer = warmer(t0);

    assert_eq!(
        warmer.step(t0, WarmGate::Unsupported),
        WarmStep::Halted(WarmHalt::Unsupported)
    );
    assert_eq!(warmer.step(t0, WarmGate::Unsupported), WarmStep::Rest);
}

#[test]
fn ping_results_are_judged_by_cache_reads_before_errors() {
    struct Case {
        name: &'static str,
        cache_read: Option<u64>,
        cache_write: Option<u64>,
        input: Option<u64>,
        output: Option<u64>,
        failed: bool,
        expected: PingResult,
    }
    let cases = [
        Case {
            name: "capped ping ends incomplete but read the prefix",
            cache_read: Some(22_357),
            cache_write: None,
            input: Some(43),
            output: Some(1),
            failed: true,
            expected: PingResult::Read,
        },
        Case {
            name: "full rewrite is a miss",
            cache_read: Some(0),
            cache_write: Some(22_357),
            input: Some(0),
            output: Some(1),
            failed: true,
            expected: PingResult::Miss,
        },
        Case {
            name: "route without cache accounting cannot prove a read",
            cache_read: None,
            cache_write: None,
            input: None,
            output: Some(1),
            failed: false,
            expected: PingResult::Miss,
        },
        Case {
            name: "failure before any usage",
            cache_read: None,
            cache_write: None,
            input: None,
            output: Some(1),
            failed: true,
            expected: PingResult::Failed,
        },
        Case {
            name: "short cached head with a large uncached suffix is a miss",
            cache_read: Some(1_024),
            cache_write: None,
            input: Some(22_000),
            output: Some(1),
            failed: true,
            expected: PingResult::Miss,
        },
        Case {
            name: "partial cache write larger than breakpoint noise is a miss",
            cache_read: Some(22_357),
            cache_write: Some(5_000),
            input: Some(43),
            output: Some(1),
            failed: true,
            expected: PingResult::Miss,
        },
        Case {
            name: "host ignored the output cap and completed a response",
            cache_read: Some(22_357),
            cache_write: None,
            input: Some(43),
            output: Some(812),
            failed: false,
            expected: PingResult::Uncapped,
        },
        Case {
            name: "unreported output cannot prove the cap",
            cache_read: Some(22_357),
            cache_write: None,
            input: Some(43),
            output: None,
            failed: true,
            expected: PingResult::Uncapped,
        },
    ];
    for case in cases {
        let usage = ModelUsage {
            cache_read_tokens: case.cache_read,
            cache_write_tokens: case.cache_write,
            input_tokens: case.input,
            output_tokens: case.output,
            ..ModelUsage::default()
        };
        assert_eq!(
            PingResult::classify(&crate::app::interactive_runtime::CacheWarmOutcome {
                usage,
                error: case.failed.then(|| "incomplete".into()),
            }),
            case.expected,
            "{}",
            case.name
        );
    }
}

// Covers: the ping budget follows each model's cache prices, including hosts
// with no write premium, and falls back when reads are unpriced.
// Owner: cache-warm budget policy
#[test]
fn ping_budget_is_half_the_break_even_at_catalog_cache_prices() {
    // Prices in USD micros per million tokens: (input, cache read, cache write).
    let cases = [
        (
            "gpt-5.6",
            Some((5_000_000, Some(500_000), Some(6_250_000))),
            5,
        ),
        (
            "gpt-5.5, no write premium",
            Some((5_000_000, Some(500_000), None)),
            4,
        ),
        (
            "0.05x reads",
            Some((5_000_000, Some(250_000), Some(6_250_000))),
            12,
        ),
        (
            "no cache discount",
            Some((5_000_000, Some(5_000_000), None)),
            0,
        ),
        (
            "unpriced reads",
            Some((5_000_000, None, Some(6_250_000))),
            FALLBACK_PING_BUDGET,
        ),
        ("no catalog row", None, FALLBACK_PING_BUDGET),
    ];
    for (name, prices, expected) in cases {
        let metadata = prices.map(|(input, read, write)| ModelMetadata {
            cost_default: Some(ModelCost {
                input_micros_per_m: Some(input),
                output_micros_per_m: None,
                cache_read_micros_per_m: read,
                cache_write_micros_per_m: write,
            }),
            ..ModelMetadata::default()
        });
        assert_eq!(ping_budget(metadata.as_ref()), expected, "{name}");
    }
}

// Covers: a process exits while its ping is in flight. The app stops
// observing that ping, so the next hold must not wait on it.
// Owner: cache-warm scheduler
#[test]
fn release_drops_an_in_flight_ping_so_a_new_hold_can_warm() {
    let t0 = Instant::now();
    let interval = TTL - PING_MARGIN;
    let mut warmer = warmer(t0);

    assert_eq!(
        warmer.step(t0 + interval, WarmGate::Holding),
        WarmStep::Ping
    );
    assert_eq!(
        warmer.step(t0 + interval, WarmGate::Released),
        WarmStep::Rest
    );
    assert_eq!(
        warmer.step(t0 + interval, WarmGate::Holding),
        WarmStep::Ping
    );
}

// Covers: `/clear` while a ping is in flight. The provider already billed the
// prefix read, so the send must finish and reach the usage ledger, while its
// result stays out of the new conversation.
// Owner: App cache-warm wiring (PTY cannot observe the detached send).
#[tokio::test]
async fn reset_lets_an_in_flight_ping_finish_unobserved() {
    let mut app = crate::tui::tests::test_app();
    let (release, gate) = tokio::sync::oneshot::channel::<()>();
    let (recorded, ledger) = tokio::sync::oneshot::channel::<()>();
    app.spawn_cache_warm_ping(async move {
        gate.await.unwrap();
        recorded.send(()).unwrap();
        super::CacheWarmOutcome::default()
    });

    app.reset_cache_warm();
    assert!(!app.tasks.has_pending());

    release.send(()).unwrap();
    assert_eq!(ledger.await, Ok(()));
    assert!(!app.apply_finished_ui_tasks());
}

// Covers: a provider switch while a ping is in flight. The ping was billed in
// this conversation, so its spend must reach the totals, but its result
// describes the old route and must not halt warming on the new one.
// Owner: App cache-warm wiring.
#[tokio::test]
async fn an_orphaned_ping_counts_its_spend_but_cannot_halt_the_new_route() {
    let mut app = crate::tui::tests::test_app();
    app.spawn_cache_warm_ping(async {
        super::CacheWarmOutcome {
            usage: ModelUsage {
                cost_usd_micros: Some(700),
                ..ModelUsage::default()
            },
            error: Some("connection reset".into()),
        }
    });
    app.cache_warm_provider_replaced();
    while !app.tasks.has_finished() {
        tokio::task::yield_now().await;
    }

    assert!(!app.apply_finished_ui_tasks());
    assert_eq!(app.usage.extra.cache_warm, 700);
    assert_eq!(app.usage.cache_warm.halt, None);
}
