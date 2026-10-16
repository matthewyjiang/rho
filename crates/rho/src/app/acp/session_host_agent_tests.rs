use std::{collections::BTreeSet, future::Future, path::Path, pin::Pin, sync::Arc};

use agent_client_protocol::{
    schema::v1::{
        RequestPermissionRequest, RequestPermissionResponse, SessionId, SessionNotification,
        StopReason,
    },
    Error as AcpError,
};
use pretty_assertions::assert_eq;
use rho_providers::reasoning::ReasoningLevel;
use rho_sdk::{
    model::{ContentBlock, Message, ModelIdentity, ModelResponse},
    provider::{ModelProvider, ScriptedProvider, ScriptedTurn},
    SessionOptions, UserInput,
};

use super::*;
use crate::{
    agent::{AgentCapabilities, AgentId, ModelPolicy, PromptPolicy, ToolCapability, ToolPolicy},
    app::{acp::session_host::mode::SessionPermission, session_assembly::BuiltSession},
    diagnostics::RuntimeDiagnostics,
    herdr::HerdrReporter,
    permission::PermissionMode,
    session::Session as StoredSession,
    tools::sdk_registry::{AppToolSet, ToolSetOptions},
};

struct NullPort;

impl AcpClientPort for NullPort {
    fn send_session_notification(
        &self,
        _notification: SessionNotification,
    ) -> Pin<Box<dyn Future<Output = Result<(), AcpError>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }

    fn request_permission(
        &self,
        _request: RequestPermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RequestPermissionResponse, AcpError>> + Send + '_>>
    {
        Box::pin(async { Err(AcpError::method_not_found()) })
    }
}

fn definition(tools: ToolPolicy) -> AgentDefinition {
    AgentDefinition {
        id: AgentId::new("acp-child").unwrap(),
        description: "ACP child".into(),
        prompt: PromptPolicy::Extend("Inspect the requested files".into()),
        runtime: AgentRuntimeSpec::Rho {
            tools,
            model: ModelPolicy::Inherit,
            reasoning: None,
        },
    }
}

fn startup(cwd: &Path) -> AcpStartup {
    let config = Config {
        provider: "test".into(),
        model: "launch-model".into(),
        permission_mode: PermissionMode::Bypass,
        ..Config::default()
    };
    let agent = AgentBinder::bind(
        Arc::new(definition(ToolPolicy::All)),
        AgentInvocation {
            role: AgentRole::AutomationRoot,
            available_tools: AgentCapabilities::all_host_tools(),
        },
        &config,
    )
    .unwrap();
    AcpStartup {
        host: crate::app::acp::AcpHost::Generic,
        diagnostics: RuntimeDiagnostics::new(&config),
        config,
        config_path: cwd.join("config.toml"),
        cwd: cwd.to_path_buf(),
        no_system_prompt: false,
        no_tools: false,
        no_subagents: false,
        agent,
        herdr: HerdrReporter::default(),
        added_dirs: Default::default(),
    }
}

async fn scripted_built(
    config: &Config,
    id: rho_sdk::SessionId,
    tools: AppToolSet,
) -> BuiltSession {
    let provider: Arc<dyn ModelProvider> = Arc::new(ScriptedProvider::new(
        ModelIdentity::new("test", &config.provider, &config.model),
        vec![ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("done".into()),
        ]))],
    ));
    let mut builder = rho_sdk::Rho::builder()
        .provider_shared(Arc::clone(&provider))
        .system_prompt(rho_sdk::SystemPrompt::Custom("test instructions".into()));
    for tool in tools.tools() {
        builder = builder.tool_shared(Arc::clone(tool));
    }
    let runtime = builder.build().unwrap();
    let session = runtime.session(SessionOptions::new().id(id)).await.unwrap();
    BuiltSession {
        runtime,
        session,
        provider,
        tools,
        hooks: None,
        approval_receiver: None,
        prompt_template: None,
        prompt: Default::default(),
        diagnostics: RuntimeDiagnostics::new(config),
    }
}

async fn host(cwd: &Path, startup: &AcpStartup) -> SessionHost {
    let stored = StoredSession::create_in_root(cwd, cwd).unwrap();
    let config = Config {
        model: "host-selected-model".into(),
        ..startup.config.clone()
    };
    let built = scripted_built(
        &config,
        rho_sdk::SessionId::from_string(stored.id()).unwrap(),
        AppToolSet::disabled(),
    )
    .await;
    SessionHost::from_built(
        SessionId::new(stored.id()),
        built,
        stored,
        Default::default(),
        SessionPermission {
            mode: PermissionMode::Supervised,
            writes: Default::default(),
            workspace_root: cwd.to_path_buf(),
        },
        startup,
        Vec::new(),
    )
}

// Covers: a child must bind its allowlist and delegated role before its first
// turn, preserving the host's pre-prompt model/mode selection and session id.
// Owner: ACP session rebind transaction (scripted provider, no credentials).
#[tokio::test]
async fn rebind_applies_child_tools_and_current_host_selection() {
    let cwd = tempfile::tempdir().unwrap();
    let startup = startup(cwd.path());
    let mut host = host(cwd.path(), &startup).await;
    let id = host.built.session.id().clone();
    let child = definition(ToolPolicy::Allow(
        [
            ToolCapability::ReadFile,
            ToolCapability::Agent,
            ToolCapability::Agents,
            ToolCapability::Advisor,
        ]
        .into(),
    ));
    // T3 picks a thought level before the child's first prompt; the rebind
    // must keep it because the definition sets none.
    host.built
        .session
        .set_reasoning_level(ReasoningLevel::High)
        .unwrap();
    let display = UserInput::text("/agent:acp-child inspect the parser");
    let response = host
        .run_as_agent_with(
            &display,
            child,
            UserInput::text("inspect the parser"),
            &startup,
            &NullPort,
            async |agent, config, id| {
                assert_eq!(config.reasoning, ReasoningLevel::High);
                let tools = AppToolSet::new(
                    &config,
                    RuntimeDiagnostics::new(&config),
                    ToolSetOptions::new(agent.rho_capabilities().unwrap().clone()),
                );
                Ok(SessionAssembly {
                    built: scripted_built(&config, id, tools).await,
                    workspace_root: cwd.path().to_path_buf(),
                    session_writes: Default::default(),
                })
            },
        )
        .await
        .unwrap();
    let inventory: BTreeSet<_> = host
        .built
        .tools
        .tools()
        .iter()
        .map(|tool| tool.spec().name)
        .collect();
    assert_eq!(
        inventory,
        BTreeSet::from(["codemode".into(), "read_file".into(), "tool_search".into()])
    );
    assert_eq!(response.stop_reason, StopReason::EndTurn);
    assert_eq!(host.built.session.id(), &id);
    assert_eq!(host.current_model().model, "host-selected-model");
    assert_eq!(host.permission.mode, PermissionMode::Supervised);
    let child_identity = host.stored.stored_agent_identity().unwrap().unwrap();
    assert_eq!(
        child_identity,
        ("acp-child".into(), host.agent.fingerprint().to_string())
    );
    host.shutdown().await;
}

// Covers: rebinding must not silently discard a conversation, including a
// loaded conversation whose completed-runs counter starts at zero.
// Owner: ACP session rebind transaction.
#[tokio::test]
async fn rebind_rejects_nonfresh_sessions_without_replacing_them() {
    let cwd = tempfile::tempdir().unwrap();
    let startup = startup(cwd.path());
    for (completed_runs, history) in [
        (0, vec![Message::user_text("retained conversation")]),
        (1, Vec::new()),
    ] {
        let mut host = host(cwd.path(), &startup).await;
        host.completed_runs = completed_runs;
        for message in history {
            host.built.session.append_message(message).unwrap();
        }
        let before = host.built.session.snapshot();
        let input = UserInput::text("task");
        let error = host
            .run_as_agent_with(
                &input,
                definition(ToolPolicy::All),
                input.clone(),
                &startup,
                &NullPort,
                |_, _, _| async { panic!("nonfresh sessions must not rebuild") },
            )
            .await
            .unwrap_err();
        assert!(error.downcast_ref::<CommandRejected>().is_some());
        assert_eq!(host.built.session.snapshot(), before);
        host.shutdown().await;
    }
}

// Covers: a reloaded child must come back with its delegated tool boundary,
// never the root's, and must not load against a changed definition.
// Owner: ACP session load agent selection.
#[test]
fn load_restores_the_child_binding_or_fails() {
    let cwd = tempfile::tempdir().unwrap();
    let agents = cwd.path().join(".agents/agents");
    std::fs::create_dir_all(&agents).unwrap();
    let file = agents.join("acp-loaded.md");
    std::fs::write(
        &file,
        "---\ndescription: Loaded child\ntools: [read_file, agent]\n---\nInspect files\n",
    )
    .unwrap();
    let startup = startup(cwd.path());
    let catalog = || {
        Ok(AgentCatalog::discover_with_home(
            cwd.path(),
            /*home*/ None,
        )?)
    };
    let definitions = catalog().unwrap();
    let definition = &definitions.find("acp-loaded").unwrap().definition;
    let stored = StoredSession::create_in_root_with_agent(
        cwd.path(),
        cwd.path(),
        "acp-loaded",
        &definition.fingerprint().to_string(),
    )
    .unwrap();

    let (agent, _) = loaded_agent(&startup, &stored, startup.config.clone(), catalog).unwrap();
    assert_eq!(agent.id().as_str(), "acp-loaded");
    assert_eq!(
        agent.rho_capabilities(),
        Some(&AgentCapabilities::new([ToolCapability::ReadFile].into()))
    );

    let root = StoredSession::create_in_root_with_agent(
        cwd.path(),
        cwd.path(),
        startup.agent.id().as_str(),
        &startup.agent.fingerprint().to_string(),
    )
    .unwrap();
    let (agent, _) = loaded_agent(&startup, &root, startup.config.clone(), catalog).unwrap();
    assert_eq!(agent.fingerprint(), startup.agent.fingerprint());

    std::fs::write(
        &file,
        "---\ndescription: Loaded child\ntools: [read_file, write_file]\n---\nInspect files\n",
    )
    .unwrap();
    assert!(loaded_agent(&startup, &stored, startup.config.clone(), catalog).is_err());
}
