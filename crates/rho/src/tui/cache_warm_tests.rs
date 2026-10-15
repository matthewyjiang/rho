use std::time::{Duration, Instant};

use pretty_assertions::assert_eq;
use rho_providers::model::ModelUsage;

use super::{
    CacheWarmer, PingResult, WarmGate, WarmHalt, WarmStep, MAX_PINGS_PER_IDLE, PING_MARGIN,
};

const TTL: Duration = Duration::from_secs(300);

struct Warmer {
    scheduler: CacheWarmer,
    clock: crate::tui::cache_stats::CacheStatsTracker,
    ttl: Duration,
}

impl Default for Warmer {
    fn default() -> Self {
        Self {
            scheduler: CacheWarmer::default(),
            clock: Default::default(),
            ttl: TTL,
        }
    }
}

impl Warmer {
    fn step(&mut self, now: Instant, gate: WarmGate) -> WarmStep {
        self.scheduler
            .step(now, gate, self.clock.last_prefix_read(), self.ttl)
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
    let mut warmer = Warmer {
        ttl,
        ..Warmer::default()
    };
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
    for _ in 0..MAX_PINGS_PER_IDLE {
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
        failed: bool,
        expected: PingResult,
    }
    let cases = [
        Case {
            name: "capped ping ends incomplete but read the prefix",
            cache_read: Some(22_357),
            cache_write: None,
            input: Some(43),
            failed: true,
            expected: PingResult::Read,
        },
        Case {
            name: "full rewrite is a miss",
            cache_read: Some(0),
            cache_write: Some(22_357),
            input: Some(0),
            failed: true,
            expected: PingResult::Miss,
        },
        Case {
            name: "route without cache accounting cannot prove a read",
            cache_read: None,
            cache_write: None,
            input: None,
            failed: false,
            expected: PingResult::Miss,
        },
        Case {
            name: "failure before any usage",
            cache_read: None,
            cache_write: None,
            input: None,
            failed: true,
            expected: PingResult::Failed,
        },
        Case {
            name: "short cached head with a large uncached suffix is a miss",
            cache_read: Some(1_024),
            cache_write: None,
            input: Some(22_000),
            failed: true,
            expected: PingResult::Miss,
        },
        Case {
            name: "partial cache write larger than breakpoint noise is a miss",
            cache_read: Some(22_357),
            cache_write: Some(5_000),
            input: Some(43),
            failed: true,
            expected: PingResult::Miss,
        },
    ];
    for case in cases {
        let usage = ModelUsage {
            cache_read_tokens: case.cache_read,
            cache_write_tokens: case.cache_write,
            input_tokens: case.input,
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
