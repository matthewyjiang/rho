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
        observed_seq: 1,
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
    store_ordered(&path, sample_info("newer"), 2_000, 1).unwrap();
    store_ordered(&path, sample_info("older"), 1_000, 2).unwrap();
    let loaded = load_at(&path).expect("stored");
    assert_eq!(loaded.observed_at_unix, 2_000);
    assert_eq!(loaded.info.status.as_deref(), Some("newer"));
}

#[test]
fn newer_observation_replaces_older() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("claude-rate-limit.json");
    store_ordered(&path, sample_info("older"), 1_000, 1).unwrap();
    store_ordered(&path, sample_info("newer"), 2_000, 1).unwrap();
    let loaded = load_at(&path).expect("stored");
    assert_eq!(loaded.observed_at_unix, 2_000);
    assert_eq!(loaded.info.status.as_deref(), Some("newer"));
}

#[test]
fn equal_timestamps_prefer_higher_sequence() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("claude-rate-limit.json");
    store_ordered(&path, sample_info("first"), 1_000, 1).unwrap();
    store_ordered(&path, sample_info("second"), 1_000, 2).unwrap();
    store_ordered(&path, sample_info("stale"), 1_000, 1).unwrap();
    let loaded = load_at(&path).expect("stored");
    assert_eq!(loaded.observed_at_unix, 1_000);
    assert_eq!(loaded.observed_seq, 2);
    assert_eq!(loaded.info.status.as_deref(), Some("second"));
}

#[test]
fn concurrent_writers_keep_latest_observation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("claude-rate-limit.json");
    let path_a = path.clone();
    let path_b = path.clone();
    let left = std::thread::spawn(move || {
        for stamp in (0..40).step_by(2) {
            store_ordered(
                &path_a,
                sample_info(&format!("a{stamp}")),
                stamp,
                stamp as u64,
            )
            .unwrap();
        }
    });
    let right = std::thread::spawn(move || {
        for stamp in (1..40).step_by(2) {
            store_ordered(
                &path_b,
                sample_info(&format!("b{stamp}")),
                stamp,
                stamp as u64,
            )
            .unwrap();
        }
    });
    left.join().unwrap();
    right.join().unwrap();
    let loaded = load_at(&path).expect("stored");
    assert_eq!(loaded.observed_at_unix, 39);
    assert_eq!(loaded.info.status.as_deref(), Some("b39"));
}

#[test]
fn out_of_order_writers_keep_highest_order_key() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("claude-rate-limit.json");
    let path_late = path.clone();
    let path_early = path.clone();
    let first = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(30));
        store_ordered(&path_early, sample_info("slow-old"), 1_000, 1).unwrap();
    });
    let second = std::thread::spawn(move || {
        store_ordered(&path_late, sample_info("fast-new"), 1_000, 5).unwrap();
    });
    first.join().unwrap();
    second.join().unwrap();
    let loaded = load_at(&path).expect("stored");
    assert_eq!(loaded.observed_seq, 5);
    assert_eq!(loaded.info.status.as_deref(), Some("fast-new"));
}

#[test]
fn slot_keeps_latest_under_out_of_order_publish() {
    let slot = RateLimitSlot::new();
    slot.publish(RateLimitObservation::with_order(sample_info("mid"), 10, 2));
    slot.publish(RateLimitObservation::with_order(sample_info("old"), 10, 1));
    slot.publish(RateLimitObservation::with_order(sample_info("new"), 11, 1));
    let taken = slot.take().expect("observation");
    assert_eq!(taken.info.status.as_deref(), Some("new"));
    assert!(slot.take().is_none());
}

#[test]
fn repeated_updates_use_unique_temp_and_replace() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("claude-rate-limit.json");
    for stamp in 0..5 {
        store_ordered(
            &path,
            sample_info(&format!("n{stamp}")),
            stamp,
            stamp as u64,
        )
        .unwrap();
        assert_eq!(load_at(&path).unwrap().observed_at_unix, stamp);
    }
}

#[test]
fn legacy_files_without_seq_deserialize_as_zero() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("claude-rate-limit.json");
    std::fs::write(
        &path,
        r#"{"observed_at_unix":1000,"info":{"status":"allowed","rateLimitType":"five_hour"}}"#,
    )
    .unwrap();
    let loaded = load_at(&path).expect("legacy");
    assert_eq!(loaded.observed_seq, 0);
    store_ordered(&path, sample_info("replacement"), 1_000, 1).unwrap();
    assert_eq!(
        load_at(&path).unwrap().info.status.as_deref(),
        Some("replacement")
    );
}
