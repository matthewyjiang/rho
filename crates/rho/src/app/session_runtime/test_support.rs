//! Runtimes for tests: a plain root agent with every tool, on scripted
//! sessions that need no credentials.

use std::{path::Path, sync::Arc};

use rho_sdk::{
    model::{ModelIdentity, ModelRequest},
    provider::{ModelProvider, ProviderFuture, ScriptedProvider},
    SessionOptions,
};

use super::{
    ApprovalPolicy, ChildAgents, Logouts, OpenedSession, QuestionSupport, SessionBuild,
    SessionPermission, SessionRuntime, SessionStorage, UsageAttribution,
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
    tools::{mcp::McpAuthorizationMode, sdk_registry::AppToolSet},
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

/// A provider on `config`'s model with no scripted turns.
fn idle_provider(config: &Config) -> Arc<dyn ModelProvider> {
    Arc::new(ScriptedProvider::new(
        ModelIdentity::new(&config.provider, "scripted", &config.model),
        Vec::new(),
    ))
}

/// A provider whose every request waits until its run is cancelled, so a
/// turn on it can only end cancelled.
pub(super) struct HangUntilCancel;

impl ModelProvider for HangUntilCancel {
    fn identity(&self) -> ModelIdentity {
        ModelIdentity::new("test", "test", "model")
    }

    fn send_turn<'a>(&'a self, request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async move {
            request.cancellation.cancelled().await;
            Err(rho_sdk::ProviderError::interrupted("cancelled"))
        })
    }
}

/// A tool-less session `id` on `config`'s model, whose provider has no
/// scripted turns.
pub(super) async fn idle_built(config: &Config, id: rho_sdk::SessionId) -> BuiltSession {
    built_on(config, id, idle_provider(config)).await
}

/// A tool-less session `id` on `provider`.
pub(super) async fn built_on(
    config: &Config,
    id: rho_sdk::SessionId,
    provider: Arc<dyn ModelProvider>,
) -> BuiltSession {
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
    runtime_on(root, config, idle_provider(config)).await
}

/// A runtime around a [`built_on`] session on `provider`, saved in a new
/// file under `root`, which is also its workspace.
pub(super) async fn runtime_on(
    root: &Path,
    config: &Config,
    provider: Arc<dyn ModelProvider>,
) -> SessionRuntime {
    let stored = StoredSession::create_in_root(root, root).expect("test session file");
    let id = rho_sdk::SessionId::from_string(stored.id()).expect("stored ids are session ids");
    let built = built_on(config, id, provider).await;
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
                build: SessionBuild {
                    config_path: workspace_root.join("rho-test-config.toml"),
                    no_system_prompt: false,
                    no_tools: false,
                    no_subagents: false,
                    diagnostics: RuntimeDiagnostics::new(config),
                    questions: QuestionSupport::Unavailable,
                    children: ChildAgents::InProcess,
                    background_wait: BackgroundWait::HoldPrompt,
                    launch_agent: agent.clone(),
                    host_io: Default::default(),
                    max_steps: None,
                    usage: UsageAttribution::Root,
                    hook_labels: Default::default(),
                    prompt_suffix: None,
                    mcp_authorization: McpAuthorizationMode::NonInteractive,
                    approvals: ApprovalPolicy::Host,
                },
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

    /// The session's whole tool set, for host tests that check what a
    /// rebuild registered.
    pub(in crate::app) fn tool_set_for_tests(&self) -> &AppToolSet {
        &self.built.tools
    }

    /// The session's live history, for host tests that compare it with what
    /// was saved.
    pub(in crate::app) fn history_for_tests(&self) -> Vec<rho_sdk::model::Message> {
        self.built.session.history()
    }
}
