//! Runtimes for tests: a plain root agent with every tool, on scripted
//! sessions that need no credentials.

use std::{path::Path, sync::Arc};

use rho_sdk::{
    model::{ModelIdentity, ModelRequest},
    provider::{ModelProvider, ProviderFuture, ScriptedProvider},
    SessionOptions,
};

use super::{
    credentials::SessionCredentials, ApprovalPolicy, ChildAgents, HostCapabilities, LeftOut,
    OpenedSession, QuestionSupport, RunShape, SessionBuild, SessionLaunch, SessionPermission,
    SessionRole, SessionRuntime, SessionStorage, UnattendedApprovals,
};
use crate::{
    agent::{
        AgentCapabilities, AgentDefinition, AgentId, AgentRuntimeSpec, ModelPolicy, PromptPolicy,
        ToolPolicy,
    },
    app::{
        agent_binding::{AgentBinder, AgentInvocation, AgentRole, BoundAgent},
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
pub(in crate::app) struct HangUntilCancel;

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
    built_with(config, id, runtime, provider).await
}

/// A session `id` on the SDK `runtime`, built on `provider` with whatever
/// tools and hooks the test gave it, and no host tools of its own.
pub(super) async fn built_with(
    config: &Config,
    id: rho_sdk::SessionId,
    runtime: rho_sdk::Rho,
    provider: Arc<dyn ModelProvider>,
) -> BuiltSession {
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

/// A runtime around a [`built_with`] session on the SDK `runtime`, saved in
/// a new file under `root`, which is also its workspace, for a host nobody
/// watches that offers `questions`.
pub(in crate::app) async fn unattended_runtime_on(
    root: &Path,
    config: &Config,
    questions: QuestionSupport,
    runtime: rho_sdk::Rho,
    provider: Arc<dyn ModelProvider>,
) -> SessionRuntime {
    let stored = StoredSession::create_in_root(root, root).expect("test session file");
    let id = rho_sdk::SessionId::from_string(stored.id()).expect("stored ids are session ids");
    let built = built_with(config, id, runtime, provider).await;
    let mut build = test_build(config, root);
    build.host = HostCapabilities {
        questions,
        ..HostCapabilities::unattended(UnattendedApprovals::Alone)
    };
    SessionRuntime::for_tests_with(build, built, SessionStorage::saved(stored), config)
}

/// A runtime around a [`built_with`] session on the SDK `runtime`, whose
/// file under `root`, also its workspace, is created on its first save, as
/// a new daemon session's is, for a host nobody watches.
pub(in crate::app) async fn unsaved_unattended_runtime_on(
    root: &Path,
    config: &Config,
    runtime: rho_sdk::Rho,
    provider: Arc<dyn ModelProvider>,
) -> SessionRuntime {
    let id = rho_sdk::SessionId::new();
    let built = built_with(config, id.clone(), runtime, provider).await;
    let mut build = test_build(config, root);
    build.host = HostCapabilities::unattended(UnattendedApprovals::Alone);
    let (file_root, id) = (root.to_path_buf(), id.to_string());
    let storage = SessionStorage::unsaved(
        root.to_path_buf(),
        Box::new(move || {
            StoredSession::create_with_id_in_root_for_test(&file_root, &file_root, &id)
        }),
    );
    SessionRuntime::for_tests_with(build, built, storage, config)
}

/// How a test session is built: as a [`root_agent`] whose host answers its
/// approvals, configured from `workspace_root`.
pub(in crate::app) fn test_build(config: &Config, workspace_root: &Path) -> SessionBuild {
    SessionBuild {
        launch: SessionLaunch {
            config_path: workspace_root.join("rho-test-config.toml"),
            left_out: LeftOut::default(),
            diagnostics: RuntimeDiagnostics::new(config),
            agent: root_agent(config),
            prompt_suffix: None,
        },
        host: HostCapabilities {
            questions: QuestionSupport::Unavailable,
            children: ChildAgents::InProcess,
            subagent_pool: crate::app::subagent_pool::SubagentPool::Own,
            environment: Default::default(),
            mcp_authorization: McpAuthorizationMode::NonInteractive,
            approvals: ApprovalPolicy::Host,
            parent_notices: None,
            session_use: super::SessionUse::Run,
        },
        run: RunShape {
            role: SessionRole::Daemon,
            max_steps: None,
            hook_labels: Default::default(),
        },
    }
}

impl SessionRuntime {
    /// A runtime around `built`, saved through `storage`, made as
    /// [`test_build`] says with the storage's workspace.
    pub(super) fn for_tests(built: BuiltSession, storage: SessionStorage, config: &Config) -> Self {
        let build = test_build(config, storage.cwd());
        Self::for_tests_with(build, built, storage, config)
    }

    /// A runtime around `built`, saved through `storage`, made as `build`
    /// says from the start, in `config`'s mode with the storage's
    /// workspace.
    pub(super) fn for_tests_with(
        build: SessionBuild,
        built: BuiltSession,
        storage: SessionStorage,
        config: &Config,
    ) -> Self {
        let workspace_root = storage.cwd().to_path_buf();
        Self::from_built_for_tests(
            build,
            built,
            storage,
            SessionPermission {
                mode: config.permission_mode,
                writes: Default::default(),
                workspace_root,
            },
            config.auth.clone(),
        )
    }

    /// A runtime around `built`, saved through `storage`, running as
    /// `build`'s launch agent with `permission` and `auth`, for host
    /// fixtures that script their own session. It has none of a host's MCP
    /// servers or roots.
    pub(in crate::app) fn from_built_for_tests(
        build: SessionBuild,
        built: BuiltSession,
        storage: SessionStorage,
        permission: SessionPermission,
        auth: String,
    ) -> Self {
        let agent = build.launch.agent.clone();
        Self::new(OpenedSession {
            build,
            built,
            storage,
            agent,
            auth,
            host_mcp: Default::default(),
            added_dirs: Default::default(),
            permission,
            reopen_notice: None,
            credentials: SessionCredentials::Scripted,
        })
    }

    /// The session's live history, for host tests that compare it with what
    /// was saved.
    pub(in crate::app) fn history_for_tests(&self) -> Vec<rho_sdk::model::Message> {
        self.built.session.history()
    }
}
