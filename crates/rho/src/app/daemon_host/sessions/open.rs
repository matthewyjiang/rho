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
//! the setup of the request that opens it: the directories, MCP servers,
//! and parts left out its client chose (see [`OpenWith`]). Its host answers
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
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use rho_protocol::{ClientEnvironment, ErrorCode, RpcError, SessionPart, SessionSetup};

use super::{
    super::{jobs::JobId, subagents::Subagents},
    open_failed,
};
use crate::{
    added_dirs::{session_added_dirs, AddedDirs},
    agent::{AgentCapabilities, AgentCatalog, DEFAULT_AGENT_ID},
    app::{
        agent_binding::{
            root_host_tools, AgentBinder, AgentInvocation, AgentRole, BoundAgent, SubagentTools,
        },
        cli_config::{settle_bound_config, ProviderRefreshStatus, SettledConfig},
        config_repository::ConfigRepository,
        host_mcp::host_mcp_config,
        session_runtime::{
            ApprovalPolicy, BackgroundWait, ChildAgents, HostCapabilities, QuestionSupport,
            RunShape, SessionBuild, SessionLaunch, SessionOpening, SessionRole, SessionRuntime,
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
#[derive(Clone, Copy)]
pub(super) struct OpenWith<'a> {
    /// The environment the session's children start in.
    pub(super) environment: &'a Ambient,
    /// The client's choices for the session.
    pub(super) setup: &'a SessionSetup,
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
/// An additional directory the session cannot grant is invalid params,
/// naming it; any other failure is `SessionOpenFailed`.
pub(super) async fn open_session(
    opening: Opening,
    with: OpenWith<'_>,
    subagents: &Subagents,
    job: &JobId,
) -> Result<OpenedSession, RpcError> {
    let added_dirs = requested_dirs(opening.cwd(), &with.setup.additional_directories)?;
    open_runtime(opening, with, added_dirs, subagents, job)
        .await
        .map_err(open_failed)
}

/// The added directories a client's setup asks for in the session whose
/// workspace is `cwd`. The daemon has no `--add-dir` of its own, so these
/// are all the session's.
fn requested_dirs(cwd: &Path, requested: &[PathBuf]) -> Result<AddedDirs, RpcError> {
    if requested.is_empty() {
        return Ok(AddedDirs::default());
    }
    let root = std::fs::canonicalize(cwd).map_err(|error| {
        open_failed(anyhow::anyhow!(
            "could not resolve the workspace {}: {error}",
            cwd.display()
        ))
    })?;
    session_added_dirs(&AddedDirs::default(), requested, &root)
        .map_err(|rejected| RpcError::new(ErrorCode::InvalidParams, rejected.to_string()))
}

/// [`open_session`] once the setup's directories resolved to `added_dirs`.
async fn open_runtime(
    opening: Opening,
    with: OpenWith<'_>,
    added_dirs: AddedDirs,
    subagents: &Subagents,
    job: &JobId,
) -> anyhow::Result<OpenedSession> {
    let cwd = &opening.cwd().to_path_buf();
    let environment = SessionEnvironment::new(with.environment.clone());
    let without = &with.setup.without;
    let repository = ConfigRepository::new(None);
    let config_path = repository.configured_path()?;
    let config = repository.load()?;
    // The last config read publishes its custom providers to the daemon,
    // as each rho process publishes its own at startup.
    config.providers.activate()?;
    let agent = match &opening {
        Opening::New { agent, .. } => bind_agent(
            cwd,
            &config,
            agent.as_deref().unwrap_or(DEFAULT_AGENT_ID),
            without,
        )?,
        Opening::Saved(stored) => bind_saved_agent(cwd, &config, stored, without)?,
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
            no_system_prompt: without.contains(&SessionPart::SystemPrompt),
            no_tools: without.contains(&SessionPart::Tools),
            no_subagents: without.contains(&SessionPart::Subagents),
            diagnostics,
            agent,
            prompt_suffix: None,
        },
        host: HostCapabilities {
            questions: if without.contains(&SessionPart::Questions) {
                QuestionSupport::Unavailable
            } else {
                QuestionSupport::Available
            },
            children: ChildAgents::InProcess(subagents.pool_for(job.clone())),
            // Nothing wakes an idle session yet, so background work holds the
            // turn open, as it does for `rho run`.
            background_wait: BackgroundWait::HoldPrompt,
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
        host_mcp: host_mcp_config(&with.setup.mcp_servers),
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
        Opening::Saved(stored) => Box::pin(SessionRuntime::open_saved(session, stored)).await?,
    };
    // Only a session that opened resizes every session's budget.
    subagents.set_limit(&config);
    Ok(OpenedSession {
        runtime,
        config,
        environment,
    })
}

/// The agent `stored` runs as, bound as its root without the parts in
/// `without`: the one it was saved as, refused when its definition changed
/// or is gone, so a reopen never widens a session to another agent's tools
/// (#1661). A session `/agent:` rebound runs as the default agent, under
/// which the runtime binds the child again with the child's own checks.
fn bind_saved_agent(
    cwd: &Path,
    config: &Config,
    stored: &StoredSession,
    without: &BTreeSet<SessionPart>,
) -> anyhow::Result<BoundAgent> {
    if stored.delegated_agent_id()?.is_some() {
        return bind_agent(cwd, config, DEFAULT_AGENT_ID, without);
    }
    let Some((id, _fingerprint)) = stored.stored_agent_identity()? else {
        anyhow::bail!(
            "the session has no saved agent, so the daemon cannot tell which agent to reopen it as"
        );
    };
    let agent = bind_agent(cwd, config, &id, without)?;
    stored.validate_agent_definition_identity(agent.definition())?;
    Ok(agent)
}

/// Agent `id` of `cwd`'s catalog, bound as a person's root session with
/// every tool this platform offers but those of the parts in `without`.
fn bind_agent(
    cwd: &Path,
    config: &Config,
    id: &str,
    without: &BTreeSet<SessionPart>,
) -> anyhow::Result<BoundAgent> {
    let catalog = AgentCatalog::discover(cwd)?;
    let definition = Arc::new(catalog.find(id)?.definition.clone());
    AgentBinder::bind(
        definition,
        AgentInvocation {
            role: AgentRole::InteractiveRoot,
            available_tools: host_tools(config, without),
        },
        config,
    )
}

/// The tools a session's agent may have once `without` leaves parts out, as
/// `--no-tools` and `--no-subagents` narrow `rho`'s.
fn host_tools(config: &Config, without: &BTreeSet<SessionPart>) -> AgentCapabilities {
    if without.contains(&SessionPart::Tools) {
        return AgentCapabilities::default();
    }
    let subagents = if without.contains(&SessionPart::Subagents) {
        SubagentTools::Withheld
    } else {
        SubagentTools::Offered
    };
    root_host_tools(config, subagents)
}

#[cfg(test)]
#[path = "open_tests.rs"]
mod tests;
