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

// Covers: the global cap comes from config, never the Claude env; invalid
// Claude env keeps the default nested cap, and both clamp to the named max.
// Owner: delegated agent concurrency
#[test]
fn concurrency_limits_take_total_from_config_and_nested_cap_from_claude_env() {
    let huge = format!("{}0", usize::MAX);
    let cases = [
        (None, 0, 1, 2),
        (
            Some("0"),
            DEFAULT_AGENT_CONCURRENCY,
            DEFAULT_AGENT_CONCURRENCY,
            2,
        ),
        (
            Some("-1"),
            DEFAULT_AGENT_CONCURRENCY,
            DEFAULT_AGENT_CONCURRENCY,
            2,
        ),
        (
            Some(""),
            DEFAULT_AGENT_CONCURRENCY,
            DEFAULT_AGENT_CONCURRENCY,
            2,
        ),
        (Some(huge.as_str()), 1_000, MAX_AGENT_CONCURRENCY, 2),
        (
            Some("3"),
            DEFAULT_AGENT_CONCURRENCY,
            DEFAULT_AGENT_CONCURRENCY,
            3,
        ),
        (Some("5"), 8, 8, 5),
        (Some("90"), 8, 8, MAX_AGENT_CONCURRENCY),
    ];
    for (claude_env, configured, total, claude_requested) in cases {
        assert_eq!(
            concurrency_limits_from_claude_env(claude_env, configured),
            ConcurrencyLimits {
                total,
                claude_requested
            },
            "{claude_env:?} with configured {configured}"
        );
    }
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

// Covers: runs take capacity in the order they joined, not in the order
// their tasks wake, and each waiting run knows the budget, its limit, and
// its place; a run that gives up its place moves the ones behind it up.
// Owner: delegated agent concurrency
#[tokio::test(flavor = "current_thread")]
async fn runs_take_capacity_in_the_order_they_joined() {
    let pool = AgentConcurrency::new(limits(1, 1));
    let held = pool
        .acquire(CapacityClass::Rho, &RunCancellation::new())
        .await
        .expect("held");
    let first = pool.join(CapacityClass::Rho);
    let second = pool.join(CapacityClass::Rho);
    let third = pool.join(CapacityClass::Rho);
    let place = |position| {
        Some(QueuePlace {
            budget: Budget::AgentConcurrency,
            limit: 1,
            position,
        })
    };
    let (first_watch, second_watch, third_watch) = (first.watch(), second.watch(), third.watch());
    assert_eq!(
        [
            first_watch.place(),
            second_watch.place(),
            third_watch.place()
        ],
        [place(1), place(2), place(3)]
    );

    // The last to join asks first; it still waits behind the others.
    let third = tokio::spawn(async move { third.acquire(&RunCancellation::new()).await });
    tokio::task::yield_now().await;
    drop(first);
    assert_eq!(
        [second_watch.place(), third_watch.place()],
        [place(1), place(2)]
    );

    drop(held);
    assert_eq!(
        [second_watch.place(), third_watch.place()],
        [None, place(1)]
    );
    let second = second
        .acquire(&RunCancellation::new())
        .await
        .expect("second takes the freed slot");
    assert!(!third.is_finished());

    drop(second);
    let third = tokio::time::timeout(std::time::Duration::from_secs(1), third)
        .await
        .expect("third takes the next slot")
        .unwrap()
        .expect("third acquired");
    assert_eq!(third_watch.place(), None);
    drop(third);
    assert_eq!(pool.available_total(), 1);
}

// Covers: a Claude run waits first on the Claude budget, then on the global
// one, and its place names the budget it is waiting on.
// Owner: delegated agent concurrency
#[tokio::test(flavor = "current_thread")]
async fn a_claude_run_reports_the_budget_it_waits_on() {
    let pool = AgentConcurrency::new(limits(1, 1));
    let claude = pool
        .acquire(CapacityClass::Claude, &RunCancellation::new())
        .await
        .expect("held Claude");
    let queued = pool.join(CapacityClass::Claude);
    let watch = queued.watch();
    assert_eq!(
        watch.place(),
        Some(QueuePlace {
            budget: Budget::ClaudeAgentConcurrency,
            limit: 1,
            position: 1,
        })
    );
    let queued = tokio::spawn(async move { queued.acquire(&RunCancellation::new()).await });
    let rho = pool.join(CapacityClass::Rho);
    drop(claude);
    wait_until(|| {
        watch
            .place()
            .is_some_and(|place| place.budget == Budget::AgentConcurrency)
    })
    .await;
    assert_eq!(
        watch.place(),
        Some(QueuePlace {
            budget: Budget::AgentConcurrency,
            limit: 1,
            position: 1,
        }),
        "the Rho run that joined the global queue first took its slot"
    );
    assert_eq!(pool.available_total(), 0);
    drop(rho);
    let queued = tokio::time::timeout(std::time::Duration::from_secs(1), queued)
        .await
        .expect("Claude run takes the global slot")
        .unwrap()
        .expect("acquired");
    assert_eq!(watch.place(), None);
    drop(queued);
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
