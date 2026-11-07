//! Opening a session: a new one, or one saved earlier, which the daemon
//! reopens as the agent it was saved as (#1661). The config is the user's
//! global config, read when the session is opened, so a config edit applies
//! to the next session opened without a daemon restart; the agents and
//! instructions are those of the session's workspace, as `rho` launched
//! there would find them.
//!
//! The session runs as the default agent with the configured permission
//! mode. Its host answers approvals and questions (both reach the daemon as
//! turn events), its children run in the daemon on the daemon-wide budget,
//! each a child job of the session's (see `subagents.rs`), and an MCP
//! server's browser login is refused rather than left waiting for a
//! terminal nobody watches.
//!
//! Until clients send their environment (#1659), the session's tools run
//! with the daemon's own environment minus provider credentials, as every
//! rho session's tools do: whatever environment the daemon started with,
//! which for an auto-spawned daemon is that of the client that spawned it.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use rho_protocol::{ErrorCode, RpcError};

use super::super::{jobs::JobId, subagents::Subagents};
use crate::{
    added_dirs::AddedDirs,
    agent::{AgentCatalog, DEFAULT_AGENT_ID},
    app::{
        agent_binding::{
            root_host_tools, AgentBinder, AgentInvocation, AgentRole, BoundAgent, SubagentTools,
        },
        cli_config::{settle_bound_config, ProviderRefreshStatus, SettledConfig},
        config_repository::ConfigRepository,
        session_runtime::{
            ApprovalPolicy, BackgroundWait, ChildAgents, HostCapabilities, Logouts,
            QuestionSupport, RunShape, SessionBuild, SessionLaunch, SessionOpening, SessionRole,
            SessionRuntime,
        },
    },
    config::Config,
    session::{Session as StoredSession, SessionPersistence},
    tools::{host_io::HostIo, mcp::McpAuthorizationMode},
};

/// The directory a session works in: an existing directory, canonicalized.
/// Parsed once from the request's `cwd`.
pub(super) struct Workspace(PathBuf);

impl Workspace {
    /// The workspace `cwd` names. A relative path, a path that does not
    /// resolve, and one that is not a directory are invalid params.
    pub(super) fn parse(cwd: &Path) -> Result<Self, RpcError> {
        let invalid = |reason: String| {
            RpcError::new(
                ErrorCode::InvalidParams,
                format!("invalid cwd {}: {reason}", cwd.display()),
            )
        };
        if !cwd.is_absolute() {
            return Err(invalid("not an absolute path".to_owned()));
        }
        let path = std::fs::canonicalize(cwd).map_err(|error| invalid(error.to_string()))?;
        if !path.is_dir() {
            return Err(invalid("not a directory".to_owned()));
        }
        Ok(Self(path))
    }
}

/// What a session opens from.
pub(super) enum Opening {
    /// Nothing yet: a new session in this workspace.
    New(Workspace),
    /// The session saved here, opened in its own workspace.
    Saved(StoredSession),
}

impl Opening {
    fn cwd(&self) -> &Path {
        match self {
            Self::New(Workspace(cwd)) => cwd,
            Self::Saved(stored) => stored.cwd(),
        }
    }
}

/// A session, open, and the config it runs with.
pub(super) struct OpenedSession {
    pub(super) runtime: SessionRuntime,
    /// The config the session was built with, which every prompt reads.
    pub(super) config: Config,
}

/// Opens the session `opening` says, whose turns are job `job`. A new
/// session's file is written with its first prompt.
pub(super) async fn open_session(
    opening: Opening,
    logouts: &Logouts,
    subagents: &Subagents,
    job: &JobId,
) -> anyhow::Result<OpenedSession> {
    let cwd = &opening.cwd().to_path_buf();
    let repository = ConfigRepository::new(None);
    let config_path = repository.configured_path()?;
    let config = repository.load()?;
    // The last config read publishes its custom providers to the daemon,
    // as each rho process publishes its own at startup.
    config.providers.activate()?;
    let agent = bind_default_agent(cwd, &config)?;
    // The agent's own config, which may pin a model and reasoning, replaces
    // the loaded one; an agent on another runtime keeps it.
    let config = agent.rho_config().cloned().unwrap_or(config);
    let SettledConfig {
        config,
        diagnostics,
    } = settle_bound_config(
        config,
        &agent,
        &ProviderRefreshStatus::NotAttempted,
        rho_providers::model::ReasoningRequestSource::PersistedOrDefault,
    )
    .await?;
    let build = SessionBuild {
        launch: SessionLaunch {
            config_path,
            no_system_prompt: false,
            no_tools: false,
            no_subagents: false,
            diagnostics,
            agent,
            prompt_suffix: None,
        },
        host: HostCapabilities {
            questions: QuestionSupport::Available,
            children: ChildAgents::InProcess(subagents.pool_for(job.clone())),
            // Nothing wakes an idle session yet, so background work holds the
            // turn open, as it does for `rho run`.
            background_wait: BackgroundWait::HoldPrompt,
            host_io: HostIo::default(),
            mcp_authorization: McpAuthorizationMode::NonInteractive,
            approvals: ApprovalPolicy::Host,
            parent_notices: None,
        },
        run: RunShape {
            role: SessionRole::Daemon,
            max_steps: None,
            hook_labels: Default::default(),
        },
    };
    let session = SessionOpening {
        build,
        process_config: &config,
        host_mcp: Default::default(),
        cwd,
        added_dirs: AddedDirs::default(),
        logouts,
    };
    // Opening a session builds a large future; keep it on the heap.
    let runtime = match opening {
        Opening::New(_) => {
            Box::pin(SessionRuntime::open_new(
                session,
                rho_sdk::SessionId::new(),
                SessionPersistence::Saved,
            ))
            .await?
        }
        Opening::Saved(stored) => Box::pin(SessionRuntime::open_saved(session, stored)).await?,
    };
    // Only a session that opened resizes every session's budget.
    subagents.set_limit(&config);
    Ok(OpenedSession { runtime, config })
}

/// The default agent of `cwd`'s catalog, bound as a person's root session
/// with every tool this platform offers.
fn bind_default_agent(cwd: &Path, config: &Config) -> anyhow::Result<BoundAgent> {
    let catalog = AgentCatalog::discover(cwd)?;
    let definition = Arc::new(catalog.find(DEFAULT_AGENT_ID)?.definition.clone());
    AgentBinder::bind(
        definition,
        AgentInvocation {
            role: AgentRole::InteractiveRoot,
            available_tools: root_host_tools(config, SubagentTools::Offered),
        },
        config,
    )
}
