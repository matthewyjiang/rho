use pretty_assertions::assert_eq;

use super::{load_from, save_to, UsageLimitsCache};
use crate::usage_limits::{UsageLimitWindow, UsageProviderKind};

fn sample_window() -> UsageLimitWindow {
    UsageLimitWindow {
        label: "Weekly".into(),
        remaining_percent: Some(40.0),
        resets_at_unix: Some(1_800_000_000),
        note: None,
    }
}

// Covers: a later success for the same provider replaces windows; other providers stay.
// Owner: pure unit
#[test]
fn cache_upsert_replaces_one_provider() {
    let mut cache = UsageLimitsCache::default();
    cache.upsert(UsageProviderKind::Codex, vec![sample_window()], 1_000);
    cache.upsert(
        UsageProviderKind::Xai,
        vec![UsageLimitWindow {
            label: "Monthly".into(),
            remaining_percent: Some(90.0),
            resets_at_unix: Some(1_900_000_000),
            note: None,
        }],
        1_100,
    );
    cache.upsert(
        UsageProviderKind::Codex,
        vec![UsageLimitWindow {
            label: "Weekly".into(),
            remaining_percent: Some(10.0),
            resets_at_unix: Some(1_850_000_000),
            note: None,
        }],
        1_200,
    );

    let codex = cache.get(UsageProviderKind::Codex).expect("codex");
    assert_eq!(codex.fetched_at_unix, 1_200);
    assert_eq!(codex.windows[0].remaining_percent, Some(10.0));
    let xai = cache.get(UsageProviderKind::Xai).expect("xai");
    assert_eq!(xai.fetched_at_unix, 1_100);
    assert_eq!(xai.windows[0].remaining_percent, Some(90.0));
    assert!(cache.get(UsageProviderKind::KimiCode).is_none());
}

// Covers: disk round-trip keeps windows; unknown versions must not look like usage.
// Owner: OS
#[test]
fn cache_roundtrip_and_unknown_version_are_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("oauth-usage-limits.json");
    let mut cache = UsageLimitsCache::default();
    cache.upsert(UsageProviderKind::KimiCode, vec![sample_window()], 50);
    save_to(&path, &cache).expect("save");
    let loaded = load_from(&path).expect("load");
    assert_eq!(loaded, cache);

    let mut unknown = cache.clone();
    unknown.version = 99;
    save_to(&path, &unknown).expect("save unknown");
    let skipped = load_from(&path).expect("load unknown");
    assert_eq!(skipped, UsageLimitsCache::default());
}
