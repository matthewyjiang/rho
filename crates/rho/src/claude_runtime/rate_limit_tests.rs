use super::*;
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering as AtomicOrdering},
        Arc,
    },
    thread,
    time::Duration,
};

fn sample_info(status: &str) -> RateLimitInfo {
    RateLimitInfo {
        status: Some(status.into()),
        rate_limit_type: Some("five_hour".into()),
        resets_at: Some(1_800),
        utilization: None,
        overage_status: None,
        overage_resets_at: None,
        is_using_overage: Some(false),
    }
}

fn secs(seconds: i64) -> u64 {
    seconds_to_nanos(seconds)
}

fn only(state: &RateLimitState) -> &RateLimitObservation {
    assert_eq!(state.windows.len(), 1, "expected one window: {state:?}");
    &state.windows[0]
}

fn sample_info_window(status: &str, window: &str) -> RateLimitInfo {
    let mut info = sample_info(status);
    info.rate_limit_type = Some(window.into());
    info
}

fn write_state_unlocked(path: &Path, observation: &RateLimitObservation) {
    let observed = RateLimitObservation {
        observed_at_unix: observation.observed_at_unix,
        observed_seq: observation.observed_seq,
        observed_at_nanos: observation.observed_at_nanos,
        observed_nonce: observation.observed_nonce.clone(),
        info: observation.info.clone(),
    };
    let contents = serde_json::to_vec_pretty(&observed).unwrap();
    crate::config_writer::write_bytes_atomically(path, &contents).unwrap();
}

#[test]
fn describe_omits_allowed_and_percent_when_missing() {
    let observed = RateLimitObservation {
        observed_at_unix: 1_000,
        observed_seq: 1,
        observed_at_nanos: secs(1_000),
        observed_nonce: "a".into(),
        info: sample_info("allowed"),
    };
    let text = observed.describe(1_000 + 120);
    assert!(text.contains("claude code:"));
    assert!(text.contains("Five hour"));
    assert!(!text.contains("allowed"), "{text}");
    assert!(text.contains("observed 2m ago"));
    assert!(!text.contains('%'));
}

#[test]
fn describe_includes_remaining_percent_from_utilization() {
    let mut info = sample_info("allowed");
    info.utilization = Some(0.31);
    let observed = RateLimitObservation::with_order(info, secs(1_000), 1, "a");
    let text = observed.describe(1_000);
    assert!(text.contains("69% left"), "{text}");
    assert!(!text.contains("allowed"), "{text}");
}

#[test]
fn multi_window_state_keeps_five_hour_and_seven_day() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rate-limits.json");
    store_ordered(
        &path,
        sample_info_window("allowed", "five_hour"),
        secs(1_000),
        1,
        "p1",
    )
    .unwrap();
    store_ordered(
        &path,
        sample_info_window("allowed", "seven_day"),
        secs(1_100),
        1,
        "p1",
    )
    .unwrap();
    // Older five_hour must not clobber the newer five_hour or remove weekly.
    store_ordered(
        &path,
        sample_info_window("stale", "five_hour"),
        secs(900),
        1,
        "p2",
    )
    .unwrap();
    let loaded = load_at(&path).expect("stored");
    assert_eq!(loaded.windows.len(), 2);
    let keys: Vec<_> = loaded
        .sorted_windows()
        .into_iter()
        .map(|window| window.info.window_key().to_owned())
        .collect();
    assert_eq!(keys, vec!["five_hour".to_string(), "seven_day".to_string()]);
    let five = loaded
        .windows
        .iter()
        .find(|window| window.info.window_key() == "five_hour")
        .unwrap();
    assert_eq!(five.info.status.as_deref(), Some("allowed"));
    assert_eq!(five.observed_at_unix, 1_000);
}

#[test]
fn loads_legacy_single_observation_file_as_one_window() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("legacy.json");
    std::fs::write(
        &path,
        r#"{"observed_at_unix":1000,"info":{"status":"allowed","rateLimitType":"five_hour"}}"#,
    )
    .unwrap();
    let loaded = load_at(&path).expect("legacy");
    assert_eq!(loaded.windows.len(), 1);
    assert_eq!(only(&loaded).info.window_key(), "five_hour");
}

#[test]
fn default_state_path_lives_under_cache_claude_code() {
    let path = default_state_path().unwrap();
    let components = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>();
    assert!(
        components
            .windows(3)
            .any(|window| { window == ["cache", "claude-code", "rate-limits.json"] }),
        "{path:?}"
    );
}

#[test]
fn older_observation_does_not_overwrite_newer() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("claude-rate-limit.json");
    store_ordered(&path, sample_info("newer"), secs(2_000), 1, "p1").unwrap();
    store_ordered(&path, sample_info("older"), secs(1_000), 2, "p2").unwrap();
    let loaded = load_at(&path).expect("stored");
    assert_eq!(only(&loaded).observed_at_unix, 2_000);
    assert_eq!(only(&loaded).info.status.as_deref(), Some("newer"));
}

#[test]
fn newer_observation_replaces_older() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("claude-rate-limit.json");
    store_ordered(&path, sample_info("older"), secs(1_000), 1, "p1").unwrap();
    store_ordered(&path, sample_info("newer"), secs(2_000), 1, "p1").unwrap();
    let loaded = load_at(&path).expect("stored");
    assert_eq!(only(&loaded).observed_at_unix, 2_000);
    assert_eq!(only(&loaded).info.status.as_deref(), Some("newer"));
}

#[test]
fn equal_timestamps_prefer_higher_sequence_then_nonce() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("claude-rate-limit.json");
    // Same nanos + nonce: higher seq wins.
    store_ordered(&path, sample_info("first"), secs(1_000), 1, "n").unwrap();
    store_ordered(&path, sample_info("second"), secs(1_000), 2, "n").unwrap();
    store_ordered(&path, sample_info("stale"), secs(1_000), 1, "n").unwrap();
    let loaded = load_at(&path).expect("stored");
    assert_eq!(only(&loaded).observed_at_unix, 1_000);
    assert_eq!(only(&loaded).observed_seq, 2);
    assert_eq!(only(&loaded).info.status.as_deref(), Some("second"));

    // Same nanos, different nonce: lexicographically greater nonce wins.
    store_ordered(&path, sample_info("nonce-low"), secs(1_000), 9, "aaa").unwrap();
    store_ordered(&path, sample_info("nonce-high"), secs(1_000), 1, "zzz").unwrap();
    let loaded = load_at(&path).expect("stored");
    assert_eq!(only(&loaded).observed_nonce, "zzz");
    assert_eq!(only(&loaded).info.status.as_deref(), Some("nonce-high"));
}

#[test]
fn concurrent_writers_keep_latest_observation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("claude-rate-limit.json");
    let path_a = path.clone();
    let path_b = path.clone();
    let left = thread::spawn(move || {
        for stamp in (0..40).step_by(2) {
            store_ordered(
                &path_a,
                sample_info(&format!("a{stamp}")),
                secs(stamp),
                stamp as u64,
                "a",
            )
            .unwrap();
        }
    });
    let right = thread::spawn(move || {
        for stamp in (1..40).step_by(2) {
            store_ordered(
                &path_b,
                sample_info(&format!("b{stamp}")),
                secs(stamp),
                stamp as u64,
                "b",
            )
            .unwrap();
        }
    });
    left.join().unwrap();
    right.join().unwrap();
    let loaded = load_at(&path).expect("stored");
    assert_eq!(only(&loaded).observed_at_unix, 39);
    assert_eq!(only(&loaded).info.status.as_deref(), Some("b39"));
}

#[test]
fn out_of_order_writers_keep_highest_order_key() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("claude-rate-limit.json");
    let path_late = path.clone();
    let path_early = path.clone();
    let first = thread::spawn(move || {
        thread::sleep(Duration::from_millis(30));
        store_ordered(&path_early, sample_info("slow-old"), secs(1_000), 1, "a").unwrap();
    });
    let second = thread::spawn(move || {
        store_ordered(&path_late, sample_info("fast-new"), secs(1_000), 5, "a").unwrap();
    });
    first.join().unwrap();
    second.join().unwrap();
    let loaded = load_at(&path).expect("stored");
    assert_eq!(only(&loaded).observed_seq, 5);
    assert_eq!(only(&loaded).info.status.as_deref(), Some("fast-new"));
}

#[test]
fn slot_keeps_latest_under_out_of_order_publish() {
    let slot = RateLimitSlot::new();
    slot.publish(RateLimitObservation::with_order(
        sample_info("mid"),
        secs(10),
        2,
        "n",
    ));
    slot.publish(RateLimitObservation::with_order(
        sample_info("old"),
        secs(10),
        1,
        "n",
    ));
    slot.publish(RateLimitObservation::with_order(
        sample_info("new"),
        secs(11),
        1,
        "n",
    ));
    let taken = slot.take().expect("observation");
    assert_eq!(only(&taken).info.status.as_deref(), Some("new"));
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
            secs(stamp),
            stamp as u64,
            "n",
        )
        .unwrap();
        assert_eq!(only(&load_at(&path).unwrap()).observed_at_unix, stamp);
    }
}

#[test]
fn legacy_files_without_nanos_or_nonce_deserialize_and_lose_to_newer() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("claude-rate-limit.json");
    // Pure seconds+info cache (pre-seq).
    std::fs::write(
        &path,
        r#"{"observed_at_unix":1000,"info":{"status":"allowed","rateLimitType":"five_hour"}}"#,
    )
    .unwrap();
    let loaded = load_at(&path).expect("legacy");
    assert_eq!(only(&loaded).observed_seq, 0);
    assert_eq!(only(&loaded).observed_at_nanos, 0);
    assert!(only(&loaded).observed_nonce.is_empty());
    store_ordered(&path, sample_info("replacement"), secs(1_000), 1, "p").unwrap();
    assert_eq!(
        only(&load_at(&path).unwrap()).info.status.as_deref(),
        Some("replacement")
    );

    // Seconds+seq cache (previous process-local scheme).
    std::fs::write(
        &path,
        r#"{"observed_at_unix":2000,"observed_seq":7,"info":{"status":"legacy-seq","rateLimitType":"five_hour"}}"#,
    )
    .unwrap();
    let loaded = load_at(&path).expect("legacy-seq");
    assert_eq!(only(&loaded).observed_seq, 7);
    assert_eq!(only(&loaded).order_key().nanos, secs(2_000));
    // Older second must not replace legacy.
    store_ordered(&path, sample_info("too-old"), secs(1_999), 99, "z").unwrap();
    assert_eq!(
        only(&load_at(&path).unwrap()).info.status.as_deref(),
        Some("legacy-seq")
    );
    // Subsecond-newer observation replaces the legacy seconds+seq cache.
    store_ordered(&path, sample_info("fresh"), secs(2_000) + 1, 0, "a").unwrap();
    assert_eq!(
        only(&load_at(&path).unwrap()).info.status.as_deref(),
        Some("fresh")
    );
}

#[test]
fn independent_writer_instances_newer_wins_regardless_of_write_order() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("claude-rate-limit.json");

    // Simulate two Rho processes: each stamps with its own nonce and order.
    let older = RateLimitObservation::with_order(sample_info("older"), secs(5_000), 1, "proc-a");
    let newer =
        RateLimitObservation::with_order(sample_info("newer"), secs(5_000) + 50, 1, "proc-b");

    // Write newer first, then older (cross-process out-of-order arrival).
    store_observation(&path, newer.clone()).unwrap();
    store_observation(&path, older).unwrap();
    let loaded = load_at(&path).expect("stored");
    assert_eq!(only(&loaded).info.status.as_deref(), Some("newer"));
    assert_eq!(only(&loaded).observed_at_nanos, newer.observed_at_nanos);
    assert_eq!(only(&loaded).observed_nonce, "proc-b");
}

#[test]
fn same_timestamp_tie_break_is_deterministic_across_writers() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("claude-rate-limit.json");
    let nanos = secs(9_000) + 123_456_789;
    // Nonce orders lexicographically: "a-uuid" < "z-uuid".
    store_ordered(&path, sample_info("low"), nanos, 1, "a-uuid").unwrap();
    store_ordered(&path, sample_info("high"), nanos, 1, "z-uuid").unwrap();
    store_ordered(&path, sample_info("low-again"), nanos, 99, "a-uuid").unwrap();
    let loaded = load_at(&path).expect("stored");
    assert_eq!(only(&loaded).observed_nonce, "z-uuid");
    assert_eq!(only(&loaded).info.status.as_deref(), Some("high"));
}

#[test]
fn lock_serializes_overlapping_writers() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("claude-rate-limit.json");
    let ready = Arc::new(AtomicBool::new(false));
    let path_a = path.clone();
    let path_b = path.clone();
    let ready_a = Arc::clone(&ready);
    let ready_b = Arc::clone(&ready);
    let a = thread::spawn(move || {
        while !ready_a.load(AtomicOrdering::Acquire) {
            thread::yield_now();
        }
        for i in 0..20u64 {
            store_ordered(
                &path_a,
                sample_info(&format!("a{i}")),
                secs(100) + i,
                i,
                "a",
            )
            .unwrap();
        }
    });
    let b = thread::spawn(move || {
        while !ready_b.load(AtomicOrdering::Acquire) {
            thread::yield_now();
        }
        for i in 0..20u64 {
            store_ordered(
                &path_b,
                sample_info(&format!("b{i}")),
                secs(100) + i + 1,
                i,
                "b",
            )
            .unwrap();
        }
    });
    ready.store(true, AtomicOrdering::Release);
    a.join().unwrap();
    b.join().unwrap();
    let loaded = load_at(&path).expect("stored");
    // Highest nanos is secs(100)+20 from b's last write (i=19 -> 100+20).
    assert_eq!(only(&loaded).observed_at_nanos, secs(100) + 20);
    assert_eq!(only(&loaded).info.status.as_deref(), Some("b19"));
}

#[test]
fn cross_process_lock_blocks_until_released_and_newer_wins() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("claude-rate-limit.json");
    let lock_path = lock_path_for(&path);

    // Hold the exclusive lock from this process. Write the older state without
    // going through store_observation so we do not re-enter the same lock.
    let lock_file = open_lock_file(&lock_path).expect("open lock");
    let lock_guard = rho_providers::file_lock::FileLock::acquire(lock_file).expect("hold lock");
    write_state_unlocked(
        &path,
        &RateLimitObservation::with_order(sample_info("parent-old"), secs(1), 1, "parent"),
    );

    let exe = std::env::current_exe().expect("current exe");
    let mut child = Command::new(&exe)
        .args([
            "--exact",
            "claude_runtime::rate_limit::tests::child_store_observation_helper",
            "--nocapture",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn child");

    {
        let stdin = child.stdin.as_mut().expect("child stdin");
        // Protocol: one line path, then nanos, seq, nonce, status.
        writeln!(stdin, "{}", path.display()).unwrap();
        writeln!(stdin, "{}", secs(9_999)).unwrap();
        writeln!(stdin, "1").unwrap();
        writeln!(stdin, "child").unwrap();
        writeln!(stdin, "child-new").unwrap();
        // Keep stdin open until we drop it after unlock so the child can block
        // on the lock with the full request already read... actually the child
        // reads all lines first then locks. Close stdin so read completes.
    }
    drop(child.stdin.take());

    // Child should be blocked on the lock (or still starting). Give it a moment
    // then confirm it has not exited successfully with a write yet while locked.
    thread::sleep(Duration::from_millis(100));
    assert!(
        child.try_wait().ok().flatten().is_none(),
        "child should still be waiting on the lock"
    );
    assert_eq!(
        only(&load_at(&path).unwrap()).info.status.as_deref(),
        Some("parent-old")
    );

    // Release the lock; child compare-and-replace should proceed and win.
    drop(lock_guard);
    let output = child.wait_with_output().expect("wait child");
    assert!(
        output.status.success(),
        "child failed: status={} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let loaded = load_at(&path).expect("stored");
    assert_eq!(only(&loaded).info.status.as_deref(), Some("child-new"));
    assert_eq!(only(&loaded).observed_nonce, "child");
    assert_eq!(only(&loaded).observed_at_nanos, secs(9_999));
}

/// Child helper for cross-process lock tests. Reads path + observation from stdin.
///
/// When stdin is empty (normal `cargo test` collection/run without a parent),
/// the helper exits successfully without touching the filesystem.
#[test]
fn child_store_observation_helper() {
    let mut stdin = std::io::stdin().lock();
    let mut buf = String::new();
    // Non-blocking-ish: if nothing is piped, read_to_string returns quickly
    // with empty content on most platforms when stdin is /dev/null.
    if stdin.read_to_string(&mut buf).is_err() || buf.trim().is_empty() {
        return;
    }
    let mut lines = buf.lines();
    let path = PathBuf::from(lines.next().expect("path").trim());
    let nanos: u64 = lines.next().expect("nanos").trim().parse().expect("nanos");
    let seq: u64 = lines.next().expect("seq").trim().parse().expect("seq");
    let nonce = lines.next().expect("nonce").trim().to_owned();
    let status = lines.next().expect("status").trim().to_owned();
    store_observation(
        &path,
        RateLimitObservation::with_order(sample_info(&status), nanos, seq, nonce),
    )
    .expect("child store");
}

#[test]
fn capture_stamps_subsecond_nanos_and_process_nonce() {
    let first = RateLimitObservation::capture(sample_info("a"));
    let second = RateLimitObservation::capture(sample_info("b"));
    assert!(first.observed_at_nanos > 0);
    assert!(!first.observed_nonce.is_empty());
    assert_eq!(first.observed_nonce, second.observed_nonce);
    assert!(
        second.observed_seq > first.observed_seq
            || second.observed_at_nanos >= first.observed_at_nanos
    );
    assert_eq!(
        first.observed_at_unix,
        nanos_to_seconds(first.observed_at_nanos)
    );
}

#[test]
fn cross_process_out_of_order_writes_keep_newer() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("claude-rate-limit.json");
    let exe = std::env::current_exe().expect("current exe");

    // Child A writes a newer observation first.
    let mut child_new = Command::new(&exe)
        .args([
            "--exact",
            "claude_runtime::rate_limit::tests::child_store_observation_helper",
            "--nocapture",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn newer child");
    {
        let stdin = child_new.stdin.as_mut().unwrap();
        writeln!(stdin, "{}", path.display()).unwrap();
        writeln!(stdin, "{}", secs(7_000) + 500).unwrap();
        writeln!(stdin, "1").unwrap();
        writeln!(stdin, "proc-new").unwrap();
        writeln!(stdin, "from-new-proc").unwrap();
    }
    drop(child_new.stdin.take());
    let newer_status = child_new.wait_with_output().unwrap();
    assert!(
        newer_status.status.success(),
        "newer child failed: {}",
        String::from_utf8_lossy(&newer_status.stderr)
    );

    // Child B then writes an older observation; it must not win.
    let mut child_old = Command::new(&exe)
        .args([
            "--exact",
            "claude_runtime::rate_limit::tests::child_store_observation_helper",
            "--nocapture",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn older child");
    {
        let stdin = child_old.stdin.as_mut().unwrap();
        writeln!(stdin, "{}", path.display()).unwrap();
        writeln!(stdin, "{}", secs(7_000)).unwrap();
        writeln!(stdin, "99").unwrap();
        writeln!(stdin, "proc-old").unwrap();
        writeln!(stdin, "from-old-proc").unwrap();
    }
    drop(child_old.stdin.take());
    let older_status = child_old.wait_with_output().unwrap();
    assert!(
        older_status.status.success(),
        "older child failed: {}",
        String::from_utf8_lossy(&older_status.stderr)
    );

    let loaded = load_at(&path).expect("stored");
    assert_eq!(only(&loaded).info.status.as_deref(), Some("from-new-proc"));
    assert_eq!(only(&loaded).observed_nonce, "proc-new");
    assert_eq!(only(&loaded).observed_at_nanos, secs(7_000) + 500);
}
