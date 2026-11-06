//! Opening a thread's session: config and agents read for the thread's
//! workspace when the thread is created, as `rho` would read them launched
//! there, so a config edit applies to the next thread without a daemon
//! restart.
//!
//! The session runs as the default agent with the configured permission
//! mode. Its host answers approvals and questions (both reach the daemon as
//! turn events), its children run in the daemon, and an MCP server's
//! browser login is refused rather than left waiting for a terminal nobody
//! watches.
//!
//! Until clients send their environment (2d), the session's tools run with
//! the daemon's own environment minus provider credentials, as every rho
//! session's tools do: whatever environment the daemon started with, which
//! for an auto-spawned daemon is that of the client that spawned it.

use std::{path::Path, sync::Arc};

use crate::{
    added_dirs::AddedDirs,
    agent::{AgentCatalog, DEFAULT_AGENT_ID},
    app::{
        agent_binding::{AgentBinder, AgentInvocation, AgentRole, BoundAgent},
        bootstrap::{root_host_tools, SubagentTools},
        cli_config::{self, ProviderRefreshStatus},
        config_repository::ConfigRepository,
        session_runtime::{
            ApprovalPolicy, BackgroundWait, ChildAgents, HostCapabilities, Logouts,
            QuestionSupport, RunShape, SessionBuild, SessionLaunch, SessionOpening, SessionRole,
            SessionRuntime,
        },
    },
    config::Config,
    credential_store::AppCredentialStore,
    diagnostics::RuntimeDiagnostics,
    session::SessionPersistence,
    tools::{host_io::HostIo, mcp::McpAuthorizationMode},
};

/// A thread's session, open, and the config it runs with.
pub(super) struct OpenedThread {
    pub(super) runtime: SessionRuntime,
    /// The config the session was built with, which every prompt reads.
    pub(super) config: Config,
}

/// Opens a new, saved session in `cwd` for a thread. Its file is written
/// with its first prompt.
pub(super) async fn open_thread(cwd: &Path, logouts: &Logouts) -> anyhow::Result<OpenedThread> {
    let cwd = std::fs::canonicalize(cwd)?;
    anyhow::ensure!(cwd.is_dir(), "{} is not a directory", cwd.display());
    let repository = ConfigRepository::new(None);
    let config_path = repository.configured_path()?;
    let mut config = repository.load()?;
    // The last config read publishes its custom providers to the daemon,
    // as each rho process publishes its own at startup.
    config.providers.activate()?;
    cli_config::prepare_model_metadata(
        &config,
        &AppCredentialStore,
        &ProviderRefreshStatus::NotAttempted,
    )
    .await;
    cli_config::normalize_reasoning(&mut config);
    let agent = bind_default_agent(&cwd, &config)?;
    // The agent may pin its own model and reasoning.
    let config = agent.rho_config().cloned().unwrap_or(config);
    let diagnostics = RuntimeDiagnostics::new(&config);
    diagnostics.update_agent(agent.id().as_str(), &agent.fingerprint().to_string());
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
            children: ChildAgents::InProcess,
            // Nothing wakes an idle thread yet, so background work holds the
            // turn open, as it does for `rho run`.
            background_wait: BackgroundWait::HoldPrompt,
            host_io: HostIo::default(),
            mcp_authorization: McpAuthorizationMode::NonInteractive,
            approvals: ApprovalPolicy::Host,
            parent_notices: None,
        },
        run: RunShape {
            role: SessionRole::DaemonThread,
            max_steps: None,
            hook_labels: Default::default(),
        },
    };
    let opening = SessionOpening {
        build,
        process_config: &config,
        host_mcp: Default::default(),
        cwd: &cwd,
        added_dirs: AddedDirs::default(),
        logouts,
    };
    // Opening a session builds a large future; keep it on the heap.
    let runtime = Box::pin(SessionRuntime::open_new(
        opening,
        rho_sdk::SessionId::new(),
        SessionPersistence::Saved,
    ))
    .await?;
    Ok(OpenedThread { runtime, config })
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

/// Opens the credential store every thread's provider reads, once, as the
/// daemon starts: like every rho process, the daemon opens it at startup.
/// A config that does not load is logged, and threads then fail to open
/// with the same error.
pub(in crate::app::daemon_host) fn open_credential_store() {
    let repository = ConfigRepository::new(None);
    let opened = repository.configured_path().and_then(|path| {
        let mut config = repository.load()?;
        crate::credential_store::initialize_from_config(&mut config, &path)
    });
    if let Err(error) = opened {
        tracing::warn!(error = %format!("{error:#}"), "could not open the credential store");
    }
}
