use std::sync::Arc;

use super::*;
use crate::app::subagent_host_input::SubagentHostInputBridge;

// Covers: questionnaire routes to any parent-bridged delegated run, not only background.
// Owner: delegated agent executor.
#[test]
fn delegated_questionnaire_requires_parent_bridge_not_background() {
    let parent = rho_sdk::SessionId::new();
    assert!(delegated_questionnaire_available(Some(&parent), true));
    assert!(!delegated_questionnaire_available(Some(&parent), false));
    assert!(!delegated_questionnaire_available(None, true));
    assert!(!delegated_questionnaire_available(None, false));
}

#[test]
fn provider_selection_updates_are_shared_with_executor_clones() {
    let executor = AgentExecutor::new(
        Config::default(),
        PathBuf::new(),
        PathBuf::new(),
        SubagentHostInputBridge::new(),
        crate::app::subagent_messaging::SubagentNoticeBridge::new(),
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
        crate::app::subagent_messaging::SubagentNoticeBridge::new(),
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
        crate::app::subagent_messaging::SubagentNoticeBridge::new(),
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
        BoundRuntime::Rho { .. } | BoundRuntime::Cursor { .. } => {
            panic!("expected Claude bound runtime")
        }
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
        BoundRuntime::Rho { .. } | BoundRuntime::Cursor { .. } => {
            panic!("expected Claude bound runtime")
        }
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

// Covers: Cursor children are process-per-turn and reject parent messages.
// Owner: delegated agent executor
#[tokio::test]
async fn cursor_child_refuses_parent_messages() {
    use pretty_assertions::assert_eq;

    let (_status_tx, status_rx) = tokio::sync::watch::channel(RunStatus::default());
    let (_completion_tx, completion_rx) = tokio::sync::watch::channel(false);
    let handle = AgentRunHandle {
        cancellation: RunCancellation::new(),
        status: status_rx,
        completion: completion_rx,
        messaging: MessagingSupport::Unsupported,
    };
    let error = handle
        .message_from_parent(
            &crate::app::subagent_messaging::ValidatedMessage::parse("steer").unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "cursor runs are process-per-turn and cannot accept messages; wait for completion"
    );
}
