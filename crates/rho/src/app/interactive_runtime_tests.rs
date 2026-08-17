use std::sync::Arc;

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, Message, ModelIdentity, ModelResponse, ModelUsage},
    provider::{ModelProvider, ScriptedProvider, ScriptedTurn},
    ApprovalDecision, ApprovalFuture, ApprovalHandler, ApprovalRequest, CompactionFuture,
    CompactionOutput, CompactionRequest, Compactor, PolicyDecision, ProviderError,
    ProviderErrorKind, Retryability, RunEvent, RunId, SessionId, SessionOptions, SystemPrompt,
    UserInput, Workspace, WorkspacePolicy,
};

use super::{
    build_runtime, InteractiveRunController, InteractiveRuntime, InteractiveSessionController,
    ProviderController, RuntimeBuildOptions,
};
use crate::{
    agent::{AgentCapabilities, ToolCapability},
    app::policy::AppPolicy,
    compaction::CompactionConfig,
    config::Config,
    diagnostics::RuntimeDiagnostics,
    permission::{PermissionMode, WriteAuthority},
    session::Session as StoredSession,
    tools::{
        agent::BackgroundSubagents,
        sdk_registry::{AppToolSet, DelegationConfig, ToolSetOptions},
    },
};

#[tokio::test]
async fn configured_token_threshold_installs_sdk_automatic_compaction_policy() {
    let provider = ScriptedProvider::new(
        ModelIdentity::new("test", "test", "test"),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "compact summary".into(),
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "done".into(),
            )])),
        ],
    );
    let shared_provider: Arc<dyn ModelProvider> = Arc::new(provider.clone());
    let tools = AppToolSet::disabled();
    let workspace = Workspace::new(std::env::current_dir().unwrap()).unwrap();
    let runtime = build_runtime(RuntimeBuildOptions {
        provider: shared_provider,
        tools: tools.tools(),
        workspace,
        workspace_policy: AppPolicy::for_mode(PermissionMode::Auto, Default::default()),
        approval_session: None,
        system_prompt: SystemPrompt::None,
        reasoning: rho_sdk::ReasoningLevel::Off,
        service_tier: None,
        compaction: CompactionConfig {
            auto_compact: true,
            threshold_percent: 1,
            target_percent: 1,
        },
        context_window: Some(1_000),
        usage_purpose: "agent",
        usage_parent_session_id: None,
        usage_recording: Default::default(),
        hook_host_labels: rho_sdk::hooks::HookHostLabels::new(),
        hooks: None,
    })
    .unwrap();
    assert_eq!(runtime.diagnostics().compaction_trigger_tokens(), Some(10));
    let session = runtime
        .session(SessionOptions::new().history(vec![
            rho_sdk::model::Message::user_text("x".repeat(2_000)),
            rho_sdk::model::Message::assistant_text("y".repeat(2_000)),
        ]))
        .await
        .unwrap();

    let mut run = session.start(UserInput::text("continue")).await.unwrap();
    let mut events = Vec::new();
    while let Some(event) = run.next_event().await {
        events.push(event);
    }
    let outcome = run.outcome().await.unwrap();

    assert_eq!(outcome.text(), "done");
    assert!(events.iter().any(|event| matches!(
        event,
        RunEvent::CompactionStarted {
            trigger: rho_sdk::CompactionTrigger::Automatic,
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        RunEvent::CompactionCompleted {
            trigger: rho_sdk::CompactionTrigger::Automatic,
            ..
        }
    )));
    assert_eq!(provider.recorded_requests().len(), 2);
}

struct PendingCompactor;

impl Compactor for PendingCompactor {
    fn compact<'a>(&'a self, _request: CompactionRequest) -> CompactionFuture<'a> {
        Box::pin(std::future::pending::<
            Result<CompactionOutput, rho_sdk::Error>,
        >())
    }
}

#[tokio::test]
async fn set_context_window_installs_automatic_compaction_when_idle() {
    let mut interactive = pending_compaction_runtime("done").await;
    interactive.compaction = CompactionConfig {
        auto_compact: true,
        threshold_percent: 1,
        target_percent: 1,
    };
    assert_eq!(
        interactive
            .sessions
            .session()
            .diagnostics()
            .compaction_trigger_tokens(),
        None
    );

    interactive.set_context_window(Some(1_000)).unwrap();

    assert_eq!(
        interactive
            .sessions
            .session()
            .diagnostics()
            .compaction_trigger_tokens(),
        Some(10)
    );
}

#[tokio::test]
async fn replace_provider_rebuilds_compactor_with_current_context_window() {
    let mut interactive = pending_compaction_runtime("done").await;
    interactive.compaction = CompactionConfig {
        auto_compact: true,
        threshold_percent: 80,
        target_percent: 50,
    };
    interactive.context_window = Some(2_000);
    let replacement: Arc<dyn ModelProvider> = Arc::new(ScriptedProvider::new(
        ModelIdentity::new("replacement", "test", "model"),
        Vec::<ScriptedTurn>::new(),
    ));

    interactive
        .replace_provider(
            Arc::clone(&replacement),
            rho_sdk::ReasoningLevel::Low,
            "test-auth",
        )
        .unwrap();

    assert_eq!(
        interactive
            .sessions
            .session()
            .diagnostics()
            .compaction_trigger_tokens(),
        Some(1_600)
    );
    assert_eq!(
        interactive.sessions.session().diagnostics().provider(),
        &ModelIdentity::new("replacement", "test", "model")
    );
    assert_eq!(
        interactive.sessions.session().reasoning_level(),
        rho_sdk::ReasoningLevel::Low
    );
}

async fn test_runtime(turns: Vec<ScriptedTurn>) -> InteractiveRuntime {
    let provider = Arc::new(ScriptedProvider::new(
        ModelIdentity::new("test", "test", "test"),
        turns,
    ));
    let shared_provider: Arc<dyn ModelProvider> = provider;
    let tools = AppToolSet::disabled();
    let workspace = Workspace::new(std::env::current_dir().unwrap()).unwrap();
    let runtime = rho_sdk::Rho::builder()
        .provider_shared(Arc::clone(&shared_provider))
        .compactor(PendingCompactor)
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    InteractiveRuntime {
        runtime,
        hooks: None,
        runs: InteractiveRunController::default(),
        sessions: InteractiveSessionController::new(
            session,
            None,
            crate::tools::web::WebAccessStore::new(),
            tools.advisor().cloned(),
        ),
        mcp_sampling: crate::tools::mcp::McpSamplingBridge::new(),
        provider: ProviderController::new(shared_provider, rho_sdk::ReasoningLevel::Off),
        tools,
        mcp_report: Default::default(),
        pending_mcp: None,
        pending_catalog_names: None,
        may_rewrite_startup_prompt: false,
        plugins_report: Default::default(),
        workspace,
        system_prompt: SystemPrompt::None,
        compaction: CompactionConfig::default(),
        pending_compact: None,
        context_window: None,
        usage_recording: Default::default(),
        config: Config::default(),
        permission_mode: PermissionMode::Auto,
        approval_handler: None,
        approval_receiver: None,
        classifier_approval_handler: None,
        agent: test_bound_agent(),
        agent_id: "default".into(),
        agent_fingerprint: "test-fingerprint".into(),
        completed_runs: 0,
        pending_persistence_error: None,
        pending_persistence_checkpoint: None,
        experimental_workspace_rewind: false,
        session_writes: Default::default(),
        live_context_warm: false,
    }
}

fn test_bound_agent() -> crate::app::agent_binding::BoundAgent {
    crate::app::agent_binding::AgentBinder::bind(
        std::sync::Arc::new(crate::agent::AgentDefinition {
            id: crate::agent::AgentId::new("default").unwrap(),
            description: "test".into(),
            prompt: crate::agent::PromptPolicy::Extend(String::new()),
            runtime: crate::agent::AgentRuntimeSpec::Rho {
                tools: crate::agent::ToolPolicy::All,
                model: crate::agent::ModelPolicy::Inherit,
                reasoning: None,
            },
        }),
        crate::app::agent_binding::AgentInvocation {
            role: crate::app::agent_binding::AgentRole::InteractiveRoot,
            available_tools: crate::agent::AgentCapabilities::all_host_tools(),
        },
        &crate::config::Config::default(),
    )
    .unwrap()
}

async fn pending_compaction_runtime(response: &str) -> InteractiveRuntime {
    test_runtime(vec![ScriptedTurn::completed(ModelResponse::Assistant(
        vec![ContentBlock::Text(response.into())],
    ))])
    .await
}

async fn permission_mode_runtime() -> InteractiveRuntime {
    let mut interactive = pending_compaction_runtime("done").await;
    let config = Config::default();
    let capabilities = AgentCapabilities::new(
        [ToolCapability::Agent, ToolCapability::Agents]
            .into_iter()
            .collect(),
    );
    interactive.tools = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::new(capabilities).delegation(DelegationConfig::new(
            std::env::current_dir().unwrap(),
            std::path::PathBuf::new(),
            BackgroundSubagents::Disabled,
            /*catalog*/ None,
        )),
    );
    interactive
}

#[tokio::test]
async fn permission_mode_switch_rebuilds_runtime_and_updates_future_delegated_policy() {
    let mut interactive = permission_mode_runtime().await;
    interactive
        .sessions
        .session()
        .append_message(Message::user_text("preserved history"))
        .unwrap();
    let session_id = interactive.sessions.session().id().clone();
    let history = interactive.sessions.session().history();

    interactive
        .set_permission_mode(PermissionMode::Plan)
        .await
        .unwrap();
    assert_eq!(interactive.permission_mode(), PermissionMode::Plan);
    assert_eq!(
        interactive
            .tools
            .subagents()
            .unwrap()
            .launch_permission_mode()
            .decision_for(rho_sdk::CapabilityKind::Write),
        PolicyDecision::Deny {
            reason: "capability is not allowed in plan mode".into()
        }
    );
    assert!(interactive.approval_handler.is_none());
    assert!(interactive.approval_receiver().is_none());
    assert_eq!(interactive.sessions.session().id(), &session_id);
    assert_eq!(interactive.sessions.session().history(), history);

    interactive
        .set_permission_mode(PermissionMode::Supervised)
        .await
        .unwrap();
    assert_eq!(interactive.permission_mode(), PermissionMode::Supervised);
    assert_eq!(
        interactive
            .tools
            .subagents()
            .unwrap()
            .launch_permission_mode()
            .decision_for(rho_sdk::CapabilityKind::Write),
        PolicyDecision::RequireApproval {
            reason: String::new()
        }
    );
    assert!(interactive.approval_handler.is_some());
    assert!(interactive.approval_receiver().is_some());
    assert_eq!(interactive.sessions.session().id(), &session_id);
    assert_eq!(interactive.sessions.session().history(), history);
    let supervised_handler = interactive.approval_handler.clone().unwrap();
    interactive
        .set_permission_mode(PermissionMode::Supervised)
        .await
        .unwrap();
    assert!(Arc::ptr_eq(
        interactive.approval_handler.as_ref().unwrap(),
        &supervised_handler
    ));

    interactive
        .set_permission_mode(PermissionMode::Bypass)
        .await
        .unwrap();
    assert_eq!(interactive.permission_mode(), PermissionMode::Bypass);
    assert!(interactive.approval_handler.is_none());
    assert!(interactive.approval_receiver().is_none());
    assert!(interactive.classifier_approval_handler.is_none());
    assert_eq!(interactive.sessions.session().id(), &session_id);
    assert_eq!(interactive.sessions.session().history(), history);

    interactive
        .set_permission_mode(PermissionMode::Auto)
        .await
        .unwrap();
    assert_eq!(interactive.permission_mode(), PermissionMode::Auto);
    assert!(interactive.approval_handler.is_some());
    assert!(interactive.approval_receiver().is_some());
    assert!(interactive.classifier_approval_handler.is_some());
    assert_eq!(interactive.sessions.session().id(), &session_id);
    assert_eq!(interactive.sessions.session().history(), history);

    interactive
        .set_permission_mode(PermissionMode::Bypass)
        .await
        .unwrap();
    assert_eq!(interactive.permission_mode(), PermissionMode::Bypass);
    assert!(interactive.approval_handler.is_none());
    assert!(interactive.approval_receiver().is_none());
    assert!(interactive.classifier_approval_handler.is_none());
    assert_eq!(interactive.sessions.session().id(), &session_id);
    assert_eq!(interactive.sessions.session().history(), history);
}

#[tokio::test]
async fn permission_mode_switch_preserves_a_pending_new_session() {
    let mut interactive = pending_compaction_runtime("done").await;
    let previous_id = interactive.session_id().clone();
    interactive.reset().await.unwrap();
    let pending_id = interactive.session_id().clone();
    assert_ne!(pending_id, previous_id);

    interactive
        .set_permission_mode(PermissionMode::Plan)
        .await
        .unwrap();
    interactive
        .set_permission_mode(PermissionMode::Auto)
        .await
        .unwrap();
    assert_eq!(interactive.session_id(), &pending_id);

    interactive
        .start(UserInput::text("first new-session turn"), None)
        .await
        .unwrap();
    while interactive.next_event().await.is_some() {}
    interactive.finish_run().await.unwrap();

    assert_eq!(interactive.session_id(), &pending_id);
    assert_eq!(interactive.sessions.session().id(), &pending_id);
}

#[tokio::test]
async fn permission_mode_switch_rejects_an_active_run_without_mutation() {
    let mut interactive = pending_compaction_runtime("done").await;
    interactive
        .start(UserInput::text("start"), None)
        .await
        .unwrap();

    let error = interactive
        .set_permission_mode(PermissionMode::Supervised)
        .await
        .unwrap_err();

    assert!(error.to_string().contains("while a run is active"));
    assert_eq!(interactive.permission_mode(), PermissionMode::Auto);
    assert!(interactive.approval_receiver().is_none());
}

// Covers: session-changing APIs must not run while a compact job holds a cloned session.
// Owner: interactive runtime compact lifecycle
#[tokio::test]
async fn session_changes_are_rejected_while_compaction_is_in_flight() {
    let mut interactive = pending_compaction_runtime("done").await;
    interactive.begin_compact_task().unwrap();
    assert!(interactive.is_compacting());

    let permission_error = interactive
        .set_permission_mode(PermissionMode::Supervised)
        .await
        .unwrap_err();
    assert!(
        permission_error
            .to_string()
            .contains("while compaction is active"),
        "{permission_error}"
    );
    assert_eq!(interactive.permission_mode(), PermissionMode::Auto);

    let reset_error = interactive.reset().await.unwrap_err();
    assert!(
        reset_error
            .to_string()
            .contains("while a run or compaction is active"),
        "{reset_error}"
    );

    let replacement: Arc<dyn ModelProvider> = Arc::new(ScriptedProvider::new(
        ModelIdentity::new("replacement", "test", "model"),
        Vec::<ScriptedTurn>::new(),
    ));
    let replace_error = interactive
        .replace_provider(
            Arc::clone(&replacement),
            rho_sdk::ReasoningLevel::Low,
            "test-auth",
        )
        .unwrap_err();
    assert!(matches!(replace_error, rho_sdk::Error::SessionBusy));

    let rewind_error = interactive
        .restore_workspace_rewind(&crate::session::tree::NodeId::from_string("leaf-1").unwrap())
        .await
        .unwrap_err();
    assert!(
        rewind_error
            .to_string()
            .contains("while compaction is active"),
        "{rewind_error}"
    );

    let _ = interactive.abort_compact_task().await;
}

// Covers: remembered workspace writes stay bound to the session and grantor
// that produced them. Reset, resume, and Auto→Allow edits must not reuse them;
// tree navigation inside the same stored session may.
// Owner: interactive runtime permission lifecycle
#[tokio::test]
async fn remembered_writes_do_not_cross_session_or_grantor_boundaries() {
    let (_repo, created_write) = untracked_workspace_write();
    let require_approval = PolicyDecision::RequireApproval {
        reason: String::new(),
    };

    let mut interactive = pending_compaction_runtime("done").await;
    remember_live_write(&interactive, &created_write).await;
    assert_eq!(
        interactive.workspace_policy().evaluate(&created_write),
        PolicyDecision::Allow
    );

    interactive.reset().await.unwrap();
    assert_eq!(
        interactive.workspace_policy().evaluate(&created_write),
        require_approval.clone()
    );

    remember_live_write(&interactive, &created_write).await;
    interactive
        .set_permission_mode(PermissionMode::AllowEdits)
        .await
        .unwrap();
    assert_eq!(
        interactive.workspace_policy().evaluate(&created_write),
        require_approval.clone()
    );
    interactive
        .set_permission_mode(PermissionMode::Auto)
        .await
        .unwrap();
    assert_eq!(
        interactive.workspace_policy().evaluate(&created_write),
        PolicyDecision::Allow,
        "classifier grants must remain bound to Auto after a round-trip"
    );

    remember_live_write(&interactive, &created_write).await;
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("workspace");
    std::fs::create_dir(&cwd).unwrap();
    let target = StoredSession::create_in_root(root.path(), &cwd).unwrap();
    interactive.resume(target).await.unwrap();
    assert_eq!(
        interactive.workspace_policy().evaluate(&created_write),
        require_approval
    );
}

// Covers: navigating the conversation tree of the live stored session keeps
// grants from the current grantor instead of installing a detached log.
// Owner: interactive runtime permission lifecycle
#[tokio::test]
async fn tree_navigation_keeps_same_session_write_authority() {
    let (_repo, created_write) = untracked_workspace_write();
    let mut interactive = pending_compaction_runtime("done").await;
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("workspace");
    std::fs::create_dir(&cwd).unwrap();
    let (storage, root_id) = stored_session_with_branch(root.path(), &cwd);
    interactive.sessions.attach_storage(storage.clone());
    remember_live_write(&interactive, &created_write).await;

    interactive
        .select_tree_node(storage, &root_id)
        .await
        .unwrap();
    assert_eq!(
        interactive.workspace_policy().evaluate(&created_write),
        PolicyDecision::Allow
    );
}

async fn remember_live_write(
    interactive: &InteractiveRuntime,
    request: &rho_sdk::CapabilityRequest,
) {
    let authority = interactive
        .permission_mode
        .write_authority()
        .unwrap_or(WriteAuthority::Classifier);
    struct Allow;
    impl ApprovalHandler for Allow {
        fn request<'a>(&'a self, _request: ApprovalRequest) -> ApprovalFuture<'a> {
            Box::pin(async { ApprovalDecision::AllowOnce })
        }
    }
    let handler = crate::permission::remember_allowed_workspace_writes(
        Arc::new(Allow),
        interactive.session_writes.clone(),
        authority,
    );
    handler
        .request(ApprovalRequest::new(request.clone(), ""))
        .await;
}

fn untracked_workspace_write() -> (tempfile::TempDir, rho_sdk::CapabilityRequest) {
    let dir = tempfile::tempdir().unwrap();
    let status = std::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .status()
        .expect("git should be available");
    assert!(status.success());
    let path = dir.path().join("new.rs");
    std::fs::write(&path, "fn main() {}\n").unwrap();
    let request = rho_sdk::CapabilityRequest::write_path(
        path,
        rho_sdk::PathScope::PrimaryWorkspace,
        rho_sdk::CapabilitySource::built_in_tool("write"),
    );
    (dir, request)
}

fn stored_session_with_branch(
    session_root: &std::path::Path,
    cwd: &std::path::Path,
) -> (StoredSession, crate::session::tree::NodeId) {
    let storage = StoredSession::create_in_root(session_root, cwd).unwrap();
    let id = SessionId::from_string(storage.id()).unwrap();
    let first = rho_sdk::SessionSnapshot::new(
        id.clone(),
        rho_sdk::Revision::from_u64(1),
        vec![Message::user_text("root")],
        ModelIdentity::new("test", "test", "test"),
        rho_sdk::CompactionState::default(),
    )
    .with_prompt_cache_key(format!("rho:{}", storage.id()));
    storage.save_snapshot(&first, first.history()).unwrap();
    let root_id = storage
        .session_tree()
        .unwrap()
        .active_leaf_id()
        .unwrap()
        .clone();
    let leaf = rho_sdk::SessionSnapshot::new(
        id,
        rho_sdk::Revision::from_u64(2),
        vec![Message::user_text("root"), Message::assistant_text("leaf")],
        ModelIdentity::new("test", "test", "test"),
        rho_sdk::CompactionState::default(),
    )
    .with_prompt_cache_key(format!("rho:{}", storage.id()));
    storage.save_snapshot(&leaf, &leaf.history()[1..]).unwrap();
    (storage, root_id)
}

#[tokio::test]
async fn a_new_run_resets_the_context_usage_baseline() {
    let mut interactive = pending_compaction_runtime("done").await;
    interactive.context_window = Some(10_000);
    interactive.observe_event(&RunEvent::UsageUpdated {
        usage: ModelUsage {
            input_tokens: Some(50_000),
            ..ModelUsage::default()
        },
    });

    interactive.observe_event(&RunEvent::Started {
        run_id: RunId::new(),
        revision: Default::default(),
    });
    interactive.observe_event(&RunEvent::StepStarted { step: 1 });
    interactive.observe_event(&RunEvent::UsageUpdated {
        usage: ModelUsage {
            input_tokens: Some(300),
            cache_read_tokens: Some(700),
            ..ModelUsage::default()
        },
    });

    assert_eq!(
        interactive.take_context_usage(),
        Some(rho_sdk::model::ContextUsage::provider_reported(
            1_000,
            Some(10_000)
        ))
    );
}

// Covers: step-start estimates must surface before provider usage arrives
// Owner: interactive run controller context accounting
#[tokio::test]
async fn context_estimated_notes_estimated_context_before_provider_usage() {
    let mut interactive = pending_compaction_runtime("done").await;
    interactive.context_window = Some(10_000);

    interactive.observe_event(&RunEvent::Started {
        run_id: RunId::new(),
        revision: Default::default(),
    });
    interactive.observe_event(&RunEvent::StepStarted { step: 1 });
    interactive.observe_event(&RunEvent::ContextEstimated { tokens: 2_500 });

    assert_eq!(
        interactive.take_context_usage(),
        Some(rho_sdk::model::ContextUsage::estimated(2_500, Some(10_000)))
    );

    interactive.observe_event(&RunEvent::UsageUpdated {
        usage: ModelUsage {
            input_tokens: Some(2_400),
            cache_read_tokens: Some(100),
            ..ModelUsage::default()
        },
    });
    assert_eq!(
        interactive.take_context_usage(),
        Some(rho_sdk::model::ContextUsage::provider_reported(
            2_500,
            Some(10_000)
        ))
    );
}

#[tokio::test]
async fn finished_run_reports_context_from_committed_history() {
    let mut interactive = pending_compaction_runtime("assistant output").await;
    interactive.context_window = Some(10_000);

    interactive
        .start(UserInput::text("user input"), None)
        .await
        .unwrap();
    while interactive.next_event().await.is_some() {}
    interactive.finish_run().await.unwrap();

    let expected_tokens = rho_sdk::model::context::estimate_context_tokens(
        &interactive.history(),
        &interactive.tools.specs(),
    );
    assert_eq!(
        interactive.take_context_usage(),
        Some(rho_sdk::model::ContextUsage::estimated(
            expected_tokens,
            Some(10_000)
        ))
    );
}

#[tokio::test]
async fn handoff_compactability_requires_history_that_can_be_reduced() {
    let mut interactive = pending_compaction_runtime("done").await;
    interactive.context_window = Some(100);
    interactive.compaction.target_percent = 50;

    assert!(!interactive.can_compact());

    for index in 0..2 {
        interactive
            .sessions
            .session()
            .append_message(Message::user_text(format!(
                "turn {index}: {}",
                "context ".repeat(80)
            )))
            .unwrap();
        interactive
            .sessions
            .session()
            .append_message(Message::assistant_text(format!(
                "answer {index}: {}",
                "detail ".repeat(80)
            )))
            .unwrap();
    }

    assert!(interactive.can_compact());
}

#[tokio::test]
async fn dropping_manual_compaction_does_not_leave_the_runtime_busy() {
    let mut interactive = pending_compaction_runtime("done").await;

    let mut compact = Box::pin(interactive.compact());
    tokio::select! {
        result = &mut compact => panic!("compaction unexpectedly completed: {result:?}"),
        () = tokio::task::yield_now() => {}
    }
    drop(compact);

    interactive
        .start(UserInput::text("continue"), None)
        .await
        .unwrap();
}

#[tokio::test]
async fn failed_turn_does_not_duplicate_the_previous_assistant_in_display_history() {
    let mut interactive = test_runtime(vec![
        ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
            "previous answer".into(),
        )])),
        ScriptedTurn::failed(ProviderError::new(
            ProviderErrorKind::Unavailable,
            "provider unavailable",
            Retryability::Permanent,
        )),
    ])
    .await;
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("workspace");
    std::fs::create_dir(&cwd).unwrap();
    let storage = StoredSession::create_in_root(root.path(), &cwd).unwrap();
    let sdk_session = interactive
        .runtime
        .session(SessionOptions::new().id(SessionId::from_string(storage.id()).unwrap()))
        .await
        .unwrap();
    interactive.sessions.replace_session(sdk_session, None);
    interactive.sessions.attach_storage(storage.clone());

    interactive
        .start(UserInput::text("successful prompt"), None)
        .await
        .unwrap();
    while interactive.next_event().await.is_some() {}
    interactive.finish_run().await.unwrap();

    interactive
        .start(UserInput::text("failed prompt"), None)
        .await
        .unwrap();
    while interactive.next_event().await.is_some() {}
    assert!(interactive.finish_run().await.is_err());
    let committed_assistant = interactive.history()[1].clone();

    let (_, histories) =
        StoredSession::open_by_id_with_histories_in_root(root.path(), &cwd, storage.id()).unwrap();
    assert_eq!(
        histories.display,
        vec![
            Message::user_text("successful prompt"),
            committed_assistant,
            Message::user_text("failed prompt"),
        ]
    );
}

#[tokio::test]
async fn failed_resume_preserves_the_current_runtime() {
    let mut interactive = pending_compaction_runtime("still available").await;
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("workspace");
    std::fs::create_dir(&cwd).unwrap();
    let target = StoredSession::create_in_root(root.path(), &cwd).unwrap();
    std::fs::write(
        target.path(),
        format!(
            "{}\n",
            serde_json::json!({
                "type": "session",
                "version": 999,
                "id": target.id(),
                "timestamp": "1",
                "cwd": cwd,
            })
        ),
    )
    .unwrap();

    assert!(interactive.resume(target).await.is_err());
    interactive
        .start(UserInput::text("continue"), None)
        .await
        .unwrap();
}

async fn advisor_test_runtime() -> InteractiveRuntime {
    let mut interactive = test_runtime(Vec::new()).await;
    let config = Config::default();
    interactive.tools = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::new(AgentCapabilities::new(
            [ToolCapability::Advisor].into_iter().collect(),
        ))
        .advisor(crate::tools::advisor::AdvisorSessionStore::new()),
    );
    interactive
}

fn advisor_model() -> crate::config::InternalAgentModelConfig {
    crate::config::InternalAgentModelConfig::new(
        "anthropic".into(),
        "claude-fable-5".into(),
        "api-key".into(),
    )
}

// Covers: toggling advisor mode mid-session must add and remove the advisor tool
// for the next turn without rewriting the system prompt, while the session ID
// survives and a model-facing schema notice is appended.
// Owner: interactive runtime advisor state transition.
#[tokio::test]
async fn advisor_mode_changes_the_tool_list_without_replacing_the_session() {
    let mut interactive = advisor_test_runtime().await;
    let session_id = interactive.sessions.session().id().clone();
    let history_before = interactive.history().len();
    let system_before = interactive.system_prompt.clone();
    let advertised = |interactive: &InteractiveRuntime| {
        interactive
            .runtime
            .diagnostics()
            .tools()
            .iter()
            .any(|tool| tool.name() == "advisor")
    };

    assert!(!interactive.tools.advisor_registered());

    let enabled = interactive
        .set_advisor(Some(advisor_model()))
        .await
        .unwrap();
    assert_eq!(enabled.as_deref(), Some("advisor mode on"));
    assert!(interactive.tools.advisor_registered());
    assert!(advertised(&interactive));
    assert_eq!(interactive.sessions.session().id(), &session_id);
    assert_eq!(interactive.system_prompt, system_before);
    assert_eq!(interactive.history().len(), history_before + 1);
    let enabled_notice = interactive.history().last().expect("enable notice").clone();
    let Message::User(blocks) = &enabled_notice else {
        panic!("expected user notice, got {enabled_notice:?}");
    };
    let enabled_text = blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert!(enabled_text.contains("[advisor mode on]"));
    assert!(enabled_text.contains("input_schema:"));
    assert!(enabled_text.contains("`advisor`"));

    let disabled = interactive.set_advisor(None).await.unwrap();
    assert_eq!(disabled.as_deref(), Some("advisor mode off"));
    assert!(!interactive.tools.advisor_registered());
    assert!(!advertised(&interactive));
    assert_eq!(interactive.sessions.session().id(), &session_id);
    assert_eq!(interactive.system_prompt, system_before);
    assert_eq!(interactive.history().len(), history_before + 2);
    let disabled_notice = interactive
        .history()
        .last()
        .expect("disable notice")
        .clone();
    let Message::User(blocks) = &disabled_notice else {
        panic!("expected user notice, got {disabled_notice:?}");
    };
    let disabled_text = blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert!(disabled_text.contains("[advisor mode off]"));
    assert!(disabled_text.contains("no longer available"));
}

// Covers: append-success / snapshot-save-failure after a successful rebuild must
// restore previous advisor registration, model, and model-visible history.
// Owner: interactive runtime advisor state transition.
#[tokio::test]
async fn advisor_notice_failure_restores_previous_registration_and_model() {
    let mut interactive = advisor_test_runtime().await;
    let store = interactive.tools.advisor().cloned().expect("advisor store");
    assert!(!interactive.tools.advisor_registered());
    assert!(store.model().is_none());
    let history_before = interactive.history();

    super::advisor::fail_next_advisor_switch_notice_for_tests();
    let error = interactive
        .set_advisor(Some(advisor_model()))
        .await
        .expect_err("notice failure should abort the transition");
    assert!(
        error
            .to_string()
            .contains("injected advisor switch notice snapshot save failure"),
        "unexpected error: {error}"
    );
    assert!(
        !interactive.tools.advisor_registered(),
        "registration must roll back"
    );
    assert!(store.model().is_none(), "model must roll back");
    assert_eq!(
        interactive.history(),
        history_before,
        "model-visible history must not keep a notice that never persisted"
    );
    assert!(
        !interactive
            .runtime
            .diagnostics()
            .tools()
            .iter()
            .any(|tool| tool.name() == "advisor"),
        "runtime must not advertise advisor after rollback"
    );
}

// Covers: advisor mode cannot change while a provider run is active.
// Owner: interactive runtime advisor state transition.
#[tokio::test]
async fn advisor_mode_rejects_change_while_a_run_is_active() {
    let mut interactive = pending_compaction_runtime("still going").await;
    let config = Config::default();
    interactive.tools = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::new(AgentCapabilities::new(
            [ToolCapability::Advisor].into_iter().collect(),
        ))
        .advisor(crate::tools::advisor::AdvisorSessionStore::new()),
    );
    interactive
        .start(UserInput::text("keep running"), None)
        .await
        .unwrap();
    assert!(interactive.is_run_active());

    let error = interactive
        .set_advisor(Some(advisor_model()))
        .await
        .expect_err("active run must block advisor transitions");
    assert!(
        error
            .to_string()
            .contains("cannot change while a run is active"),
        "unexpected error: {error}"
    );
    assert!(!interactive.tools.advisor_registered());
    interactive.shutdown().await;
}

async fn edit_tool_test_runtime() -> InteractiveRuntime {
    edit_tool_runtime(crate::config::EditTool::Pinned(
        rho_tools::EditFormat::Hashline,
    ))
    .await
}

/// Shared factory for TUI tests that exercise Auto edit-tool handoff.
pub(super) async fn edit_tool_runtime(edit_tool: crate::config::EditTool) -> InteractiveRuntime {
    let mut interactive = pending_compaction_runtime("done").await;
    let config = Config {
        edit_tool,
        ..Config::default()
    };
    interactive.tools = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::new(AgentCapabilities::new(
            [ToolCapability::Edit, ToolCapability::ReadFile]
                .into_iter()
                .collect(),
        )),
    );
    interactive
}

// Covers: /config edit-tool selection must swap the advertised edit surface for
// the next turn without rewriting the system prompt, while the session ID
// survives and a model-facing schema notice is appended.
// Owner: interactive runtime edit-tool state transition.
#[tokio::test]
async fn edit_tool_switch_rebuilds_tools_and_appends_schema_notice() {
    let mut interactive = edit_tool_test_runtime().await;
    let session_id = interactive.sessions.session().id().clone();
    let history_before = interactive.history().len();
    let system_before = interactive.system_prompt.clone();
    let advertised = |interactive: &InteractiveRuntime, name: &str| {
        interactive
            .runtime
            .diagnostics()
            .tools()
            .iter()
            .any(|tool| tool.name() == name)
    };

    assert!(interactive.tools.contains("edit"));
    assert!(!interactive.tools.contains("str_replace"));

    let change = interactive
        .set_edit_tool(
            rho_tools::EditFormat::StrReplace,
            Config::default().max_output_bytes,
        )
        .await
        .unwrap()
        .expect("edit tool should change");
    assert_eq!(change.previous, rho_tools::EditFormat::Hashline);
    assert_eq!(change.display, "edit tool switched to str_replace");
    assert_eq!(interactive.system_prompt, system_before);
    assert!(interactive.tools.contains("str_replace"));
    assert!(!interactive.tools.contains("edit"));
    assert!(advertised(&interactive, "str_replace"));
    assert!(!advertised(&interactive, "edit"));
    assert_eq!(interactive.sessions.session().id(), &session_id);
    assert_eq!(interactive.history().len(), history_before + 1);
    let notice = interactive.history().last().expect("switch notice").clone();
    let Message::User(blocks) = &notice else {
        panic!("expected user notice, got {notice:?}");
    };
    let text = blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert!(text.contains("[edit tool switched]"));
    assert!(text.contains("`str_replace`"));
    assert!(text.contains("input_schema:"));
    assert!(!text.contains("restart"));
}

// Covers: the enable notice names the reviewer, and swapping the advisor model
// while advisor mode stays on still tells the executor. That swap changes no
// tool list, so nothing else in the session would report it.
// Owner: interactive runtime advisor state transition.
#[tokio::test]
async fn advisor_notices_name_the_reviewer_model_including_a_model_only_change() {
    fn last_notice_text(interactive: &InteractiveRuntime) -> String {
        let last = interactive.history().last().expect("notice").clone();
        let Message::User(blocks) = &last else {
            panic!("expected user notice, got {last:?}");
        };
        blocks
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    let mut interactive = advisor_test_runtime().await;

    interactive
        .set_advisor(Some(advisor_model()))
        .await
        .unwrap();
    assert!(last_notice_text(&interactive).contains("anthropic/claude-fable-5"));

    let history_after_enable = interactive.history().len();
    let switched = interactive
        .set_advisor(Some(crate::config::InternalAgentModelConfig::new(
            "openai".into(),
            "gpt-5.6-sol".into(),
            "api-key".into(),
        )))
        .await
        .unwrap();

    assert_eq!(
        switched
            .as_deref()
            .map(|text| text.starts_with("advisor model switched to openai/gpt-5.6-sol")),
        Some(true),
        "{switched:?}"
    );
    assert!(interactive.tools.advisor_registered());
    assert_eq!(interactive.history().len(), history_after_enable + 1);
    let notice = last_notice_text(&interactive);
    assert_eq!(notice.lines().count(), 1, "{notice:?}");
    assert!(notice.contains("openai/gpt-5.6-sol"), "{notice}");

    // The notice reports the model, so only the model decides whether there was
    // a switch. Changing the reasoning level alone must add no notice.
    let mut same_model_new_reasoning = crate::config::InternalAgentModelConfig::new(
        "openai".into(),
        "gpt-5.6-sol".into(),
        "api-key".into(),
    );
    same_model_new_reasoning.reasoning = Some(rho_providers::reasoning::ReasoningLevel::High);
    let unchanged = interactive
        .set_advisor(Some(same_model_new_reasoning))
        .await
        .unwrap();
    assert_eq!(unchanged, None);
    assert_eq!(interactive.history().len(), history_after_enable + 1);
}
