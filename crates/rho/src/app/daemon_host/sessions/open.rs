//! Opening a session: a new one, or one saved earlier, which the daemon
//! reopens as the agent it was saved as (#1661). The config is the user's
//! global config as it is when the session is opened, so a config edit
//! applies to the next session opened without a daemon restart; a session
//! already open keeps its config until it reopens. A new session fails to
//! open on a config that does not load, rather than run on the last good
//! values; a reopened one, which a person may be in the middle of, runs on
//! them. The
//! agents and instructions are those of the session's workspace, as `rho`
//! launched there would find them.
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
//! of the session's (see `subagents.rs`), unless its setup has the client's
//! host run them as its own threads, and an MCP server's browser login
//! is refused rather than left waiting for a terminal nobody watches.
//! A session whose model has no usable credentials opens only when its
//! setup's `sign_in` is `later`, signed out as the TUI starts; otherwise
//! the open fails with `SignInRequired`.
//!
//! A fork (`rho/session/fork`) opens new, from the saved session it forks:
//! as the agent that session was saved as, with the parts it leaves out and
//! the directories it adds, its conversation as last saved, and its
//! settings, then in supervised mode (see `fork.rs`).
//!
//! The session's processes, MCP servers, hooks, OpenAPI sources, CLI agents,
//! and workflow runs use its client's environment, not the daemon's (#1659):
//! the opening client's, then each prompting client's (see
//! [`session_environment`]). Its project trust (`RHO_TRUST_PROJECT_*`) is
//! the opening client's for as long as the daemon has it loaded (#1779).

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use rho_protocol::{ClientEnvironment, ErrorCode, RpcError, SessionSetup};

use super::{
    super::{jobs::JobId, settings::ConfigStore, subagents::Subagents},
    pull_request_wake::PullRequestWake,
    setup::{Base, Setup},
};
use crate::{
    agent::{AgentCapabilities, AgentCatalog, AgentDefinition, DEFAULT_AGENT_ID},
    app::{
        agent_binding::{
            root_host_tools, AgentBinder, AgentInvocation, AgentRole, BoundAgent, LaunchTools,
        },
        cli_config::{settle_bound_config, ProviderRefreshStatus, SettledConfig},
        peer_sessions::PeerInbox,
        prompt_content::PromptContext,
        session_assembly::SignIn,
        session_runtime::{
            ApprovalPolicy, CacheWarming, ChildAgents, CredentialFailure, HostCapabilities,
            IdleTurns, RunShape, SessionBuild, SessionLaunch, SessionOpening, SessionRole,
            SessionRuntime, SessionUse,
        },
        subagent_pool::SubagentPool,
        tools_prompt::ToolSourceLoad,
    },
    caller_context::CallerContext,
    config::Config,
    session::Session as StoredSession,
    session_environment::{Ambient, SessionEnvironment},
    tools::mcp::McpAuthorizationMode,
    workspace::{ProjectTrust, ProjectTrusts},
};

/// The directory a session works in: an existing directory, canonicalized
/// to the form its commands take as a working directory and other `rho`
/// processes name it by. Parsed once from the request's `cwd`.
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
        let path = crate::paths::canonicalize(cwd).map_err(|error| invalid(error.to_string()))?;
        if !path.is_dir() {
            return Err(invalid("not a directory".to_owned()));
        }
        Ok(Self(path))
    }

    pub(super) fn path(&self) -> &Path {
        &self.0
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
    /// A new fork of the session saved here, in that session's workspace
    /// as the agent it was saved as (see `fork.rs`).
    Fork(StoredSession),
}

impl Opening {
    fn cwd(&self) -> &Path {
        match self {
            Self::New {
                workspace: Workspace(cwd),
                ..
            } => cwd,
            Self::Saved(stored) | Self::Fork(stored) => stored.cwd(),
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
    /// What a turn the request starts may do through session control, which
    /// `environment` carries too (see `grant.rs`).
    pub(super) grant: CallerContext,
}

/// A session, open, and the config it runs with.
pub(super) struct OpenedSession {
    pub(super) runtime: SessionRuntime,
    /// The config the session was built with, which every prompt reads.
    pub(super) config: Config,
    /// The global config it was built from, before its agent's own.
    pub(super) global: Arc<Config>,
    /// The environment the session's children see, which each prompt
    /// replaces with its client's.
    pub(super) environment: SessionEnvironment,
    /// The setup the opening request brought, as it brought it: what the
    /// session reopens with when pull request news wakes it unloaded (see
    /// `pull_request_wake.rs`).
    pub(super) setup: SessionSetup,
    /// The session it forks, for a fork, new or reopened.
    pub(super) fork_of: Option<rho_sdk::SessionId>,
    /// Prompt context its next model turn sends (see `held_context.rs`):
    /// none from here; a new fork's opener sets its word to the model
    /// before adding it.
    pub(super) held_context: PromptContext,
}

/// What a session is opened for.
enum Purpose<'a> {
    /// Running turns, signed in as `sign_in` says, with the opening
    /// request's `setup` and the daemon's pull request service.
    Run {
        sign_in: SignIn,
        setup: SessionSetup,
        pull_requests: &'a PullRequestWake,
    },
    /// Only reading what it saved.
    Read,
}

/// The environment a client's `environment` gives a session's children:
/// exactly the client's variables; its [`SessionEnvironment`] keeps provider
/// credentials from its tools' processes. A variable no process
/// could be given is invalid params, naming it, so it fails the request
/// rather than every later command (#1659).
pub(in crate::app::daemon_host) fn session_environment(
    environment: &ClientEnvironment,
) -> Result<Ambient, RpcError> {
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

/// The global config a new session opens with: `config.toml` as it is
/// now. One that does not load is `SessionOpenFailed`, naming each problem,
/// rather than the last good values. Read before anything the open cannot
/// undo, such as adding the session's worktree.
pub(super) async fn config_for_new(settings: &ConfigStore) -> Result<Arc<Config>, RpcError> {
    settings
        .config_to_open()
        .await
        .map_err(|error| open_failed(error.into()))
}

/// The global config a saved session reopens with, to run or only to read:
/// the last that loaded, as its turns read while it stays loaded, so a hand
/// edit that broke `config.toml` does not break a conversation the daemon
/// unloaded while it sat idle. Fails only if none ever loaded.
pub(super) async fn config_for_saved(settings: &ConfigStore) -> Result<Arc<Config>, RpcError> {
    settings
        .agent_config()
        .await
        .map_err(|error| open_failed(error.into()))
}

/// Opens the session `opening` says, whose turns are job `job`, with what
/// `with` brings and `global`, read from `settings` by [`config_for_new`]
/// or [`config_for_saved`].
/// A new session's file is written with its first prompt. A setup the
/// session cannot take is invalid params (see `setup.rs`); any other
/// failure is `SessionOpenFailed`.
pub(super) async fn open_session(
    opening: Opening,
    with: &OpenWith,
    subagents: &Subagents,
    pull_requests: &PullRequestWake,
    settings: &ConfigStore,
    global: Arc<Config>,
    job: &JobId,
) -> Result<OpenedSession, RpcError> {
    let base = match &opening {
        Opening::New { .. } => Base::New,
        Opening::Saved(stored) => Base::Reopened(stored),
        Opening::Fork(parent) => Base::Forked(parent),
    };
    let setup = Setup::resolve(&with.setup, opening.cwd(), base)?;
    let sign_in = match with.setup.sign_in {
        rho_protocol::SignIn::Required => SignIn::Required,
        rho_protocol::SignIn::Later => SignIn::Optional,
    };
    open_runtime(
        opening,
        SessionEnvironment::new(with.environment.clone()),
        setup,
        Purpose::Run {
            sign_in,
            setup: with.setup.clone(),
            pull_requests,
        },
        subagents.pool_for(job.clone()),
        settings,
        global,
    )
    .await
    .map_err(open_failed)
}

/// The error for a session the daemon could not open: `SignInRequired`
/// when signing in can fix it, in Rho's words with the full cause only in
/// the daemon's log, else `SessionOpenFailed`.
pub(super) fn open_failed(error: anyhow::Error) -> RpcError {
    match CredentialFailure::find(&error) {
        Some(failure) => {
            tracing::warn!(error = format!("{error:#}"), "session needs a sign-in");
            RpcError::new(
                ErrorCode::SignInRequired,
                format!("could not open session: {}", failure.message()),
            )
        }
        None => RpcError::new(
            ErrorCode::SessionOpenFailed,
            format!("could not open session: {error:#}"),
        ),
    }
}

/// The project trust `stored` is read under, which no client is there to
/// grant: none when the agents it was saved as are found without, so a
/// project file never stands in for an agent saved without trust; else the
/// project's agents and plugins, which a session saved as one of them
/// needs. A read only parses them: it starts no hooks, tool sources, or
/// turns.
fn reading_trust(stored: &StoredSession) -> anyhow::Result<ProjectTrusts> {
    let catalog = AgentCatalog::discover(stored.cwd(), ProjectTrusts::UNTRUSTED)?;
    let found = |id: &str, accepts: &dyn Fn(&AgentDefinition) -> bool| {
        catalog
            .find(id)
            .is_ok_and(|entry| accepts(&entry.definition))
    };
    let root = stored.root_agent()?;
    let delegated = stored.delegated_agent_id()?;
    let found_untrusted = root
        .as_ref()
        .is_none_or(|root| found(&root.id, &|definition| root.validate(definition).is_ok()))
        && delegated.as_deref().is_none_or(|id| {
            found(id, &|definition| {
                stored
                    .validate_agent_definition_identity(definition)
                    .is_ok()
            })
        });
    Ok(if found_untrusted {
        ProjectTrusts::UNTRUSTED
    } else {
        ProjectTrusts {
            agents: ProjectTrust::Trusted,
            plugins: ProjectTrust::Trusted,
            ..ProjectTrusts::UNTRUSTED
        }
    })
}

/// Opens `stored` only to read what it saved, its turns recorded on job
/// `job`: it starts none of its MCP servers, OpenAPI sources, or hooks, sees
/// no client's environment (see [`SessionEnvironment::for_reading`]), and
/// takes no setup but what its file keeps. Its agents are found under
/// [`reading_trust`], with [`config_for_saved`]. Fails as [`open_session`]
/// does.
pub(super) async fn open_to_read(
    stored: StoredSession,
    subagents: &Subagents,
    settings: &ConfigStore,
    job: &JobId,
) -> Result<OpenedSession, RpcError> {
    let project_trust = reading_trust(&stored).map_err(open_failed)?;
    let setup = Setup::resolve(
        &SessionSetup::default(),
        stored.cwd(),
        Base::Reopened(&stored),
    )?;
    let global = config_for_saved(settings).await?;
    open_runtime(
        Opening::Saved(stored),
        SessionEnvironment::for_reading(project_trust),
        setup,
        Purpose::Read,
        subagents.pool_for(job.clone()),
        settings,
        global,
    )
    .await
    .map_err(open_failed)
}

/// [`open_session`] and [`open_to_read`] once the setup resolved and
/// `global` was read from `settings`, for `purpose`. The session's children
/// share `subagent_pool`; its agent is found under `environment`'s project
/// trust (#1779).
async fn open_runtime(
    opening: Opening,
    environment: SessionEnvironment,
    setup: Setup,
    purpose: Purpose<'_>,
    subagent_pool: SubagentPool,
    settings: &ConfigStore,
    global: Arc<Config>,
) -> anyhow::Result<OpenedSession> {
    // A session opened to read takes no setup but what its file keeps.
    let (session_use, requested, pull_requests) = match purpose {
        Purpose::Run {
            sign_in,
            setup,
            pull_requests,
        } => (SessionUse::Run(sign_in), setup, Some(pull_requests)),
        Purpose::Read => (SessionUse::Read, SessionSetup::default(), None),
    };
    let cwd = &opening.cwd().to_path_buf();
    let config_path = settings.agent_path().to_path_buf();
    let Setup {
        left_out,
        questions,
        surfaces,
        children,
        idle_turns,
        added_dirs,
        host_mcp,
        persistence,
        adds_to_saved,
    } = setup;
    // The config each session opens with publishes its custom providers to
    // the daemon, as each rho process publishes its own at startup.
    global.providers.activate()?;
    // And its credential store: `rho credential-store set` and an edit of
    // `config.toml` choose one from outside the daemon, as the way out of
    // an OS store it cannot reach. A store that cannot be opened fails the
    // credential reads below with the same error.
    if let Err(error) = crate::credential_store::activate_from_config(&global, &config_path) {
        tracing::warn!(error = %format!("{error:#}"), "could not open the credential store");
    }
    let config = (*global).clone();
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
            &environment,
        )?,
        Opening::Saved(stored) | Opening::Fork(stored) => {
            bind_saved_agent(cwd, &config, stored, available_tools, &environment)?
        }
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
        environment.credentials().as_ref(),
        &ProviderRefreshStatus::NotAttempted,
        rho_providers::model::ReasoningRequestSource::PersistedOrDefault,
    )
    .await?;
    // Warming is for a model that ends its turn and sits idle on a process,
    // which only a session whose turns do not wait for one does. Decided as
    // the session opens: the setting's later changes stop or resume the
    // warming, not the offer.
    let cache_warming = match idle_turns {
        IdleTurns::Host if config.keep_cache_warm => CacheWarming::Host(Default::default()),
        IdleTurns::Host | IdleTurns::Never => CacheWarming::None,
    };
    // Bound before the tools are built, so the session's pull request inbox
    // is keyed by the id it opens with.
    let id = match &opening {
        Opening::New { .. } | Opening::Fork(_) => rho_sdk::SessionId::new(),
        Opening::Saved(stored) => rho_sdk::SessionId::from_string(stored.id())?,
    };
    // Only a session whose idle turns the daemon starts links and watches
    // pull requests, since news could wake no other: one whose wait
    // `BackgroundWait::of` (session_runtime/delegation.rs) makes `HostTurns`.
    // A T3 Code session has T3's own. A client that takes no daemon turns
    // (`rho run --detach`, ACP, `idle_wake: never`) gets no tools, so its
    // agent waits as a headless run does.
    // Likewise, only such a session hears of the turns it starts in other
    // sessions through session control (see `peer_wake`).
    let (pull_requests, peer_sessions) = match (children, idle_turns) {
        (ChildAgents::InProcess, IdleTurns::Host) => (
            pull_requests.map(|service| service.inbox(&id, cwd)),
            Some(PeerInbox::default()),
        ),
        (ChildAgents::InProcess, IdleTurns::Never) | (ChildAgents::HostThreads, _) => (None, None),
    };
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
            surfaces,
            pull_requests,
            peer_sessions,
            children,
            idle_turns,
            subagent_pool,
            environment: environment.clone(),
            mcp_authorization: McpAuthorizationMode::NonInteractive,
            approvals: ApprovalPolicy::Host,
            parent_notices: None,
            session_use,
            // Opening never waits for a slow MCP server, as the in-process
            // TUI's first frame does not. A host's threads bind its task
            // tools when the session is built, so they need its MCP first.
            tool_sources: match children {
                ChildAgents::InProcess => ToolSourceLoad::Deferred,
                ChildAgents::HostThreads => ToolSourceLoad::Inline,
            },
            cache_warming,
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
    let fork_of = match &opening {
        Opening::Fork(parent) => Some(rho_sdk::SessionId::from_string(parent.id())?),
        Opening::Saved(stored) => stored
            .forked_from()?
            .map(|parent| rho_sdk::SessionId::from_string(&parent.id))
            .transpose()?,
        Opening::New { .. } => None,
    };
    // Opening a session builds a large future; keep it on the heap.
    let runtime = match opening {
        Opening::New { .. } => Box::pin(SessionRuntime::open_new(session, id, persistence)).await?,
        Opening::Fork(parent) => {
            Box::pin(SessionRuntime::open_fork(session, id, &parent, persistence)).await?
        }
        Opening::Saved(stored) => {
            let mut runtime = Box::pin(SessionRuntime::open_saved(session, stored)).await?;
            debug_assert_eq!(
                runtime.session_id(),
                &id,
                "a saved session opens with its file's id"
            );
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
    Ok(OpenedSession {
        runtime,
        config,
        global,
        environment,
        setup: requested,
        fork_of,
        held_context: PromptContext::default(),
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
    environment: &SessionEnvironment,
) -> anyhow::Result<BoundAgent> {
    let Some(root) = stored.root_agent()? else {
        // A delegated session written before launch identities were saved:
        // nothing names its root, so it runs under the default agent, as it
        // did then.
        return bind_agent(cwd, config, DEFAULT_AGENT_ID, available_tools, environment);
    };
    let agent = bind_agent(cwd, config, &root.id, available_tools, environment)?;
    root.validate(agent.definition())?;
    Ok(agent)
}

/// Agent `id` of `cwd`'s catalog as `environment`'s project trust finds it,
/// its model chosen among those `environment`'s credentials can use, bound as a
/// person's root session with `available_tools`.
fn bind_agent(
    cwd: &Path,
    config: &Config,
    id: &str,
    available_tools: AgentCapabilities,
    environment: &SessionEnvironment,
) -> anyhow::Result<BoundAgent> {
    let catalog = AgentCatalog::discover(cwd, environment.project_trust())?;
    let definition = Arc::new(catalog.find(id)?.definition.clone());
    AgentBinder::bind_with_credentials(
        definition,
        AgentInvocation {
            role: AgentRole::InteractiveRoot,
            available_tools,
        },
        config,
        /*model_override*/ None,
        environment.credentials().as_ref(),
    )
}

#[cfg(test)]
#[path = "open_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "open_failed_tests.rs"]
mod open_failed_tests;
