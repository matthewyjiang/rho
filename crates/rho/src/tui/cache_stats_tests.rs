use std::time::{Duration, Instant};

use pretty_assertions::assert_eq;
use rho_providers::model::{models_dev::ModelCost, ModelMetadata, ModelUsage};
use rho_sdk::{ModelCallMetrics, ModelCallProfile, ReasoningLevel};

use super::{
    CacheRebilled, CacheStatsTracker, CACHE_MISS_NOISE_FLOOR_TOKENS, DEFAULT_PROMPT_CACHE_TTL,
    SIGNIFICANT_MISS_TOKENS,
};

/// One completed model call, as the tracker observes it.
#[derive(Clone, Copy)]
struct Request {
    model: &'static str,
    /// Uncached input tokens billed at the full rate.
    input: u64,
    /// `None` means the provider does not report cache accounting at all.
    cache_read: Option<u64>,
    cache_write: Option<u64>,
    /// Seconds from the session start to when this call completed.
    at_secs: u64,
    /// Wall time of the call itself, used to derive its start instant.
    latency_secs: u64,
}

impl Request {
    /// Back-to-back request on the same model, one second after the last one.
    const fn next(at_secs: u64, input: u64, cache_read: u64) -> Self {
        Self {
            model: "claude",
            input,
            cache_read: Some(cache_read),
            cache_write: Some(0),
            at_secs,
            latency_secs: 1,
        }
    }

    const fn warm(at_secs: u64, cache_write: u64) -> Self {
        Self {
            model: "claude",
            input: 0,
            cache_read: Some(0),
            cache_write: Some(cache_write),
            at_secs,
            latency_secs: 1,
        }
    }

    const fn on_model(mut self, model: &'static str) -> Self {
        self.model = model;
        self
    }

    /// Provider reports no cache fields at all (local model, plain OpenAI-compatible host).
    const fn without_cache_reporting(mut self) -> Self {
        self.cache_read = None;
        self.cache_write = None;
        self
    }
}

fn play(
    tracker: &mut CacheStatsTracker,
    t0: Instant,
    requests: &[Request],
    metadata: Option<&ModelMetadata>,
) {
    for request in requests {
        tracker.usage_updated(&ModelUsage {
            input_tokens: Some(request.input),
            cache_read_tokens: request.cache_read,
            cache_write_tokens: request.cache_write,
            ..ModelUsage::default()
        });
        tracker.record_request(
            &ModelCallProfile {
                provider: "anthropic".into(),
                model: request.model.into(),
                reasoning: ReasoningLevel::Off,
                service_tier: None,
            },
            ModelCallMetrics {
                output_tokens: None,
                time_to_first_token: None,
                generation_time: None,
                total_latency: Duration::from_secs(request.latency_secs),
                generation_output_tokens: None,
            },
            metadata,
            t0 + Duration::from_secs(request.at_secs),
            DEFAULT_PROMPT_CACHE_TTL,
        );
    }
}

fn priced_metadata() -> ModelMetadata {
    let mut metadata = ModelMetadata::default();
    metadata.cost_default = Some(ModelCost {
        input_micros_per_m: Some(1_000_000),
        output_micros_per_m: Some(2_000_000),
        cache_read_micros_per_m: Some(100_000),
        cache_write_micros_per_m: Some(1_250_000),
    });
    metadata
}

/// High input/cache-read spread so a miss below the token tripwire can still
/// cross the $0.10 notice floor.
fn expensive_metadata() -> ModelMetadata {
    let mut metadata = ModelMetadata::default();
    metadata.cost_default = Some(ModelCost {
        input_micros_per_m: Some(20_000_000),
        output_micros_per_m: None,
        cache_read_micros_per_m: Some(0),
        cache_write_micros_per_m: None,
    });
    metadata
}

struct Case {
    name: &'static str,
    requests: Vec<Request>,
    metadata: Option<fn() -> ModelMetadata>,
    rebilled: CacheRebilled,
}

const NONE: CacheRebilled = CacheRebilled {
    missed_tokens: 0,
    miss_count: 0,
    extra_cost_usd_micros: 0,
    unpriced_miss_count: 0,
};

// Covers: first request, full hits, the noise floor on both sides, providers
// that never report cache, model switch, idle TTL, shrunken prompts, and
// misses on either side of the notice tripwires.
// Owner: tui cache-miss policy
#[test]
fn tracker_counts_only_real_misses() {
    let cases = vec![
        Case {
            name: "first request is never a miss",
            requests: vec![Request::next(0, 50_000, 0)],
            metadata: None,
            rebilled: NONE,
        },
        Case {
            name: "full cache hit is not a miss",
            requests: vec![Request::warm(0, 40_000), Request::next(2, 200, 40_000)],
            metadata: None,
            rebilled: NONE,
        },
        Case {
            name: "miss at the noise floor is ignored",
            requests: vec![
                Request::warm(0, 10_000),
                Request::next(
                    2,
                    CACHE_MISS_NOISE_FLOOR_TOKENS,
                    10_000 - CACHE_MISS_NOISE_FLOOR_TOKENS,
                ),
            ],
            metadata: None,
            rebilled: NONE,
        },
        Case {
            name: "one token over the noise floor counts",
            requests: vec![
                Request::warm(0, 10_000),
                Request::next(
                    2,
                    CACHE_MISS_NOISE_FLOOR_TOKENS + 1,
                    10_000 - CACHE_MISS_NOISE_FLOOR_TOKENS - 1,
                ),
            ],
            metadata: None,
            rebilled: CacheRebilled {
                missed_tokens: CACHE_MISS_NOISE_FLOOR_TOKENS + 1,
                miss_count: 1,
                extra_cost_usd_micros: 0,
                unpriced_miss_count: 1,
            },
        },
        Case {
            name: "providers that never report cache are never billed a miss",
            requests: vec![
                Request::next(0, 40_000, 0).without_cache_reporting(),
                Request::next(2, 60_000, 0).without_cache_reporting(),
            ],
            metadata: None,
            rebilled: NONE,
        },
        Case {
            // Regression: a session-scoped "saw cache" latch used to bill every
            // later request on a non-reporting provider as a full miss.
            name: "switching to a non-reporting provider after cache activity stays silent",
            requests: vec![
                Request::warm(0, 50_000),
                Request::next(2, 200, 50_000),
                Request::next(4, 60_000, 0)
                    .on_model("local")
                    .without_cache_reporting(),
                Request::next(6, 70_000, 0)
                    .on_model("local")
                    .without_cache_reporting(),
            ],
            metadata: None,
            rebilled: NONE,
        },
        Case {
            name: "a miss after a model switch is counted",
            requests: vec![
                Request::warm(0, SIGNIFICANT_MISS_TOKENS),
                Request::next(2, SIGNIFICANT_MISS_TOKENS, 0).on_model("sonnet"),
            ],
            metadata: None,
            rebilled: CacheRebilled {
                missed_tokens: SIGNIFICANT_MISS_TOKENS,
                miss_count: 1,
                extra_cost_usd_micros: 0,
                unpriced_miss_count: 1,
            },
        },
        Case {
            name: "a miss after idling past the TTL is counted",
            requests: vec![
                Request::warm(0, SIGNIFICANT_MISS_TOKENS),
                Request::next(
                    DEFAULT_PROMPT_CACHE_TTL.as_secs() + 61,
                    SIGNIFICANT_MISS_TOKENS,
                    0,
                ),
            ],
            metadata: None,
            rebilled: CacheRebilled {
                missed_tokens: SIGNIFICANT_MISS_TOKENS,
                miss_count: 1,
                extra_cost_usd_micros: 0,
                unpriced_miss_count: 1,
            },
        },
        Case {
            name: "a shrunken prompt only re-bills what it actually sent",
            requests: vec![Request::warm(0, 50_000), Request::next(2, 8_000, 0)],
            metadata: None,
            rebilled: CacheRebilled {
                missed_tokens: 8_000,
                miss_count: 1,
                extra_cost_usd_micros: 0,
                unpriced_miss_count: 1,
            },
        },
        Case {
            name: "a miss at the token tripwire is counted",
            requests: vec![
                Request::warm(0, SIGNIFICANT_MISS_TOKENS),
                Request::next(2, SIGNIFICANT_MISS_TOKENS, 0),
            ],
            metadata: None,
            rebilled: CacheRebilled {
                missed_tokens: SIGNIFICANT_MISS_TOKENS,
                miss_count: 1,
                extra_cost_usd_micros: 0,
                unpriced_miss_count: 1,
            },
        },
        Case {
            name: "a priced miss below the token tripwire is counted",
            requests: vec![Request::warm(0, 10_000), Request::next(2, 5_001, 4_999)],
            metadata: Some(expensive_metadata),
            rebilled: CacheRebilled {
                missed_tokens: 5_001,
                miss_count: 1,
                extra_cost_usd_micros: 100_020,
                unpriced_miss_count: 0,
            },
        },
        Case {
            name: "a small priced miss is counted",
            requests: vec![Request::warm(0, 10_000), Request::next(2, 2_000, 8_000)],
            metadata: Some(priced_metadata),
            rebilled: CacheRebilled {
                missed_tokens: 2_000,
                miss_count: 1,
                extra_cost_usd_micros: 1_800,
                unpriced_miss_count: 0,
            },
        },
        Case {
            name: "a missed prefix written back to cache uses the write rate",
            requests: vec![
                Request::warm(0, 10_000),
                Request {
                    model: "claude",
                    input: 0,
                    cache_read: Some(0),
                    cache_write: Some(2_000),
                    at_secs: 2,
                    latency_secs: 1,
                },
            ],
            metadata: Some(priced_metadata),
            rebilled: CacheRebilled {
                missed_tokens: 2_000,
                miss_count: 1,
                extra_cost_usd_micros: 2_300,
                unpriced_miss_count: 0,
            },
        },
    ];

    let t0 = Instant::now() + Duration::from_secs(3_600);
    for case in &cases {
        let mut tracker = CacheStatsTracker::default();
        let metadata = case.metadata.map(|build| build());
        play(&mut tracker, t0, &case.requests, metadata.as_ref());

        assert_eq!(tracker.rebilled(), &case.rebilled, "{}", case.name);
    }
}

// Covers: compaction rewrites the prefix, so the next request cannot be a miss,
// while session totals survive.
// Owner: tui cache-miss policy
#[test]
fn compaction_clears_the_prefix_but_keeps_session_totals() {
    let t0 = Instant::now();
    let mut tracker = CacheStatsTracker::default();

    play(
        &mut tracker,
        t0,
        &[
            Request::warm(0, 50_000),
            Request::next(2, SIGNIFICANT_MISS_TOKENS, 30_000),
        ],
        None,
    );
    let after_miss = tracker.rebilled().clone();
    assert_eq!(after_miss.miss_count, 1);

    tracker.prompt_prefix_reset();
    play(&mut tracker, t0, &[Request::next(4, 60_000, 0)], None);

    assert_eq!(tracker.rebilled(), &after_miss);
}

// Covers: a request that never reported usage is not sampled, and a stale
// delta is not reused by the next call.
// Owner: tui cache-miss policy
#[test]
fn a_request_without_reported_usage_is_not_sampled() {
    let t0 = Instant::now();
    let mut tracker = CacheStatsTracker::default();

    let profile = ModelCallProfile {
        provider: "anthropic".into(),
        model: "claude".into(),
        reasoning: ReasoningLevel::Off,
        service_tier: None,
    };
    let metrics = ModelCallMetrics {
        output_tokens: None,
        time_to_first_token: None,
        generation_time: None,
        total_latency: Duration::from_secs(1),
        generation_output_tokens: None,
    };

    tracker.record_request(&profile, metrics, None, t0, DEFAULT_PROMPT_CACHE_TTL);
    assert_eq!(tracker.rebilled(), &NONE);

    play(&mut tracker, t0, &[Request::warm(1, 50_000)], None);
    // Second call reports no new usage, so the warm sample must not be replayed.
    tracker.record_request(
        &profile,
        metrics,
        None,
        t0 + Duration::from_secs(2),
        DEFAULT_PROMPT_CACHE_TTL,
    );

    assert_eq!(tracker.rebilled(), &NONE);
}
