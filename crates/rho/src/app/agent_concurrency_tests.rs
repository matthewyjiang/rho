use std::sync::Arc;

use super::*;
use crate::config::{DEFAULT_AGENT_CONCURRENCY, MAX_AGENT_CONCURRENCY};
use rho_tools::cancellation::RunCancellation;

/// Deterministic scheduling probe: wait until `ready` is true, yielding so the
/// runtime can progress other tasks without wall-clock sleeps.
async fn wait_until(mut ready: impl FnMut() -> bool) {
    for _ in 0..10_000 {
        if ready() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("condition not met after cooperative yields");
}

fn limits(total: usize, claude_requested: usize) -> ConcurrencyLimits {
    ConcurrencyLimits {
        total,
        claude_requested,
    }
}

// Covers: invalid env values keep the configured total and default Claude cap.
// Owner: delegated agent concurrency
#[test]
fn zero_invalid_and_huge_concurrency_values_fall_back() {
    let expected = ConcurrencyLimits {
        total: DEFAULT_AGENT_CONCURRENCY,
        claude_requested: 2,
    };
    assert_eq!(
        concurrency_limits_from_env(Some("0"), Some("0"), DEFAULT_AGENT_CONCURRENCY),
        expected
    );
    assert_eq!(
        concurrency_limits_from_env(Some("-1"), Some("nope"), DEFAULT_AGENT_CONCURRENCY),
        expected
    );
    assert_eq!(
        concurrency_limits_from_env(Some(""), Some(" "), DEFAULT_AGENT_CONCURRENCY),
        expected
    );
    let huge = format!("{}0", usize::MAX);
    assert_eq!(
        concurrency_limits_from_env(
            Some(huge.as_str()),
            Some(huge.as_str()),
            DEFAULT_AGENT_CONCURRENCY
        ),
        expected
    );
}

// Covers: env overrides clamp to the named max instead of opening unbounded fan-out.
// Owner: delegated agent concurrency
#[test]
fn total_and_claude_env_values_interact() {
    assert_eq!(
        concurrency_limits_from_env(Some("bad"), Some("3"), DEFAULT_AGENT_CONCURRENCY),
        ConcurrencyLimits {
            total: DEFAULT_AGENT_CONCURRENCY,
            claude_requested: 3
        }
    );
    assert_eq!(
        concurrency_limits_from_env(Some("1"), Some("bad"), DEFAULT_AGENT_CONCURRENCY),
        ConcurrencyLimits {
            total: 1,
            claude_requested: 2
        }
    );
    assert_eq!(
        concurrency_limits_from_env(Some("8"), Some("5"), DEFAULT_AGENT_CONCURRENCY),
        ConcurrencyLimits {
            total: 8,
            claude_requested: 5
        }
    );
    assert_eq!(
        concurrency_limits_from_env(Some("100"), Some("90"), DEFAULT_AGENT_CONCURRENCY),
        ConcurrencyLimits {
            total: MAX_AGENT_CONCURRENCY,
            claude_requested: MAX_AGENT_CONCURRENCY
        }
    );
}

#[tokio::test(flavor = "current_thread")]
async fn claude_queue_does_not_starve_rho_and_progresses_after_release() {
    let pool = AgentConcurrency::new(limits(2, 1));

    let active_claude = pool
        .acquire(CapacityClass::Claude, &RunCancellation::new())
        .await
        .expect("active Claude should acquire");
    assert_eq!(pool.available_total(), 1);
    assert_eq!(pool.available_claude(), 0);

    let queued_started = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let queued_claude = tokio::spawn({
        let pool = pool.clone();
        let queued_started = Arc::clone(&queued_started);
        async move {
            queued_started.store(true, std::sync::atomic::Ordering::SeqCst);
            pool.acquire(CapacityClass::Claude, &RunCancellation::new())
                .await
        }
    });

    wait_until(|| queued_started.load(std::sync::atomic::Ordering::SeqCst)).await;
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
    assert!(
        !queued_claude.is_finished(),
        "queued Claude must still wait on Claude capacity"
    );
    assert_eq!(pool.available_total(), 1);
    assert_eq!(pool.available_claude(), 0);

    let rho = pool
        .acquire(CapacityClass::Rho, &RunCancellation::new())
        .await
        .expect("Rho should take the spare global permit");
    assert_eq!(pool.available_total(), 0);
    assert_eq!(pool.available_claude(), 0);
    assert!(
        !queued_claude.is_finished(),
        "queued Claude must not finish while Claude capacity is held"
    );

    drop(active_claude);
    let queued = tokio::time::timeout(std::time::Duration::from_secs(1), queued_claude)
        .await
        .expect("queued Claude should acquire after active Claude releases")
        .unwrap()
        .expect("queued Claude should not cancel");
    assert_eq!(pool.available_total(), 0);
    assert_eq!(pool.available_claude(), 0);

    drop(rho);
    drop(queued);
    assert_eq!(pool.available_total(), 2);
    assert_eq!(pool.available_claude(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn claude_waits_on_global_after_taking_claude_capacity() {
    let pool = AgentConcurrency::new(limits(2, 1));

    let rho_a = pool
        .acquire(CapacityClass::Rho, &RunCancellation::new())
        .await
        .expect("rho a");
    let rho_b = pool
        .acquire(CapacityClass::Rho, &RunCancellation::new())
        .await
        .expect("rho b");
    assert_eq!(pool.available_total(), 0);
    assert_eq!(pool.available_claude(), 1);

    let queued = tokio::spawn({
        let pool = pool.clone();
        async move {
            pool.acquire(CapacityClass::Claude, &RunCancellation::new())
                .await
        }
    });

    wait_until(|| pool.available_claude() == 0).await;
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    assert!(!queued.is_finished());
    assert_eq!(pool.available_total(), 0);
    assert_eq!(pool.available_claude(), 0);

    drop(rho_a);
    let permits = tokio::time::timeout(std::time::Duration::from_secs(1), queued)
        .await
        .expect("Claude should finish once a global slot frees")
        .unwrap()
        .expect("Claude acquired");
    assert_eq!(pool.available_total(), 0);
    assert_eq!(pool.available_claude(), 0);
    drop(rho_b);
    drop(permits);
    assert_eq!(pool.available_total(), 2);
    assert_eq!(pool.available_claude(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_during_claude_wait_releases_nothing() {
    let pool = AgentConcurrency::new(limits(2, 1));
    let _held = pool
        .acquire(CapacityClass::Claude, &RunCancellation::new())
        .await
        .expect("held Claude occupies nested capacity");
    let cancellation = RunCancellation::new();

    let queued = tokio::spawn({
        let pool = pool.clone();
        let cancellation = cancellation.clone();
        async move { pool.acquire(CapacityClass::Claude, &cancellation).await }
    });

    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
    cancellation.cancel();

    let result = tokio::time::timeout(std::time::Duration::from_secs(1), queued)
        .await
        .expect("cancel during Claude wait")
        .unwrap();
    assert!(result.is_none());
    assert_eq!(pool.available_total(), 1);
    assert_eq!(pool.available_claude(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_during_global_wait_releases_claude_permit() {
    let pool = AgentConcurrency::new(limits(1, 1));
    let _rho = pool
        .acquire(CapacityClass::Rho, &RunCancellation::new())
        .await
        .expect("rho fills the only global slot");
    let cancellation = RunCancellation::new();

    let queued = tokio::spawn({
        let pool = pool.clone();
        let cancellation = cancellation.clone();
        async move { pool.acquire(CapacityClass::Claude, &cancellation).await }
    });

    wait_until(|| pool.available_claude() == 0).await;
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    assert_eq!(pool.available_total(), 0);
    assert_eq!(pool.available_claude(), 0);

    cancellation.cancel();

    let result = tokio::time::timeout(std::time::Duration::from_secs(1), queued)
        .await
        .expect("cancel during global wait")
        .unwrap();
    assert!(result.is_none());
    assert_eq!(pool.available_total(), 0);
    assert_eq!(pool.available_claude(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_during_rho_global_wait_holds_nothing() {
    let pool = AgentConcurrency::new(limits(1, 1));
    let _held = pool
        .acquire(CapacityClass::Rho, &RunCancellation::new())
        .await
        .expect("held rho");
    let cancellation = RunCancellation::new();

    let queued = tokio::spawn({
        let pool = pool.clone();
        let cancellation = cancellation.clone();
        async move { pool.acquire(CapacityClass::Rho, &cancellation).await }
    });

    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
    cancellation.cancel();

    let result = tokio::time::timeout(std::time::Duration::from_secs(1), queued)
        .await
        .expect("cancel during Rho global wait")
        .unwrap();
    assert!(result.is_none());
    assert_eq!(pool.available_total(), 0);
    assert_eq!(pool.available_claude(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn rho_skips_claude_pool_entirely() {
    let pool = AgentConcurrency::new(limits(1, 0));

    let rho = pool
        .acquire(CapacityClass::Rho, &RunCancellation::new())
        .await
        .expect("Rho ignores Claude pool");
    assert_eq!(pool.available_total(), 0);
    assert_eq!(pool.available_claude(), 0);
    drop(rho);
    assert_eq!(pool.available_total(), 1);
    assert_eq!(pool.available_claude(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_interrupts_concurrency_queue() {
    let pool = AgentConcurrency::new(limits(0, 1));
    let cancellation = RunCancellation::new();
    let queued = tokio::spawn({
        let pool = pool.clone();
        let cancellation = cancellation.clone();
        async move { pool.acquire(CapacityClass::Rho, &cancellation).await }
    });

    cancellation.cancel();

    let permit = tokio::time::timeout(std::time::Duration::from_secs(1), queued)
        .await
        .expect("queued acquisition should observe cancellation")
        .unwrap();
    assert!(permit.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_wins_when_a_permit_is_already_available() {
    let pool = AgentConcurrency::new(limits(1, 1));
    let cancellation = RunCancellation::new();
    cancellation.cancel();

    let permit = pool.acquire(CapacityClass::Rho, &cancellation).await;
    assert!(permit.is_none());
    assert_eq!(pool.available_total(), 1);
}

// Covers: raising the live cap unblocks a waiter without restarting the process.
// Owner: delegated agent concurrency
#[tokio::test(flavor = "current_thread")]
async fn raising_total_unblocks_queued_run() {
    let pool = AgentConcurrency::new(limits(1, 1));
    let held = pool
        .acquire(CapacityClass::Rho, &RunCancellation::new())
        .await
        .expect("held");
    let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let queued = tokio::spawn({
        let pool = pool.clone();
        let started = Arc::clone(&started);
        async move {
            started.store(true, std::sync::atomic::Ordering::SeqCst);
            pool.acquire(CapacityClass::Rho, &RunCancellation::new())
                .await
        }
    });

    wait_until(|| started.load(std::sync::atomic::Ordering::SeqCst)).await;
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    assert!(!queued.is_finished());

    pool.set_total(2);
    let extra = tokio::time::timeout(std::time::Duration::from_secs(1), queued)
        .await
        .expect("raise should unblock")
        .unwrap()
        .expect("queued run acquired");
    drop(held);
    drop(extra);
    assert_eq!(pool.available_total(), 2);
}

// Covers: lowering the cap leaves in-flight runs; new work waits until active drops.
// Owner: delegated agent concurrency
#[tokio::test(flavor = "current_thread")]
async fn lowering_total_does_not_preempt_active_runs() {
    let pool = AgentConcurrency::new(limits(2, 2));
    let first = pool
        .acquire(CapacityClass::Rho, &RunCancellation::new())
        .await
        .expect("first");
    let second = pool
        .acquire(CapacityClass::Rho, &RunCancellation::new())
        .await
        .expect("second");
    pool.set_total(1);
    assert_eq!(pool.total_limit(), 1);
    assert_eq!(pool.available_total(), 0);

    let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let queued = tokio::spawn({
        let pool = pool.clone();
        let started = Arc::clone(&started);
        async move {
            started.store(true, std::sync::atomic::Ordering::SeqCst);
            pool.acquire(CapacityClass::Rho, &RunCancellation::new())
                .await
        }
    });
    wait_until(|| started.load(std::sync::atomic::Ordering::SeqCst)).await;
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    assert!(!queued.is_finished());

    drop(first);
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    assert!(
        !queued.is_finished(),
        "one in-flight run still occupies the lowered cap"
    );

    drop(second);
    let extra = tokio::time::timeout(std::time::Duration::from_secs(1), queued)
        .await
        .expect("slot after both releases")
        .unwrap()
        .expect("queued run acquired");
    drop(extra);
    assert_eq!(pool.available_total(), 1);
}

// Covers: raising total restores the nested Claude cap instead of leaving it clamped.
// Owner: delegated agent concurrency
#[tokio::test(flavor = "current_thread")]
async fn raising_total_restores_nested_claude_cap() {
    let pool = AgentConcurrency::new(limits(1, 2));
    assert_eq!(pool.available_claude(), 1);
    pool.set_total(10);
    assert_eq!(pool.available_claude(), 2);
    assert_eq!(pool.available_total(), 10);
}
