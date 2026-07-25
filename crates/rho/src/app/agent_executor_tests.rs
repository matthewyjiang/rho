use std::sync::Arc;

use super::*;
use crate::app::subagent_host_input::SubagentHostInputBridge;

#[test]
fn model_updates_are_shared_with_executor_clones() {
    let executor = AgentExecutor::new(
        Config::default(),
        PathBuf::new(),
        PathBuf::new(),
        SubagentHostInputBridge::new(),
    );
    let cloned = executor.clone();

    executor.update_model("openai-codex", "gpt-5.6-luna", rho_sdk::ReasoningLevel::Low);

    let config = cloned.config.read().expect("delegated config lock");
    assert_eq!(config.provider, "openai-codex");
    assert_eq!(config.model, "gpt-5.6-luna");
    assert_eq!(config.reasoning, rho_sdk::ReasoningLevel::Low);
}

#[test]
fn permission_mode_updates_are_shared_with_executor_clones() {
    let executor = AgentExecutor::new(
        Config::default(),
        PathBuf::new(),
        PathBuf::new(),
        SubagentHostInputBridge::new(),
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
fn default_concurrency_is_global_four_with_nested_claude_two() {
    let limits = concurrency_limits_from_env(None, None);
    assert_eq!(
        limits,
        ConcurrencyLimits {
            total: 4,
            claude: 2
        }
    );
}

#[test]
fn total_env_override_keeps_default_claude_cap_and_clamps() {
    let limits = concurrency_limits_from_env(Some("6"), None);
    assert_eq!(
        limits,
        ConcurrencyLimits {
            total: 6,
            claude: 2
        }
    );

    let tight = concurrency_limits_from_env(Some("1"), None);
    assert_eq!(
        tight,
        ConcurrencyLimits {
            total: 1,
            claude: 1
        }
    );
}

#[test]
fn claude_env_override_raises_nested_cap_within_total() {
    let limits = concurrency_limits_from_env(Some("6"), Some("4"));
    assert_eq!(
        limits,
        ConcurrencyLimits {
            total: 6,
            claude: 4
        }
    );
}

#[test]
fn claude_env_override_clamps_to_total() {
    let limits = concurrency_limits_from_env(Some("3"), Some("10"));
    assert_eq!(
        limits,
        ConcurrencyLimits {
            total: 3,
            claude: 3
        }
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

#[tokio::test]
async fn cancellation_interrupts_concurrency_queue() {
    let permits = Arc::new(tokio::sync::Semaphore::new(0));
    let cancellation = RunCancellation::new();
    let queued = tokio::spawn({
        let permits = Arc::clone(&permits);
        let cancellation = cancellation.clone();
        async move { acquire_permit_or_cancel(permits, &cancellation).await }
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

    let permit = acquire_permit_or_cancel(permits, &cancellation)
        .await
        .unwrap();

    assert!(permit.is_none());
}

#[test]
fn update_model_does_not_alter_bound_claude_runtime() {
    use crate::agent::{
        AgentDefinition, AgentId, AgentRuntime, AgentTools, ModelPolicy, ModelSelection,
        PromptPolicy,
    };
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
    );

    let definition = Arc::new(AgentDefinition {
        id: AgentId::new("claude-bound").unwrap(),
        description: "claude".into(),
        prompt: PromptPolicy::Replace("plan".into()),
        model: ModelPolicy::Select(ModelSelection {
            provider: None,
            model: "opus".into(),
        }),
        runtime: AgentRuntime::ClaudeCli,
        tools: AgentTools::Claude(vec!["Read".into()]),
        reasoning: None,
        inherit_claude_config: false,
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

    executor.update_model(
        "moonshot-kimi",
        "kimi-parent-after",
        rho_sdk::ReasoningLevel::High,
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
