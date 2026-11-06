//! Runtimes for tests: a plain root agent with every tool, on scripted
//! sessions that need no credentials.

use std::{path::Path, sync::Arc};

use rho_sdk::{
    model::ModelIdentity,
    provider::{ModelProvider, ScriptedProvider},
    SessionOptions,
};

use super::{
    ChildAgents, Logouts, OpenedSession, QuestionSupport, SessionBuild, SessionPermission,
    SessionRuntime, SessionStorage,
};
use crate::{
    agent::{
        AgentCapabilities, AgentDefinition, AgentId, AgentRuntimeSpec, ModelPolicy, PromptPolicy,
        ToolPolicy,
    },
    app::{
        agent_binding::{AgentBinder, AgentInvocation, AgentRole, BoundAgent},
        headless_delegation::BackgroundWait,
        session_assembly::BuiltSession,
    },
    config::Config,
    diagnostics::RuntimeDiagnostics,
    session::Session as StoredSession,
    tools::sdk_registry::AppToolSet,
};

/// A plain root agent, `rho`, with every host tool, bound on `config`.
pub(super) fn root_agent(config: &Config) -> BoundAgent {
    AgentBinder::bind(
        Arc::new(AgentDefinition {
            id: AgentId::new("rho").expect("test agent id"),
            description: "test".into(),
            prompt: PromptPolicy::Extend(String::new()),
            runtime: AgentRuntimeSpec::Rho {
                tools: ToolPolicy::All,
                model: ModelPolicy::Inherit,
                reasoning: None,
            },
        }),
        AgentInvocation {
            role: AgentRole::AutomationRoot,
            available_tools: AgentCapabilities::all_host_tools(),
        },
        config,
    )
    .expect("bind test agent")
}

/// A tool-less session `id` on `config`'s model, whose provider has no
/// scripted turns.
pub(super) async fn idle_built(config: &Config, id: rho_sdk::SessionId) -> BuiltSession {
    let provider: Arc<dyn ModelProvider> = Arc::new(ScriptedProvider::new(
        ModelIdentity::new(&config.provider, "scripted", &config.model),
        Vec::new(),
    ));
    let runtime = rho_sdk::Rho::builder()
        .provider_shared(Arc::clone(&provider))
        .build()
        .expect("test runtime");
    let session = runtime
        .session(SessionOptions::new().id(id))
        .await
        .expect("test session");
    BuiltSession {
        runtime,
        session,
        provider,
        tools: AppToolSet::disabled(),
        hooks: None,
        approval_receiver: None,
        prompt_template: None,
        prompt: Default::default(),
        diagnostics: RuntimeDiagnostics::new(config),
    }
}

/// A runtime around an [`idle_built`] session, saved in a new file under
/// `root`, which is also its workspace.
pub(super) async fn idle_runtime(root: &Path, config: &Config) -> SessionRuntime {
    let stored = StoredSession::create_in_root(root, root).expect("test session file");
    let id = rho_sdk::SessionId::from_string(stored.id()).expect("stored ids are session ids");
    let built = idle_built(config, id).await;
    SessionRuntime::for_tests(built, SessionStorage::saved(stored), config)
}

impl SessionRuntime {
    /// A runtime around `built`, saved through `storage`, for a
    /// [`root_agent`] whose workspace is the storage's.
    pub(super) fn for_tests(built: BuiltSession, storage: SessionStorage, config: &Config) -> Self {
        let agent = root_agent(config);
        let workspace_root = storage.cwd().to_path_buf();
        Self::new(
            OpenedSession {
                built,
                storage,
                agent: agent.clone(),
                auth: config.auth.clone(),
                host_mcp: Default::default(),
                added_dirs: Default::default(),
                permission: SessionPermission {
                    mode: config.permission_mode,
                    writes: Default::default(),
                    workspace_root: workspace_root.clone(),
                },
                reopen_notice: None,
            },
            SessionBuild {
                config_path: workspace_root.join("rho-test-config.toml"),
                no_system_prompt: false,
                no_tools: false,
                no_subagents: false,
                diagnostics: RuntimeDiagnostics::new(config),
                questions: QuestionSupport::Unavailable,
                children: ChildAgents::InProcess,
                background_wait: BackgroundWait::HoldPrompt,
                launch_agent: agent,
                host_io: Default::default(),
            },
            &Logouts::default(),
        )
    }

    /// The session's workspace and file, for host tests that read what the
    /// runtime saved.
    pub(in crate::app) fn storage(&self) -> &SessionStorage {
        &self.storage
    }

    /// The agent the session runs as.
    pub(in crate::app) fn agent(&self) -> &BoundAgent {
        &self.agent
    }
}
