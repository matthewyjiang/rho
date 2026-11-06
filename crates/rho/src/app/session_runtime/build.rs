use std::{
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::Arc,
};

use rho_sdk::{hooks::HookHostLabels, provider::ModelProvider, SessionOptions};

use super::{
    ensure_auto_classifier_model, ApprovalPolicy, BackgroundWait, SessionPermission,
    UnattendedApprovals,
};
use crate::{
    added_dirs::AddedDirs,
    app::{
        agent_binding::BoundAgent,
        session_assembly::{
            assemble_session, ApprovalInputs, BuiltSession, SessionAssembly, SessionAssemblyOptions,
        },
        subagent_messaging::NoticePoster,
        subagent_pool::SubagentPool,
        tools_prompt::McpSamplingSupport,
    },
    config::Config,
    credential_store::AppCredentialStore,
    diagnostics::RuntimeDiagnostics,
    session::{SessionOrigin, SessionProvenance, SessionTarget},
    tools::{
        host_io::HostIo,
        mcp::{config::McpConfig, McpAuthorizationMode},
        plan_exit::PlanExitHost,
        sdk_registry::AppToolSet,
    },
};

/// What a host gives every session build of one session: how it was
/// launched, what the host does for the session, and how each run is
/// shaped. Kept by the runtime, so an agent rebind builds the same way.
#[derive(Clone)]
pub(crate) struct SessionBuild {
    pub(crate) launch: SessionLaunch,
    pub(crate) host: HostCapabilities,
    /// What the session is for and how each of its runs is shaped.
    pub(crate) run: RunShape,
}

/// How the session was launched: process flags and the agent it launched
/// as.
#[derive(Clone)]
pub(crate) struct SessionLaunch {
    pub(crate) config_path: PathBuf,
    pub(crate) no_system_prompt: bool,
    pub(crate) no_tools: bool,
    pub(crate) no_subagents: bool,
    pub(crate) diagnostics: RuntimeDiagnostics,
    /// The agent the host launched with. A delegated rebind stays within its
    /// tools.
    pub(crate) agent: BoundAgent,
    /// Instructions appended to the session's system prompt, kept when a
    /// model prompt replaces the rest, such as a child's contract with its
    /// parent. Not part of the [`RunShape`]: it lives in the built prompt,
    /// which a permission mode rebuild reuses.
    pub(crate) prompt_suffix: Option<&'static str>,
}

/// What the session's host does for it: whether it asks a person, who runs
/// its children and answers its approvals, and what it serves its tools.
#[derive(Clone)]
pub(crate) struct HostCapabilities {
    /// Whether the host can put questions to a person.
    pub(crate) questions: QuestionSupport,
    /// Who runs the `agent` tool's children.
    pub(crate) children: ChildAgents,
    /// Whether background work that outlives a prompt holds it open, or the
    /// host wakes the idle session when the work finishes.
    pub(crate) background_wait: BackgroundWait,
    /// What the session's host does for its tools, shared by every tool set
    /// the session builds.
    pub(crate) host_io: HostIo,
    /// Whether an MCP server's browser login may wait for a person.
    pub(crate) mcp_authorization: McpAuthorizationMode,
    /// Who answers the session's approval requests.
    pub(crate) approvals: ApprovalPolicy,
    /// The parent a delegated child posts notices to, which gives the
    /// session the `message_parent` tools.
    pub(crate) parent_notices: Option<Arc<dyn NoticePoster>>,
}

impl HostCapabilities {
    /// A host nobody watches live (`rho run`, Rho subagents, workflow
    /// nodes): it asks nobody, the session decides its approvals with
    /// `approvals`, its children run in this process, and background work
    /// holds the prompt open, since nothing wakes the session after it.
    pub(crate) fn unattended(approvals: UnattendedApprovals) -> Self {
        Self {
            questions: QuestionSupport::Unavailable,
            children: ChildAgents::InProcess(SubagentPool::Own),
            background_wait: BackgroundWait::HoldPrompt,
            host_io: HostIo::default(),
            mcp_authorization: McpAuthorizationMode::from_process(),
            approvals: ApprovalPolicy::Unattended(approvals),
            parent_notices: None,
        }
    }
}

/// What a session is for and how each of its runs is shaped. Every SDK
/// runtime the session builds, when it opens and again for a permission
/// mode change, takes all of it: each build site destructures it whole, so
/// a new knob cannot reach one and miss the other.
#[derive(Clone)]
pub(crate) struct RunShape {
    /// What the session is for, which decides whose work its model usage
    /// is recorded as and the origin a new session's file records.
    pub(crate) role: SessionRole,
    /// The most model steps one run may take; `None` keeps the configured
    /// step limit. `rho run --max-steps` and an agent's step limit set it.
    pub(crate) max_steps: Option<NonZeroUsize>,
    /// Labels every hook envelope of the session carries, such as a
    /// workflow node's.
    pub(crate) hook_labels: HookHostLabels,
}

/// What a session is for. Its model usage is recorded as an agent's or a
/// subagent's from it, a delegated child's usage and hooks also naming the
/// session that started it, and a new session's file records its origin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SessionRole {
    /// A person's session in the interactive TUI.
    #[expect(dead_code, reason = "the TUI builds on the runtime in a later PR")]
    Interactive,
    /// An ACP client's session, such as Zed's or T3 Code's.
    Acp,
    /// A session the rho daemon hosts, which its clients prompt.
    Daemon,
    /// `rho run`'s session.
    Run,
    /// A Rho subagent `parent` started through the `agent` tool. Every host
    /// binds its session as the subagent manager's parent before its first
    /// turn, so a subagent always has one; only workflow nodes run
    /// parentless.
    Subagent {
        parent: rho_sdk::SessionId,
        /// The workspace `parent` is saved in, `None` while it has no file.
        /// An id is unique only within a workspace, so the subagent's file
        /// records both to name its parent.
        parent_workspace: Option<PathBuf>,
    },
    /// An agent node of a workflow run, which no session started.
    WorkflowNode,
}

impl SessionRole {
    /// The usage purpose the session's requests are recorded under.
    pub(super) fn usage_purpose(&self) -> &'static str {
        match self {
            Self::Interactive | Self::Acp | Self::Daemon | Self::Run => "agent",
            Self::Subagent { .. } | Self::WorkflowNode => "subagent",
        }
    }

    /// The session that delegated this one, which its usage and status
    /// name.
    pub(crate) fn parent(&self) -> Option<&rho_sdk::SessionId> {
        match self {
            Self::Subagent { parent, .. } => Some(parent),
            Self::Interactive | Self::Acp | Self::Daemon | Self::Run | Self::WorkflowNode => None,
        }
    }

    /// Where a new session's file records it came from: the host that
    /// created it and, for a subagent of a saved session, that parent, so
    /// deleting the parent deletes it too. A workflow node records no
    /// parent: the workflow runtime does not know the session that started
    /// its run, if any.
    pub(super) fn provenance(&self) -> SessionProvenance {
        let (origin, parent) = match self {
            Self::Interactive => (SessionOrigin::Interactive, None),
            Self::Acp => (SessionOrigin::Acp, None),
            Self::Daemon => (SessionOrigin::Daemon, None),
            Self::Run => (SessionOrigin::Run, None),
            Self::Subagent {
                parent,
                parent_workspace,
            } => (
                SessionOrigin::Subagent,
                parent_workspace
                    .as_ref()
                    .map(|cwd| SessionTarget::new(parent.to_string(), cwd.clone())),
            ),
            Self::WorkflowNode => (SessionOrigin::WorkflowNode, None),
        };
        SessionProvenance { origin, parent }
    }
}

/// Whether a host can put a question to a person, such as through an ACP
/// form elicitation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QuestionSupport {
    Available,
    /// Sessions get neither the questionnaire tool nor the MCP elicitation
    /// capability, and a question that arrives anyway is declined.
    Unavailable,
}

/// Who runs the children the `agent` tool starts.
#[derive(Clone)]
pub(crate) enum ChildAgents {
    /// Subagents run in this process, as do the agent nodes of the session's
    /// workflow runs, taking capacity and reporting as the host says.
    InProcess(SubagentPool),
    /// The host runs them as its own threads (T3 Code's `--t3code`), so its
    /// UI owns them.
    HostThreads,
}

impl ChildAgents {
    /// The pool the session's in-process agent runs take capacity from:
    /// its subagents', and its workflow runs' agent nodes, which run in
    /// process on the session's own pool even when the host's threads run
    /// its subagents.
    pub(crate) fn pool(&self) -> SubagentPool {
        match self {
            Self::InProcess(pool) => pool.clone(),
            Self::HostThreads => SubagentPool::Own,
        }
    }
}

/// Builds a session as `build` says, with the permission state of
/// `config`'s mode, for opening a session (see `open.rs`) or an agent rebind.
/// `config` is the session's config: the process config plus the host's MCP
/// servers. `added_dirs` is the session's set: launch `--add-dir` values,
/// plus the host's own roots.
pub(super) async fn build_session(
    build: &SessionBuild,
    agent: &BoundAgent,
    config: &Config,
    workspace: &Path,
    added_dirs: &AddedDirs,
    session_options: impl FnOnce(Arc<dyn ModelProvider>) -> anyhow::Result<SessionOptions>,
) -> anyhow::Result<(BuiltSession, SessionPermission)> {
    ensure_auto_classifier_model(config)?;
    // A session request can follow a terminal login another process saved,
    // even when startup itself succeeded without credentials.
    AppCredentialStore::reread_external_logins();
    let mut host_tools: anyhow::Result<()> = Ok(());
    // A host that cannot ask a person gets neither the questionnaire tool nor
    // the MCP capability, which Rho would then have to decline every time.
    let questions = match build.host.questions {
        QuestionSupport::Available => true,
        QuestionSupport::Unavailable => false,
    };
    let RunShape {
        role,
        max_steps,
        hook_labels,
    } = &build.run;
    let SessionAssembly {
        built,
        workspace_root,
        session_writes,
    } = assemble_session(SessionAssemblyOptions {
        config,
        config_path: build.launch.config_path.clone(),
        cwd: workspace,
        no_system_prompt: build.launch.no_system_prompt,
        no_tools: build.launch.no_tools,
        no_subagents: build.launch.no_subagents,
        questionnaire_enabled: questions,
        mcp_elicitation: if questions {
            crate::tools::mcp::McpElicitationSupport::Available
        } else {
            crate::tools::mcp::McpElicitationSupport::Unavailable
        },
        mcp_sampling: McpSamplingSupport::Unavailable,
        tool_source_attach: crate::app::tools_prompt::ToolSourceAttach::Connect,
        mcp_authorization: build.host.mcp_authorization,
        diagnostics: &build.launch.diagnostics,
        agent,
        max_steps: *max_steps,
        usage_purpose: role.usage_purpose(),
        usage_parent_session_id: role.parent().cloned(),
        hook_host_labels: hook_labels.clone(),
        added_dirs,
        host_io: build.host.host_io.clone(),
        subagent_pool: build.host.children.pool(),
        extend_tools: |mut tools: AppToolSet| {
            if let Some(poster) = &build.host.parent_notices {
                tools.add_bundle(crate::tools::message_parent_bundle(Arc::clone(poster)));
            }
            match build.host.children {
                ChildAgents::InProcess(_) => {}
                ChildAgents::HostThreads => {
                    host_tools = crate::tools::agent::install_host_delegation(
                        &mut tools,
                        workspace,
                        config.max_output_bytes,
                    );
                }
            }
            with_plan_exit(tools, build, agent, config)
        },
        system_prompt_suffix: build.launch.prompt_suffix,
        approval: |inputs: ApprovalInputs| Ok(build.host.approvals.wire(inputs)),
        session_options,
    })
    .await?;
    if let Err(error) = host_tools {
        built.teardown().await;
        return Err(error);
    }
    let permission = SessionPermission {
        mode: config.permission_mode,
        writes: session_writes,
        workspace_root,
    };
    Ok((built, permission))
}

/// `tools` with `exit_plan_mode` when `build` offers it to `agent`. Plan
/// approval goes through a permission prompt, which every host that answers
/// approvals answers, so leaving plan mode never depends on asking
/// questions. Like the TUI, only Rho-runtime agents get it; like `rho run`,
/// an unattended session does not.
pub(super) fn with_plan_exit(
    tools: AppToolSet,
    build: &SessionBuild,
    agent: &BoundAgent,
    config: &Config,
) -> AppToolSet {
    if !build.launch.no_tools
        && agent.rho_capabilities().is_some()
        && build.host.approvals.offers_plan_exit()
    {
        tools.with_plan_exit_host(config, PlanExitHost::Permission)
    } else {
        tools
    }
}

/// The process config with the host's MCP servers merged in.
pub(crate) fn session_config(process: &Config, host_mcp: &McpConfig) -> Config {
    let mut config = process.clone();
    overlay_host_mcp(&mut config.mcp, host_mcp.clone());
    config
}

/// Overlays a host's MCP servers onto the process MCP config for one
/// session. A host entry replaces a config server of the same name for this
/// session only, because the host's entry carries session-specific
/// credentials; an invalid host entry suppresses it rather than connect it
/// under an identity the host meant to replace.
fn overlay_host_mcp(process: &mut McpConfig, host: McpConfig) {
    for invalid in &host.invalid_servers {
        if process.servers.remove(&invalid.identity).is_some() {
            tracing::warn!(server = %invalid.identity, "host MCP entry is invalid; not connecting the configured server it replaces");
        }
    }
    process.merge(host);
}
