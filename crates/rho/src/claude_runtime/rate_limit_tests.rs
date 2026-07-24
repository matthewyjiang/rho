use super::*;

fn sample_info(status: &str) -> RateLimitInfo {
    RateLimitInfo {
        status: Some(status.into()),
        rate_limit_type: Some("five_hour".into()),
        resets_at: Some(1_800),
        overage_status: None,
        overage_resets_at: None,
        is_using_overage: Some(false),
    }
}

#[test]
fn describe_includes_window_status_and_age_without_percent() {
    let observed = ObservedRateLimit {
        observed_at_unix: 1_000,
        info: sample_info("allowed"),
    };
    let text = observed.describe(1_000 + 120);
    assert!(text.contains("claude code:"));
    assert!(text.contains("five hour"));
    assert!(text.contains("allowed"));
    assert!(text.contains("last observed 2m ago"));
    assert!(!text.contains('%'));
}

#[test]
fn older_observation_does_not_overwrite_newer() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("claude-rate-limit.json");
    store_at(&path, sample_info("newer"), 2_000).unwrap();
    store_at(&path, sample_info("older"), 1_000).unwrap();
    let loaded = load_at(&path).expect("stored");
    assert_eq!(loaded.observed_at_unix, 2_000);
    assert_eq!(loaded.info.status.as_deref(), Some("newer"));
}

#[test]
fn newer_observation_replaces_older() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("claude-rate-limit.json");
    store_at(&path, sample_info("older"), 1_000).unwrap();
    store_at(&path, sample_info("newer"), 2_000).unwrap();
    let loaded = load_at(&path).expect("stored");
    assert_eq!(loaded.observed_at_unix, 2_000);
    assert_eq!(loaded.info.status.as_deref(), Some("newer"));
}

#[test]
fn concurrent_writers_keep_latest_observation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("claude-rate-limit.json");
    let path_a = path.clone();
    let path_b = path.clone();
    let left = std::thread::spawn(move || {
        for stamp in (0..40).step_by(2) {
            store_at(&path_a, sample_info(&format!("a{stamp}")), stamp).unwrap();
        }
    });
    let right = std::thread::spawn(move || {
        for stamp in (1..40).step_by(2) {
            store_at(&path_b, sample_info(&format!("b{stamp}")), stamp).unwrap();
        }
    });
    left.join().unwrap();
    right.join().unwrap();
    let loaded = load_at(&path).expect("stored");
    assert_eq!(loaded.observed_at_unix, 39);
    assert_eq!(loaded.info.status.as_deref(), Some("b39"));
}

#[test]
fn repeated_updates_use_unique_temp_and_replace() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("claude-rate-limit.json");
    for stamp in 0..5 {
        store_at(&path, sample_info(&format!("n{stamp}")), stamp).unwrap();
        assert_eq!(load_at(&path).unwrap().observed_at_unix, stamp);
    }
}
