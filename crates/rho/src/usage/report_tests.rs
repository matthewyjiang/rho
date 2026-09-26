use std::path::Path;

use chrono::{FixedOffset, NaiveDate, NaiveDateTime};
use pretty_assertions::assert_eq;
use rho_providers::model::{models_dev::ModelCost, ModelMetadata, ModelUsage};
use tempfile::TempDir;

use super::*;
use crate::usage::{RequestOutcome, SqliteUsageRecorder, UsageEvent, UsageRecorder};

/// 2026-03-10 12:00 UTC. Tests read the ledger in UTC.
const NOW_MS: i64 = 1_773_144_000_000;
const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 24 * HOUR_MS;

fn now() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 3, 10)
        .unwrap()
        .and_hms_opt(12, 0, 0)
        .unwrap()
}

fn utc() -> FixedOffset {
    FixedOffset::east_opt(0).unwrap()
}

/// $1/M input, $10/M output, cache reads unpriced.
fn priced() -> ModelMetadata {
    ModelMetadata {
        cost_default: Some(ModelCost {
            input_micros_per_m: Some(1_000_000),
            output_micros_per_m: Some(10_000_000),
            cache_read_micros_per_m: None,
            cache_write_micros_per_m: None,
        }),
        ..ModelMetadata::default()
    }
}

fn pricing(provider: &str, model: &str) -> RoutePricing {
    match (provider, model) {
        ("ollama", _) => RoutePricing::Local,
        ("xai", "grok-4.6") => RoutePricing::Catalog(Arc::new(priced())),
        _ => RoutePricing::Unknown,
    }
}

struct Row {
    provider: &'static str,
    model: &'static str,
    purpose: &'static str,
    at_ms: i64,
    reported: Option<u64>,
}

fn row(provider: &'static str, model: &'static str, at_ms: i64) -> Row {
    Row {
        provider,
        model,
        purpose: "agent",
        at_ms,
        reported: None,
    }
}

/// 100K input + 10K output tokens per request: $0.20 at [`priced`].
fn write_ledger(directory: &TempDir, rows: &[Row]) -> std::path::PathBuf {
    let path = directory.path().join("usage.sqlite3");
    let recorder = SqliteUsageRecorder::new(&path).unwrap();
    for row in rows {
        let mut event = UsageEvent::new(
            row.provider,
            row.model,
            row.purpose,
            RequestOutcome::Completed,
            ModelUsage {
                input_tokens: Some(100_000),
                output_tokens: Some(10_000),
                cost_usd_micros: row.reported,
                ..ModelUsage::default()
            },
        );
        event.occurred_at_ms = row.at_ms;
        recorder.record(&event).unwrap();
    }
    path
}

fn load(path: &Path) -> SpendReports {
    load_spend_reports(path, &utc(), now(), pricing).unwrap()
}

fn totals(requests: u64, actual: u64, computed: u64, local: u64, unpriced: u64) -> SpendTotals {
    SpendTotals {
        requests,
        tokens: requests * 110_000,
        actual_usd_micros: actual,
        computed_usd_micros: computed,
        local_requests: local,
        unpriced_requests: unpriced,
    }
}

fn group(name: &str, totals: SpendTotals) -> SpendGroup {
    SpendGroup {
        name: name.to_owned(),
        totals,
    }
}

// Covers: each request is priced reported > catalog > sibling-route catalog >
// local > unpriced, and models merge across proxy prefixes while providers
// stay exact routes.
// Owner: usage report
#[test]
fn prices_requests_and_groups_models_across_routes() {
    let directory = tempfile::tempdir().unwrap();
    let path = write_ledger(
        &directory,
        &[
            // Catalog-priced route.
            row("xai", "grok-4.6", NOW_MS),
            // Proxy route borrows the sibling catalog price.
            Row {
                purpose: "subagent",
                ..row("cliproxyapi", "x-ai/grok-4.6", NOW_MS)
            },
            // Provider-reported cost wins over any catalog price.
            Row {
                reported: Some(1_500_000),
                ..row("cursorapi", "grok-4.6", NOW_MS)
            },
            row("ollama", "qwen3.8:27b", NOW_MS),
            row("cliproxyapi", "anthropic/claude-opus-5-5", NOW_MS),
        ],
    );

    let reports = load(&path);
    let report = reports.get(SpendRange::AllTime);

    let first_day = NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();
    assert_eq!(
        report,
        &SpendReport {
            totals: totals(5, 1_500_000, 400_000, 1, 1),
            providers: vec![
                group("cursorapi", totals(1, 1_500_000, 0, 0, 0)),
                group("cliproxyapi", totals(2, 0, 200_000, 0, 1)),
                group("xai", totals(1, 0, 200_000, 0, 0)),
                group("ollama", totals(1, 0, 0, 1, 0)),
            ],
            models: vec![
                group("grok-4.6", totals(3, 1_500_000, 400_000, 0, 0)),
                group("claude-opus-5-5", totals(1, 0, 0, 0, 1)),
                group("qwen3.8:27b", totals(1, 0, 0, 1, 0)),
            ],
            purposes: vec![
                group("agent", totals(4, 1_500_000, 200_000, 1, 1)),
                group("subagent", totals(1, 0, 200_000, 0, 0)),
            ],
            timeline: Timeline {
                unit: TimelineUnit::Day,
                start: first_day.and_hms_opt(0, 0, 0).unwrap(),
                equivalent_usd_micros: vec![1_900_000],
            },
        }
    );
}

// Covers: fixed windows start at local midnight, exclude older and
// future-dated rows, and bucket by day (or by hour for today) with empty days
// kept as zero.
// Owner: usage report
#[test]
fn ranges_filter_by_local_day_and_bucket_the_timeline() {
    let directory = tempfile::tempdir().unwrap();
    let path = write_ledger(
        &directory,
        &[
            row("xai", "grok-4.6", NOW_MS - 40 * DAY_MS),
            row("xai", "grok-4.6", NOW_MS - 3 * DAY_MS),
            row("xai", "grok-4.6", NOW_MS - 2 * HOUR_MS),
            row("xai", "grok-4.6", NOW_MS),
            // Clock skew: dated tomorrow, so it belongs to no range.
            row("xai", "grok-4.6", NOW_MS + DAY_MS),
        ],
    );
    let reports = load(&path);
    let midnight = |days_ago: i64| {
        (now().date() - chrono::Duration::days(days_ago))
            .and_hms_opt(0, 0, 0)
            .unwrap()
    };
    let with_tail = |len: usize, tail: &[u64]| {
        let mut buckets = vec![0; len - tail.len()];
        buckets.extend_from_slice(tail);
        buckets
    };

    let mut today_hours = vec![0; 24];
    today_hours[10] = 200_000;
    today_hours[12] = 200_000;
    let cases = [
        (
            SpendRange::AllTime,
            4,
            Timeline {
                unit: TimelineUnit::Day,
                start: midnight(40),
                equivalent_usd_micros: {
                    let mut buckets = with_tail(41, &[200_000, 0, 0, 400_000]);
                    buckets[0] = 200_000;
                    buckets
                },
            },
        ),
        (
            SpendRange::Last30Days,
            3,
            Timeline {
                unit: TimelineUnit::Day,
                start: midnight(29),
                equivalent_usd_micros: with_tail(30, &[200_000, 0, 0, 400_000]),
            },
        ),
        (
            SpendRange::Last7Days,
            3,
            Timeline {
                unit: TimelineUnit::Day,
                start: midnight(6),
                equivalent_usd_micros: with_tail(7, &[200_000, 0, 0, 400_000]),
            },
        ),
        (
            SpendRange::Today,
            2,
            Timeline {
                unit: TimelineUnit::Hour,
                start: midnight(0),
                equivalent_usd_micros: today_hours,
            },
        ),
    ];
    for (range, requests, timeline) in cases {
        let report = reports.get(range);
        assert_eq!(
            (report.totals.requests, report.timeline.clone()),
            (requests, timeline),
            "{range:?}"
        );
    }
}

// Covers: /spend must never create, migrate, or misread a ledger it does not
// understand; a missing ledger is an empty history.
// Owner: usage report
#[test]
fn load_is_read_only_and_rejects_newer_schemas() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing.sqlite3");
    assert_eq!(load(&missing), SpendReports::empty(now()));
    assert!(!missing.exists());

    let path = write_ledger(&directory, &[row("xai", "grok-4.6", NOW_MS)]);
    rusqlite::Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
        .unwrap();
    let error = load_spend_reports(&path, &utc(), now(), pricing).unwrap_err();
    assert!(matches!(
        error,
        UsageLedgerError::UnsupportedSchema { found, supported }
            if found == SCHEMA_VERSION + 1 && supported == SCHEMA_VERSION
    ));
}

// Covers: built-in Ollama is local, and other routes are priced only from a
// cached catalog row that carries a price.
// Owner: usage report
#[test]
fn catalog_route_pricing_uses_priced_catalog_rows() {
    use rho_providers::model::models_dev::{
        with_models_dev_cache_dir_for_tests, write_cached_model_metadata_for_tests,
    };

    let catalog = tempfile::tempdir().unwrap();
    with_models_dev_cache_dir_for_tests(catalog.path().to_path_buf(), || {
        write_cached_model_metadata_for_tests("anthropic", "claude-opus-5-5", &priced());
        write_cached_model_metadata_for_tests(
            "anthropic",
            "claude-free",
            &ModelMetadata::default(),
        );
        let cases = [
            (
                "anthropic",
                "claude-opus-5-5",
                RoutePricing::Catalog(Arc::new(priced())),
            ),
            ("anthropic", "claude-free", RoutePricing::Unknown),
            ("anthropic", "claude-missing", RoutePricing::Unknown),
            ("ollama", "qwen3.8:27b", RoutePricing::Local),
        ];
        for (provider, model, expected) in cases {
            assert_eq!(
                catalog_route_pricing(provider, model),
                expected,
                "{provider}/{model}"
            );
        }
    });
}
