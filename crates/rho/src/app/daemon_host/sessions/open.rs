//! Opening a session: a new one, or one saved earlier, which the daemon
//! reopens as the agent it was saved as (#1661). The config is the user's
//! global config, read when the session is opened, so a config edit applies
//! to the next session opened without a daemon restart; the agents and
//! instructions are those of the session's workspace, as `rho` launched
//! there would find them.
//!
//! A new session runs as the agent its client names, else the default
//! agent, with the configured model, reasoning, fast mode, and permission
//! mode. A reopened one runs as the agent it was saved as, which must still
//! be defined as it was, and with the settings a client last chose for it
//! (`rho/session/configure`), else the configured ones. Either way it takes
//! the setup of the request that opens it, on top of the directories and
//! parts left out its file keeps (see `setup.rs`). Its host answers
//! approvals and questions (both reach the daemon as turn events), its
//! children run in the daemon on the daemon-wide budget, each a child job
//! of the session's (see `subagents.rs`), and an MCP server's browser login
//! is refused rather than left waiting for a terminal nobody watches.
//!
//! The session's processes, MCP servers, hooks, OpenAPI sources, CLI agents,
//! and workflow runs use its client's environment, not the daemon's (#1659):
//! the opening client's, then each prompting client's (see
//! [`session_environment`]).

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use rho_protocol::{ClientEnvironment, ErrorCode, RpcError, SessionSetup};

use super::{
    super::{jobs::JobId, subagents::Subagents},
    open_failed,
    setup::Setup,
};
use crate::{
    agent::{AgentCapabilities, AgentCatalog, DEFAULT_AGENT_ID},
    app::{
        agent_binding::{
            root_host_tools, AgentBinder, AgentInvocation, AgentRole, BoundAgent, LaunchTools,
        },
        cli_config::{settle_bound_config, ProviderRefreshStatus, SettledConfig},
        config_repository::ConfigRepository,
        session_runtime::{
            ApprovalPolicy, HostCapabilities, RunShape, SessionBuild, SessionLaunch,
            SessionOpening, SessionRole, SessionRuntime,
        },
    },
    config::Config,
    session::{Session as StoredSession, SessionPersistence},
    session_environment::{Ambient, SessionEnvironment},
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
    /// Nothing yet: a new session in `workspace`, running as agent `agent`,
    /// the default agent when `None`.
    New {
        workspace: Workspace,
        agent: Option<String>,
    },
    /// The session saved here, opened in its own workspace as the agent it
    /// was saved as.
    Saved(StoredSession),
}

impl Opening {
    fn cwd(&self) -> &Path {
        match self {
            Self::New {
                workspace: Workspace(cwd),
                ..
            } => cwd,
            Self::Saved(stored) => stored.cwd(),
        }
    }
}

/// What a request brings for the session it names, which applies only when
/// that request opens the session: a new session, or a saved one it
/// reopens. A session the daemon has loaded keeps what it opened with, so a
/// request for it leaves this unused.
pub(super) struct OpenWith {
    /// The environment the session's children start in.
    pub(super) environment: Ambient,
    /// The client's choices for the session.
    pub(super) setup: SessionSetup,
}

/// A session, open, and the config it runs with.
pub(super) struct OpenedSession {
    pub(super) runtime: SessionRuntime,
    /// The config the session was built with, which every prompt reads.
    pub(super) config: Config,
    /// The environment the session's children see, which each prompt
    /// replaces with its client's.
    pub(super) environment: SessionEnvironment,
}

/// The environment a client's `environment` gives a session's children:
/// exactly the client's variables; its [`SessionEnvironment`] keeps provider
/// credentials from its tools' processes. A variable no process
/// could be given is invalid params, naming it, so it fails the request
/// rather than every later command (#1659).
pub(super) fn session_environment(environment: &ClientEnvironment) -> Result<Ambient, RpcError> {
    let variables = environment
        .iter()
        .map(|(name, value)| {
            if name.is_empty() || name.contains(['=', '\0']) || value.contains('\0') {
                return Err(RpcError::new(
                    ErrorCode::InvalidParams,
                    format!(
                        "invalid environment variable {name:?}: a name must be nonempty without `=` or NUL, and a value without NUL"
                    ),
                ));
            }
            Ok((name.to_owned(), value.to_owned()))
        })
        .collect::<Result<_, _>>()?;
    Ok(Ambient::Client(Arc::new(variables)))
}

/// Opens the session `opening` says, whose turns are job `job`, with what
/// `with` brings. A new session's file is written with its first prompt.
/// A setup the session cannot take is invalid params (see `setup.rs`); any
/// other failure is `SessionOpenFailed`.
pub(super) async fn open_session(
    opening: Opening,
    with: &OpenWith,
    subagents: &Subagents,
    job: &JobId,
) -> Result<OpenedSession, RpcError> {
    let saved = match &opening {
        Opening::New { .. } => None,
        Opening::Saved(stored) => Some(stored),
    };
    let setup = Setup::resolve(&with.setup, opening.cwd(), saved)?;
    open_runtime(opening, &with.environment, setup, subagents, job)
        .await
        .map_err(open_failed)
}

/// [`open_session`] once its setup resolved.
async fn open_runtime(
    opening: Opening,
    environment: &Ambient,
    setup: Setup,
    subagents: &Subagents,
    job: &JobId,
) -> anyhow::Result<OpenedSession> {
    let cwd = &opening.cwd().to_path_buf();
    let environment = SessionEnvironment::new(environment.clone());
    let Setup {
        left_out,
        questions,
        added_dirs,
        host_mcp,
        adds_to_saved,
    } = setup;
    let repository = ConfigRepository::new(None);
    let config_path = repository.configured_path()?;
    let config = repository.load()?;
    // The last config read publishes its custom providers to the daemon,
    // as each rho process publishes its own at startup.
    config.providers.activate()?;
    let available_tools = root_host_tools(
        &config,
        LaunchTools::from_flags(left_out.tools, left_out.subagents),
    );
    let agent = match &opening {
        Opening::New { agent, .. } => bind_agent(
            cwd,
            &config,
            agent.as_deref().unwrap_or(DEFAULT_AGENT_ID),
            available_tools,
        )?,
        Opening::Saved(stored) => bind_saved_agent(cwd, &config, stored, available_tools)?,
    };
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
            left_out,
            diagnostics,
            agent,
            prompt_suffix: None,
        },
        host: HostCapabilities {
            questions,
            subagent_pool: subagents.pool_for(job.clone()),
            host_io: HostIo {
                environment: environment.clone(),
                ..HostIo::default()
            },
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
        host_mcp,
        cwd,
        added_dirs,
    };
    // Opening a session builds a large future; keep it on the heap.
    let runtime = match opening {
        Opening::New { .. } => {
            Box::pin(SessionRuntime::open_new(
                session,
                rho_sdk::SessionId::new(),
                SessionPersistence::Saved,
            ))
            .await?
        }
        Opening::Saved(stored) => {
            let mut runtime = Box::pin(SessionRuntime::open_saved(session, stored)).await?;
            if adds_to_saved {
                // The next turn would save it too; an unload before then
                // would lose it.
                if let Err(error) = runtime.save_state() {
                    tracing::warn!(
                        error = format!("{error:#}"),
                        "could not save the setup a reopen added; the session's next turn saves it"
                    );
                }
            }
            runtime
        }
    };
    // Only a session that opened resizes every session's budget.
    subagents.set_limit(&config);
    Ok(OpenedSession {
        runtime,
        config,
        environment,
    })
}

/// The root agent `stored` runs under, bound with `available_tools`: the
/// one it was saved as, refused when its definition changed or is gone, so
/// a reopen never widens a session to another agent's tools (#1661). A
/// session `/agent:` rebound binds the root it was rebound from, with the
/// same checks; the runtime then binds the child again under that root's
/// tools, with the child's own checks.
fn bind_saved_agent(
    cwd: &Path,
    config: &Config,
    stored: &StoredSession,
    available_tools: AgentCapabilities,
) -> anyhow::Result<BoundAgent> {
    let Some(root) = stored.root_agent()? else {
        // A delegated session written before launch identities were saved:
        // nothing names its root, so it runs under the default agent, as it
        // did then.
        return bind_agent(cwd, config, DEFAULT_AGENT_ID, available_tools);
    };
    let agent = bind_agent(cwd, config, &root.id, available_tools)?;
    root.validate(agent.definition())?;
    Ok(agent)
}

/// Agent `id` of `cwd`'s catalog, bound as a person's root session with
/// `available_tools`.
fn bind_agent(
    cwd: &Path,
    config: &Config,
    id: &str,
    available_tools: AgentCapabilities,
) -> anyhow::Result<BoundAgent> {
    let catalog = AgentCatalog::discover(cwd)?;
    let definition = Arc::new(catalog.find(id)?.definition.clone());
    AgentBinder::bind(
        definition,
        AgentInvocation {
            role: AgentRole::InteractiveRoot,
            available_tools,
        },
        config,
    )
}

#[cfg(test)]
#[path = "open_tests.rs"]
mod tests;
