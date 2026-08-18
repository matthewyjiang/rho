use std::time::{Duration, Instant};

use pretty_assertions::assert_eq;
use rho_providers::model::{models_dev::ModelCost, ModelMetadata, ModelUsage};

use super::{
    notice_text, CacheMissCause, CacheMissNotice, CacheRebilled, CacheStatsTracker, ModelKey,
    CACHE_MISS_NOISE_FLOOR_TOKENS, PROVIDER_CACHE_TTL_HINT, SIGNIFICANT_MISS_TOKENS,
};

fn model(id: &str) -> ModelKey {
    ModelKey::new("anthropic", id)
}

fn usage(input: u64, cache_read: Option<u64>, cache_write: Option<u64>) -> ModelUsage {
    ModelUsage {
        input_tokens: Some(input),
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
        ..ModelUsage::default()
    }
}

fn priced_metadata() -> ModelMetadata {
    ModelMetadata {
        cost_default: Some(ModelCost {
            input_micros_per_m: Some(1_000_000),
            output_micros_per_m: Some(2_000_000),
            cache_read_micros_per_m: Some(100_000),
            cache_write_micros_per_m: None,
        }),
        ..ModelMetadata::default()
    }
}

/// High input/cache-read spread so a miss below the token tripwire can still
/// cross the $0.10 notice floor.
fn expensive_metadata() -> ModelMetadata {
    ModelMetadata {
        cost_default: Some(ModelCost {
            input_micros_per_m: Some(20_000_000),
            output_micros_per_m: None,
            cache_read_micros_per_m: Some(0),
            cache_write_micros_per_m: None,
        }),
        ..ModelMetadata::default()
    }
}

fn commit(
    tracker: &mut CacheStatsTracker,
    key: ModelKey,
    step_usage: ModelUsage,
    started: Instant,
    completed: Instant,
    metadata: Option<&ModelMetadata>,
    retry: bool,
) {
    tracker.step_started(key, started, metadata);
    if retry {
        tracker.attempt_restarted();
    }
    tracker.usage_updated(&step_usage, completed);
    tracker.run_finished(metadata, completed);
}

// Covers: first request, full hits, noise floor, silent providers, resets,
// model switch, idle TTL, retry skip, shrunken prompts, and notice gating.
// Owner: tui cache-miss policy
#[test]
fn tracker_counts_only_real_misses() {
    let t0 = Instant::now();
    struct Case {
        name: &'static str,
        run: fn(&mut CacheStatsTracker, Instant),
        rebilled: CacheRebilled,
        notices: usize,
        last_cause: Option<CacheMissCause>,
    }

    let cases = [
        Case {
            name: "first request is never a miss",
            run: |tracker, t0| {
                commit(
                    tracker,
                    model("claude"),
                    usage(50_000, Some(0), Some(50_000)),
                    t0,
                    t0,
                    None,
                    false,
                );
            },
            rebilled: CacheRebilled::default(),
            notices: 0,
            last_cause: None,
        },
        Case {
            name: "full cache hit is not a miss",
            run: |tracker, t0| {
                commit(
                    tracker,
                    model("claude"),
                    usage(1_000, Some(0), Some(40_000)),
                    t0,
                    t0,
                    None,
                    false,
                );
                commit(
                    tracker,
                    model("claude"),
                    usage(200, Some(40_000), Some(200)),
                    t0 + Duration::from_secs(1),
                    t0 + Duration::from_secs(2),
                    None,
                    false,
                );
            },
            rebilled: CacheRebilled::default(),
            notices: 0,
            last_cause: None,
        },
        Case {
            name: "miss at the 1024-token floor is noise",
            run: |tracker, t0| {
                commit(
                    tracker,
                    model("claude"),
                    usage(0, Some(0), Some(10_000)),
                    t0,
                    t0,
                    None,
                    false,
                );
                commit(
                    tracker,
                    model("claude"),
                    usage(
                        CACHE_MISS_NOISE_FLOOR_TOKENS,
                        Some(10_000 - CACHE_MISS_NOISE_FLOOR_TOKENS),
                        Some(0),
                    ),
                    t0 + Duration::from_secs(1),
                    t0 + Duration::from_secs(2),
                    None,
                    false,
                );
            },
            rebilled: CacheRebilled::default(),
            notices: 0,
            last_cause: None,
        },
        Case {
            name: "one token over the floor counts",
            run: |tracker, t0| {
                commit(
                    tracker,
                    model("claude"),
                    usage(0, Some(0), Some(10_000)),
                    t0,
                    t0,
                    None,
                    false,
                );
                commit(
                    tracker,
                    model("claude"),
                    usage(
                        CACHE_MISS_NOISE_FLOOR_TOKENS + 1,
                        Some(10_000 - CACHE_MISS_NOISE_FLOOR_TOKENS - 1),
                        Some(0),
                    ),
                    t0 + Duration::from_secs(1),
                    t0 + Duration::from_secs(2),
                    None,
                    false,
                );
            },
            rebilled: CacheRebilled {
                missed_tokens: CACHE_MISS_NOISE_FLOOR_TOKENS + 1,
                miss_count: 1,
                extra_cost_usd_micros: None,
            },
            notices: 0,
            last_cause: None,
        },
        Case {
            name: "providers that never report cache stay silent",
            run: |tracker, t0| {
                commit(
                    tracker,
                    model("local"),
                    usage(40_000, None, None),
                    t0,
                    t0,
                    None,
                    false,
                );
                commit(
                    tracker,
                    model("local"),
                    usage(42_000, None, None),
                    t0 + Duration::from_secs(1),
                    t0 + Duration::from_secs(2),
                    None,
                    false,
                );
            },
            rebilled: CacheRebilled::default(),
            notices: 0,
            last_cause: None,
        },
        Case {
            name: "zero cache after a cached request is a miss",
            run: |tracker, t0| {
                commit(
                    tracker,
                    model("claude"),
                    usage(0, Some(0), Some(30_000)),
                    t0,
                    t0,
                    None,
                    false,
                );
                commit(
                    tracker,
                    model("claude"),
                    usage(30_000, Some(0), Some(0)),
                    t0 + Duration::from_secs(1),
                    t0 + Duration::from_secs(2),
                    None,
                    false,
                );
            },
            rebilled: CacheRebilled {
                missed_tokens: 30_000,
                miss_count: 1,
                extra_cost_usd_micros: None,
            },
            notices: 1,
            last_cause: Some(CacheMissCause::Unattributed),
        },
        Case {
            name: "compaction resets comparison but keeps totals",
            run: |tracker, t0| {
                commit(
                    tracker,
                    model("claude"),
                    usage(0, Some(0), Some(30_000)),
                    t0,
                    t0,
                    None,
                    false,
                );
                commit(
                    tracker,
                    model("claude"),
                    usage(30_000, Some(0), Some(0)),
                    t0 + Duration::from_secs(1),
                    t0 + Duration::from_secs(2),
                    None,
                    false,
                );
                tracker.compaction_reset();
                commit(
                    tracker,
                    model("claude"),
                    usage(8_000, Some(0), Some(8_000)),
                    t0 + Duration::from_secs(3),
                    t0 + Duration::from_secs(4),
                    None,
                    false,
                );
            },
            rebilled: CacheRebilled {
                missed_tokens: 30_000,
                miss_count: 1,
                extra_cost_usd_micros: None,
            },
            notices: 1,
            last_cause: Some(CacheMissCause::Unattributed),
        },
        Case {
            name: "full reset clears totals",
            run: |tracker, t0| {
                commit(
                    tracker,
                    model("claude"),
                    usage(0, Some(0), Some(30_000)),
                    t0,
                    t0,
                    None,
                    false,
                );
                commit(
                    tracker,
                    model("claude"),
                    usage(30_000, Some(0), Some(0)),
                    t0 + Duration::from_secs(1),
                    t0 + Duration::from_secs(2),
                    None,
                    false,
                );
                tracker.reset();
            },
            rebilled: CacheRebilled::default(),
            notices: 0,
            last_cause: None,
        },
        Case {
            name: "model switch is a counted miss with that cause",
            run: |tracker, t0| {
                commit(
                    tracker,
                    model("claude"),
                    usage(0, Some(0), Some(25_000)),
                    t0,
                    t0,
                    None,
                    false,
                );
                commit(
                    tracker,
                    ModelKey::new("openai", "gpt"),
                    usage(25_000, Some(0), Some(0)),
                    t0 + Duration::from_secs(1),
                    t0 + Duration::from_secs(2),
                    None,
                    false,
                );
            },
            rebilled: CacheRebilled {
                missed_tokens: 25_000,
                miss_count: 1,
                extra_cost_usd_micros: None,
            },
            notices: 1,
            last_cause: Some(CacheMissCause::ModelSwitch),
        },
        Case {
            name: "idle past the TTL hint names the gap",
            run: |tracker, t0| {
                commit(
                    tracker,
                    model("claude"),
                    usage(0, Some(0), Some(25_000)),
                    t0,
                    t0,
                    None,
                    false,
                );
                let later = t0 + PROVIDER_CACHE_TTL_HINT + Duration::from_secs(30);
                commit(
                    tracker,
                    model("claude"),
                    usage(25_000, Some(0), Some(0)),
                    later,
                    later + Duration::from_secs(1),
                    None,
                    false,
                );
            },
            rebilled: CacheRebilled {
                missed_tokens: 25_000,
                miss_count: 1,
                extra_cost_usd_micros: None,
            },
            notices: 1,
            last_cause: Some(CacheMissCause::Idle(
                PROVIDER_CACHE_TTL_HINT + Duration::from_secs(30),
            )),
        },
        Case {
            name: "idle under the TTL hint stays unattributed",
            run: |tracker, t0| {
                commit(
                    tracker,
                    model("claude"),
                    usage(0, Some(0), Some(25_000)),
                    t0,
                    t0,
                    None,
                    false,
                );
                let later = t0 + PROVIDER_CACHE_TTL_HINT - Duration::from_secs(1);
                commit(
                    tracker,
                    model("claude"),
                    usage(25_000, Some(0), Some(0)),
                    later,
                    later + Duration::from_secs(1),
                    None,
                    false,
                );
            },
            rebilled: CacheRebilled {
                missed_tokens: 25_000,
                miss_count: 1,
                extra_cost_usd_micros: None,
            },
            notices: 1,
            last_cause: Some(CacheMissCause::Unattributed),
        },
        Case {
            name: "retry-tainted samples are not counted",
            run: |tracker, t0| {
                commit(
                    tracker,
                    model("claude"),
                    usage(0, Some(0), Some(30_000)),
                    t0,
                    t0,
                    None,
                    false,
                );
                commit(
                    tracker,
                    model("claude"),
                    usage(30_000, Some(0), Some(0)),
                    t0 + Duration::from_secs(1),
                    t0 + Duration::from_secs(2),
                    None,
                    true,
                );
            },
            rebilled: CacheRebilled::default(),
            notices: 0,
            last_cause: None,
        },
        Case {
            name: "shrunken prompts use the overlapping prefix",
            run: |tracker, t0| {
                commit(
                    tracker,
                    model("claude"),
                    usage(0, Some(0), Some(40_000)),
                    t0,
                    t0,
                    None,
                    false,
                );
                commit(
                    tracker,
                    model("claude"),
                    usage(8_000, Some(0), Some(0)),
                    t0 + Duration::from_secs(1),
                    t0 + Duration::from_secs(2),
                    None,
                    false,
                );
            },
            rebilled: CacheRebilled {
                missed_tokens: 8_000,
                miss_count: 1,
                extra_cost_usd_micros: None,
            },
            notices: 0,
            last_cause: None,
        },
        Case {
            name: "token tripwire emits a notice",
            run: |tracker, t0| {
                commit(
                    tracker,
                    model("claude"),
                    usage(0, Some(0), Some(SIGNIFICANT_MISS_TOKENS)),
                    t0,
                    t0,
                    None,
                    false,
                );
                commit(
                    tracker,
                    model("claude"),
                    usage(SIGNIFICANT_MISS_TOKENS, Some(0), Some(0)),
                    t0 + Duration::from_secs(1),
                    t0 + Duration::from_secs(2),
                    None,
                    false,
                );
            },
            rebilled: CacheRebilled {
                missed_tokens: SIGNIFICANT_MISS_TOKENS,
                miss_count: 1,
                extra_cost_usd_micros: None,
            },
            notices: 1,
            last_cause: Some(CacheMissCause::Unattributed),
        },
        Case {
            name: "priced cost tripwire emits a notice below the token floor",
            run: |tracker, t0| {
                let metadata = expensive_metadata();
                commit(
                    tracker,
                    model("claude"),
                    usage(0, Some(0), Some(10_000)),
                    t0,
                    t0,
                    Some(&metadata),
                    false,
                );
                commit(
                    tracker,
                    model("claude"),
                    usage(5_001, Some(4_999), Some(0)),
                    t0 + Duration::from_secs(1),
                    t0 + Duration::from_secs(2),
                    Some(&metadata),
                    false,
                );
            },
            rebilled: CacheRebilled {
                missed_tokens: 5_001,
                miss_count: 1,
                extra_cost_usd_micros: Some(100_020),
            },
            notices: 1,
            last_cause: Some(CacheMissCause::Unattributed),
        },
        Case {
            name: "priced miss below both tripwires is counted without a notice",
            run: |tracker, t0| {
                let metadata = priced_metadata();
                commit(
                    tracker,
                    model("claude"),
                    usage(0, Some(0), Some(10_000)),
                    t0,
                    t0,
                    Some(&metadata),
                    false,
                );
                commit(
                    tracker,
                    model("claude"),
                    usage(2_000, Some(8_000), Some(0)),
                    t0 + Duration::from_secs(1),
                    t0 + Duration::from_secs(2),
                    Some(&metadata),
                    false,
                );
            },
            rebilled: CacheRebilled {
                missed_tokens: 2_000,
                miss_count: 1,
                extra_cost_usd_micros: Some(1_800),
            },
            notices: 0,
            last_cause: None,
        },
    ];

    for case in cases {
        let mut tracker = CacheStatsTracker::default();
        (case.run)(&mut tracker, t0);
        assert_eq!(tracker.rebilled(), &case.rebilled, "{}", case.name);
        let notices = tracker.take_turn_notices();
        assert_eq!(notices.len(), case.notices, "{}", case.name);
        assert_eq!(
            notices.last().map(|notice| notice.cause),
            case.last_cause,
            "{}",
            case.name
        );
        assert!(tracker.take_turn_notices().is_empty(), "{}", case.name);
    }
}

// Covers: notice copy for each observable cause and the unpriced form.
// Owner: tui cache-miss policy
#[test]
fn notice_text_names_cause_and_optional_cost() {
    assert_eq!(
        notice_text(&CacheMissNotice {
            missed_tokens: 45_200,
            extra_cost_usd_micros: Some(320_000),
            cause: CacheMissCause::Unattributed,
        }),
        "cache miss: 45.2K tokens re-billed (~$0.320)"
    );
    assert_eq!(
        notice_text(&CacheMissNotice {
            missed_tokens: 45_200,
            extra_cost_usd_micros: Some(320_000),
            cause: CacheMissCause::ModelSwitch,
        }),
        "cache miss after model switch: 45.2K tokens re-billed (~$0.320)"
    );
    assert_eq!(
        notice_text(&CacheMissNotice {
            missed_tokens: 45_200,
            extra_cost_usd_micros: None,
            cause: CacheMissCause::Idle(Duration::from_secs(12 * 60)),
        }),
        "cache miss after 12m idle (cache TTL is about 5m): 45.2K tokens re-billed"
    );
}
