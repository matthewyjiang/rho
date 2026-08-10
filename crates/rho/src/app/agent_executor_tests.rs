use std::sync::Arc;

use super::*;
use crate::app::subagent_host_input::SubagentHostInputBridge;

#[test]
fn provider_selection_updates_are_shared_with_executor_clones() {
    let executor = AgentExecutor::new(
        Config::default(),
        PathBuf::new(),
        PathBuf::new(),
        SubagentHostInputBridge::new(),
        crate::app::subagent_notice::SubagentNoticeBridge::new(),
    );
    let cloned = executor.clone();

    executor.update_selection(
        "openai-codex",
        "gpt-5.6-luna",
        rho_sdk::ReasoningLevel::Low,
        "codex",
    );

    let config = cloned.config.read().expect("delegated config lock");
    assert_eq!(config.provider, "openai-codex");
    assert_eq!(config.model, "gpt-5.6-luna");
    assert_eq!(config.auth, "codex");
    assert_eq!(config.reasoning, rho_sdk::ReasoningLevel::Low);
}

#[test]
fn permission_mode_updates_are_shared_with_executor_clones() {
    let executor = AgentExecutor::new(
        Config::default(),
        PathBuf::new(),
        PathBuf::new(),
        SubagentHostInputBridge::new(),
        crate::app::subagent_notice::SubagentNoticeBridge::new(),
    );
    let cloned = executor.clone();

    executor.update_permission_mode(crate::permission::PermissionMode::Plan);
    assert_eq!(
        cloned.launch_permission_mode(),
        crate::permission::PermissionMode::Plan
    );

    cloned.update_permission_mode(crate::permission::PermissionMode::Supervised);
    assert_eq!(
        executor.launch_permission_mode(),
        crate::permission::PermissionMode::Supervised
    );
}

#[test]
fn zero_invalid_and_huge_concurrency_values_fall_back() {
    assert_eq!(
        concurrency_limits_from_env(Some("0"), Some("0")),
        ConcurrencyLimits {
            total: 4,
            claude: 2
        }
    );
    assert_eq!(
        concurrency_limits_from_env(Some("-1"), Some("nope")),
        ConcurrencyLimits {
            total: 4,
            claude: 2
        }
    );
    assert_eq!(
        concurrency_limits_from_env(Some(""), Some(" ")),
        ConcurrencyLimits {
            total: 4,
            claude: 2
        }
    );
    // Larger than usize::MAX decimal representation is rejected by parse.
    let huge = format!("{}0", usize::MAX);
    assert_eq!(
        concurrency_limits_from_env(Some(huge.as_str()), Some(huge.as_str())),
        ConcurrencyLimits {
            total: 4,
            claude: 2
        }
    );
}

#[test]
fn total_and_claude_env_values_interact() {
    // Valid Claude override with invalid total keeps default total and clamps.
    assert_eq!(
        concurrency_limits_from_env(Some("bad"), Some("3")),
        ConcurrencyLimits {
            total: 4,
            claude: 3
        }
    );
    // Valid total with invalid Claude keeps default Claude, clamped to total.
    assert_eq!(
        concurrency_limits_from_env(Some("1"), Some("bad")),
        ConcurrencyLimits {
            total: 1,
            claude: 1
        }
    );
    // Both valid: Claude is min(requested, total).
    assert_eq!(
        concurrency_limits_from_env(Some("8"), Some("5")),
        ConcurrencyLimits {
            total: 8,
            claude: 5
        }
    );
}

const CLOSED_MSG: &str = "test concurrency pool closed";

#[tokio::test]
async fn cancellation_interrupts_concurrency_queue() {
    let permits = Arc::new(tokio::sync::Semaphore::new(0));
    let cancellation = RunCancellation::new();
    let queued = tokio::spawn({
        let permits = Arc::clone(&permits);
        let cancellation = cancellation.clone();
        async move { acquire_permit_or_cancel(permits, &cancellation, CLOSED_MSG).await }
    });

    cancellation.cancel();

    let permit = tokio::time::timeout(std::time::Duration::from_secs(1), queued)
        .await
        .expect("queued acquisition should observe cancellation")
        .unwrap()
        .unwrap();
    assert!(permit.is_none());
}

#[tokio::test]
async fn cancellation_wins_when_a_permit_is_already_available() {
    let permits = Arc::new(tokio::sync::Semaphore::new(1));
    let cancellation = RunCancellation::new();
    cancellation.cancel();

    let permit = acquire_permit_or_cancel(permits, &cancellation, CLOSED_MSG)
        .await
        .unwrap();

    assert!(permit.is_none());
}

#[tokio::test]
async fn closed_semaphore_returns_clear_error() {
    let permits = Arc::new(tokio::sync::Semaphore::new(1));
    permits.close();

    let error = acquire_permit_or_cancel(permits, &RunCancellation::new(), CLOSED_MSG)
        .await
        .expect_err("closed pool should error");
    assert!(
        error.to_string().contains(CLOSED_MSG),
        "unexpected error: {error:#}"
    );
}

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

#[tokio::test(flavor = "current_thread")]
async fn claude_queue_does_not_starve_rho_and_progresses_after_release() {
    // total=2, claude=1: one active Claude holds both pools; a second Claude
    // waits on Claude capacity without taking the spare global slot, so Rho
    // can still start. After active Claude and Rho release, queued Claude
    // progresses.
    let total = Arc::new(tokio::sync::Semaphore::new(2));
    let claude = Arc::new(tokio::sync::Semaphore::new(1));

    let active_claude = acquire_runtime_permits(
        Arc::clone(&total),
        Arc::clone(&claude),
        CapacityClass::Claude,
        &RunCancellation::new(),
    )
    .await
    .unwrap()
    .expect("active Claude should acquire");
    assert_eq!(total.available_permits(), 1);
    assert_eq!(claude.available_permits(), 0);

    let queued_started = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let queued_claude = tokio::spawn({
        let total = Arc::clone(&total);
        let claude = Arc::clone(&claude);
        let queued_started = Arc::clone(&queued_started);
        async move {
            queued_started.store(true, std::sync::atomic::Ordering::SeqCst);
            acquire_runtime_permits(
                total,
                claude,
                CapacityClass::Claude,
                &RunCancellation::new(),
            )
            .await
        }
    });

    wait_until(|| queued_started.load(std::sync::atomic::Ordering::SeqCst)).await;
    // Yield so the queued Claude task reaches its Claude-pool wait.
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
    assert!(
        !queued_claude.is_finished(),
        "queued Claude must still wait on Claude capacity"
    );
    // Spare global capacity must remain free while Claude is nested-waiting.
    assert_eq!(total.available_permits(), 1);
    assert_eq!(claude.available_permits(), 0);

    let rho = acquire_runtime_permits(
        Arc::clone(&total),
        Arc::clone(&claude),
        CapacityClass::Rho,
        &RunCancellation::new(),
    )
    .await
    .unwrap()
    .expect("Rho should take the spare global permit");
    assert_eq!(total.available_permits(), 0);
    assert_eq!(claude.available_permits(), 0);
    assert!(
        !queued_claude.is_finished(),
        "queued Claude must not finish while Claude capacity is held"
    );

    // Releasing the active Claude frees nested Claude capacity and one global
    // slot. Queued Claude can finish even while Rho still holds its spare
    // global permit - that is the non-starvation property.
    drop(active_claude);
    let queued = tokio::time::timeout(std::time::Duration::from_secs(1), queued_claude)
        .await
        .expect("queued Claude should acquire after active Claude releases")
        .unwrap()
        .unwrap()
        .expect("queued Claude should not cancel");
    // Rho still holds one global permit; queued Claude took the freed pair.
    assert_eq!(total.available_permits(), 0);
    assert_eq!(claude.available_permits(), 0);

    drop(rho);
    drop(queued);
    assert_eq!(total.available_permits(), 2);
    assert_eq!(claude.available_permits(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn claude_waits_on_global_after_taking_claude_capacity() {
    // Global full (two Rho runs), Claude free: Claude takes nested capacity
    // first, waits on global, then finishes when a Rho slot frees.
    let total = Arc::new(tokio::sync::Semaphore::new(2));
    let claude = Arc::new(tokio::sync::Semaphore::new(1));

    let rho_a = acquire_runtime_permits(
        Arc::clone(&total),
        Arc::clone(&claude),
        CapacityClass::Rho,
        &RunCancellation::new(),
    )
    .await
    .unwrap()
    .expect("rho a");
    let rho_b = acquire_runtime_permits(
        Arc::clone(&total),
        Arc::clone(&claude),
        CapacityClass::Rho,
        &RunCancellation::new(),
    )
    .await
    .unwrap()
    .expect("rho b");
    assert_eq!(total.available_permits(), 0);
    assert_eq!(claude.available_permits(), 1);

    let queued = tokio::spawn({
        let total = Arc::clone(&total);
        let claude = Arc::clone(&claude);
        async move {
            acquire_runtime_permits(
                total,
                claude,
                CapacityClass::Claude,
                &RunCancellation::new(),
            )
            .await
        }
    });

    wait_until(|| claude.available_permits() == 0).await;
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    assert!(!queued.is_finished());
    assert_eq!(total.available_permits(), 0);
    assert_eq!(claude.available_permits(), 0);

    drop(rho_a);
    let permits = tokio::time::timeout(std::time::Duration::from_secs(1), queued)
        .await
        .expect("Claude should finish once a global slot frees")
        .unwrap()
        .unwrap()
        .expect("Claude acquired");
    assert_eq!(total.available_permits(), 0);
    assert_eq!(claude.available_permits(), 0);
    drop(rho_b);
    drop(permits);
    assert_eq!(total.available_permits(), 2);
    assert_eq!(claude.available_permits(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_during_claude_wait_releases_nothing() {
    let total = Arc::new(tokio::sync::Semaphore::new(2));
    let claude = Arc::new(tokio::sync::Semaphore::new(0));
    let cancellation = RunCancellation::new();

    let queued = tokio::spawn({
        let total = Arc::clone(&total);
        let claude = Arc::clone(&claude);
        let cancellation = cancellation.clone();
        async move { acquire_runtime_permits(total, claude, CapacityClass::Claude, &cancellation).await }
    });

    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
    cancellation.cancel();

    let result = tokio::time::timeout(std::time::Duration::from_secs(1), queued)
        .await
        .expect("cancel during Claude wait")
        .unwrap()
        .unwrap();
    assert!(result.is_none());
    assert_eq!(total.available_permits(), 2);
    assert_eq!(claude.available_permits(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_during_global_wait_releases_claude_permit() {
    let total = Arc::new(tokio::sync::Semaphore::new(0));
    let claude = Arc::new(tokio::sync::Semaphore::new(1));
    let cancellation = RunCancellation::new();

    let queued = tokio::spawn({
        let total = Arc::clone(&total);
        let claude = Arc::clone(&claude);
        let cancellation = cancellation.clone();
        async move { acquire_runtime_permits(total, claude, CapacityClass::Claude, &cancellation).await }
    });

    // Let the task take the Claude permit and block on global.
    wait_until(|| claude.available_permits() == 0).await;
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    assert_eq!(total.available_permits(), 0);
    assert_eq!(claude.available_permits(), 0);

    cancellation.cancel();

    let result = tokio::time::timeout(std::time::Duration::from_secs(1), queued)
        .await
        .expect("cancel during global wait")
        .unwrap()
        .unwrap();
    assert!(result.is_none());
    // Claude permit acquired before the global wait must be returned.
    assert_eq!(total.available_permits(), 0);
    assert_eq!(claude.available_permits(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_during_rho_global_wait_holds_nothing() {
    let total = Arc::new(tokio::sync::Semaphore::new(0));
    let claude = Arc::new(tokio::sync::Semaphore::new(1));
    let cancellation = RunCancellation::new();

    let queued = tokio::spawn({
        let total = Arc::clone(&total);
        let claude = Arc::clone(&claude);
        let cancellation = cancellation.clone();
        async move { acquire_runtime_permits(total, claude, CapacityClass::Rho, &cancellation).await }
    });

    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
    cancellation.cancel();

    let result = tokio::time::timeout(std::time::Duration::from_secs(1), queued)
        .await
        .expect("cancel during Rho global wait")
        .unwrap()
        .unwrap();
    assert!(result.is_none());
    assert_eq!(total.available_permits(), 0);
    assert_eq!(claude.available_permits(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn rho_skips_claude_pool_entirely() {
    let total = Arc::new(tokio::sync::Semaphore::new(1));
    // Claude pool empty: Rho must still acquire.
    let claude = Arc::new(tokio::sync::Semaphore::new(0));

    let rho = acquire_runtime_permits(
        Arc::clone(&total),
        Arc::clone(&claude),
        CapacityClass::Rho,
        &RunCancellation::new(),
    )
    .await
    .unwrap()
    .expect("Rho ignores Claude pool");
    assert_eq!(total.available_permits(), 0);
    assert_eq!(claude.available_permits(), 0);
    drop(rho);
    assert_eq!(total.available_permits(), 1);
    assert_eq!(claude.available_permits(), 0);
}

#[test]
fn update_selection_does_not_alter_bound_claude_runtime() {
    use crate::agent::{AgentDefinition, AgentId, AgentRuntimeSpec, PromptPolicy};
    use crate::app::agent_binding::{AgentBinder, AgentInvocation, AgentRole, BoundRuntime};

    let executor = AgentExecutor::new(
        Config {
            provider: "openai-codex".into(),
            model: "gpt-parent-before".into(),
            permission_mode: crate::permission::PermissionMode::Plan,
            ..Config::default()
        },
        PathBuf::new(),
        PathBuf::new(),
        SubagentHostInputBridge::new(),
        crate::app::subagent_notice::SubagentNoticeBridge::new(),
    );

    let definition = Arc::new(AgentDefinition {
        id: AgentId::new("claude-bound").unwrap(),
        description: "claude".into(),
        prompt: PromptPolicy::Replace("plan".into()),
        runtime: AgentRuntimeSpec::ClaudeCli(crate::agent::ClaudeAgentConfig {
            tools: crate::agent::ClaudeToolPolicy::Allow(vec!["Read".into()]),
            inherit_claude_config: false,
            model: Some("opus".into()),
            reasoning: None,
        }),
    });

    // Bind before the parent model changes.
    let host_before = executor
        .config
        .read()
        .expect("delegated config lock")
        .clone();
    let bound_before = AgentBinder::bind(
        Arc::clone(&definition),
        AgentInvocation {
            role: AgentRole::Delegated,
            available_tools: crate::agent::AgentCapabilities::default(),
        },
        &host_before,
    )
    .unwrap();

    executor.update_selection(
        "moonshot-kimi",
        "kimi-parent-after",
        rho_sdk::ReasoningLevel::High,
        "kimi-oauth",
    );

    let host_after = executor
        .config
        .read()
        .expect("delegated config lock")
        .clone();
    assert_eq!(host_after.provider, "moonshot-kimi");
    assert_eq!(host_after.model, "kimi-parent-after");

    // Already-bound Claude runtime keeps definition model/tools; parent snapshot
    // is irrelevant once BoundRuntime::ClaudeCli is produced.
    match bound_before.runtime() {
        BoundRuntime::ClaudeCli {
            model,
            tools,
            inherit_claude_config,
            permission_mode,
            ..
        } => {
            assert_eq!(model.as_deref(), Some("opus"));
            assert_eq!(tools.as_slice(), ["Read".to_string()].as_slice());
            assert!(!*inherit_claude_config);
            assert_eq!(*permission_mode, crate::permission::PermissionMode::Plan);
        }
        BoundRuntime::Rho { .. } => panic!("expected Claude bound runtime"),
    }
    assert!(bound_before.rho_config().is_none());

    // Re-bind after update_model: Claude model still comes from definition only.
    let bound_after = AgentBinder::bind(
        definition,
        AgentInvocation {
            role: AgentRole::Delegated,
            available_tools: crate::agent::AgentCapabilities::default(),
        },
        &host_after,
    )
    .unwrap();
    match bound_after.runtime() {
        BoundRuntime::ClaudeCli {
            model,
            permission_mode,
            ..
        } => {
            assert_eq!(model.as_deref(), Some("opus"));
            // Permission mode is the only host field Claude bind snapshots.
            assert_eq!(*permission_mode, crate::permission::PermissionMode::Plan);
        }
        BoundRuntime::Rho { .. } => panic!("expected Claude bound runtime"),
    }
    assert_ne!(host_after.model, "opus");
    assert_ne!(host_after.provider, "claude-code");
}

#[test]
fn ensure_stream_json_input_is_idempotent() {
    let bare = vec!["-p".into(), "--output-format".into(), "stream-json".into()];
    let once = ensure_stream_json_input(bare.clone());
    assert!(once
        .windows(2)
        .any(|w| w == ["--input-format", "stream-json"]));
    let twice = ensure_stream_json_input(once.clone());
    assert_eq!(once, twice);
}
