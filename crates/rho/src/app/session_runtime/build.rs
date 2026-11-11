use std::{
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::Arc,
};

use rho_sdk::{hooks::HookHostLabels, provider::ModelProvider, SessionOptions};

use super::{
    ensure_auto_classifier_model, ApprovalPolicy, LeftOut, SessionPermission, UnattendedApprovals,
};
use crate::{
    added_dirs::AddedDirs,
    app::{
        agent_binding::BoundAgent,
        session_assembly::{
            assemble_session, ApprovalInputs, BuiltSession, HookStart, SessionAssembly,
            SessionAssemblyOptions, SignIn,
        },
        subagent_messaging::NoticePoster,
        subagent_pool::SubagentPool,
        tools_prompt::{McpSamplingSupport, ToolSourceAttach, ToolSourceLoad},
    },
    config::Config,
    credential_store::AppCredentialStore,
    diagnostics::RuntimeDiagnostics,
    session::{SessionOrigin, SessionProvenance, SessionTarget},
    tools::{
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
    /// The parts the session leaves out, which its saves record.
    pub(crate) left_out: LeftOut,
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
/// its children and where those in process take capacity, who answers its
/// approvals, and what it serves its tools.
#[derive(Clone)]
pub(crate) struct HostCapabilities {
    /// Whether the host can put questions to a person.
    pub(crate) questions: QuestionSupport,
    /// Who runs the `agent` tool's children.
    pub(crate) children: ChildAgents,
    /// Whether the host starts a turn when background work finishes idle.
    pub(crate) idle_turns: IdleTurns,
    /// Where the session's in-process agent runs take capacity: its
    /// subagents, and its workflow runs' agent nodes, which run in process
    /// even when the host's threads run its subagents.
    pub(crate) subagent_pool: SubagentPool,
    /// The environment the session's children see, shared by every tool
    /// set the session builds.
    pub(crate) environment: crate::session_environment::SessionEnvironment,
    /// Whether an MCP server's browser login may wait for a person.
    pub(crate) mcp_authorization: McpAuthorizationMode,
    /// Who answers the session's approval requests.
    pub(crate) approvals: ApprovalPolicy,
    /// The parent a delegated child posts notices to, which gives the
    /// session the `message_parent` tools.
    pub(crate) parent_notices: Option<Arc<dyn NoticePoster>>,
    /// Whether the host runs the session or only reads what it saved.
    pub(crate) session_use: SessionUse,
    /// When a session the host runs loads its MCP servers and OpenAPI
    /// sources: deferred, its prompts wait for them. A rebind loads its
    /// own inline, since it runs within a prompt.
    pub(crate) tool_sources: ToolSourceLoad,
}

/// Whether a host runs a session or only reads what it saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionUse {
    /// The host runs its turns: the session connects its MCP servers and
    /// OpenAPI sources and starts its workspace's hooks as it opens, and
    /// opens without a credential for its model only when the host signs in
    /// later.
    Run(SignIn),
    /// The host reads what the session saved, then shuts it down without a
    /// turn: nothing of the session starts, neither tool sources nor hooks,
    /// and opening it saves nothing to its file.
    Read,
}

impl HostCapabilities {
    /// A host nobody watches live (`rho run`, Rho subagents, workflow
    /// nodes): it asks nobody, the session decides its approvals with
    /// `approvals`, and its children run in this process on its own pool.
    pub(crate) fn unattended(approvals: UnattendedApprovals) -> Self {
        Self {
            questions: QuestionSupport::Unavailable,
            children: ChildAgents::InProcess,
            idle_turns: IdleTurns::Never,
            subagent_pool: SubagentPool::Own,
            environment: Default::default(),
            mcp_authorization: McpAuthorizationMode::from_process(),
            approvals: ApprovalPolicy::Unattended(approvals),
            parent_notices: None,
            session_use: SessionUse::Run(SignIn::Required),
            tool_sources: ToolSourceLoad::Inline,
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
            Self::Interactive | Self::Daemon | Self::Run => "agent",
            Self::Subagent { .. } | Self::WorkflowNode => "subagent",
        }
    }

    /// The session that delegated this one, which its usage and status
    /// name.
    pub(crate) fn parent(&self) -> Option<&rho_sdk::SessionId> {
        match self {
            Self::Subagent { parent, .. } => Some(parent),
            Self::Interactive | Self::Daemon | Self::Run | Self::WorkflowNode => None,
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

/// Who runs the children the `agent` tool starts. Like its MCP servers,
/// the client that opens the session chooses it; nothing is saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChildAgents {
    /// Subagents run in this process.
    InProcess,
    /// The host runs them as its own threads (T3 Code's, which `rho acp
    /// --t3code` asks for; see `tools::agent::install_host_delegation`), so
    /// its UI owns them. The agent nodes of the session's workflow runs
    /// still run in this process.
    // Only the daemon, unix-only so far, opens host-threads sessions (#1665).
    #[cfg_attr(not(unix), allow(dead_code))]
    HostThreads,
}

/// Whether the host starts a turn of its own on the idle session when a
/// workflow run or background process of the session finishes. The client
/// that opens the session chooses it; nothing is saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IdleTurns {
    /// A prompt waits for the work it started.
    Never,
    /// The host starts a turn that delivers the result, so no prompt waits.
    // Only the daemon, unix-only so far, starts turns of its own (#1665).
    #[cfg_attr(not(unix), allow(dead_code))]
    Host,
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
    tool_source_load: ToolSourceLoad,
    session_options: impl FnOnce(Arc<dyn ModelProvider>) -> anyhow::Result<SessionOptions>,
) -> anyhow::Result<(BuiltSession, SessionPermission)> {
    let credentials = build.host.environment.credentials();
    match build.run.role {
        // A daemon session opens in Auto without a classifier model, as an
        // in-process TUI session does: the classifier denies every gated tool
        // until a client picks a model and sets Auto again.
        SessionRole::Daemon => {
            crate::permission_classifier::check_screen_config(config, credentials.as_ref())?;
        }
        SessionRole::Interactive
        | SessionRole::Run
        | SessionRole::Subagent { .. }
        | SessionRole::WorkflowNode => {
            ensure_auto_classifier_model(config, credentials.as_ref())?;
        }
    }
    // A session request can follow a terminal login another process saved,
    // even when startup itself succeeded without credentials.
    AppCredentialStore::reread_external_logins();
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
        no_system_prompt: build.launch.left_out.system_prompt,
        no_tools: build.launch.left_out.tools,
        no_subagents: build.launch.left_out.subagents,
        questionnaire_enabled: questions,
        mcp_elicitation: if questions {
            crate::tools::mcp::McpElicitationSupport::Available
        } else {
            crate::tools::mcp::McpElicitationSupport::Unavailable
        },
        mcp_sampling: McpSamplingSupport::Unavailable,
        tool_source_attach: match build.host.session_use {
            SessionUse::Run(_) => ToolSourceAttach::Connect,
            SessionUse::Read => ToolSourceAttach::None,
        },
        tool_source_load,
        hook_start: match build.host.session_use {
            SessionUse::Run(_) => HookStart::Start,
            SessionUse::Read => HookStart::Skip,
        },
        sign_in: match build.host.session_use {
            SessionUse::Run(sign_in) => sign_in,
            SessionUse::Read => SignIn::Optional,
        },
        mcp_authorization: build.host.mcp_authorization,
        diagnostics: &build.launch.diagnostics,
        agent,
        max_steps: *max_steps,
        usage_purpose: role.usage_purpose(),
        usage_parent_session_id: role.parent().cloned(),
        hook_host_labels: hook_labels.clone(),
        added_dirs,
        environment: build.host.environment.clone(),
        subagent_pool: build.host.subagent_pool.clone(),
        extend_tools: |mut tools: AppToolSet| {
            if let Some(poster) = &build.host.parent_notices {
                tools.add_bundle(crate::tools::message_parent_bundle(Arc::clone(poster)));
            }
            // After MCP attach, so the host's task tools are bound.
            match build.host.children {
                ChildAgents::InProcess => {}
                ChildAgents::HostThreads => crate::tools::agent::install_host_delegation(
                    &mut tools,
                    workspace,
                    config.max_output_bytes,
                    build.host.environment.project_trust(),
                ),
            }
            with_plan_exit(tools, build, agent, config)
        },
        system_prompt_suffix: build.launch.prompt_suffix,
        approval: |inputs: ApprovalInputs| Ok(build.host.approvals.wire(inputs)),
        session_options,
    })
    .await?;
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
    if !build.launch.left_out.tools
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
