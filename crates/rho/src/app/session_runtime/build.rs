use std::{
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::Arc,
};

use rho_sdk::{hooks::HookHostLabels, provider::ModelProvider, SessionOptions};

use super::{ApprovalPolicy, SessionPermission};
use crate::{
    added_dirs::AddedDirs,
    app::{
        agent_binding::BoundAgent,
        automation::ensure_headless_auto_classifier_model,
        headless_delegation::BackgroundWait,
        session_assembly::{
            assemble_session, ApprovalInputs, BuiltSession, SessionAssembly, SessionAssemblyOptions,
        },
        tools_prompt::McpSamplingSupport,
    },
    config::Config,
    credential_store::AppCredentialStore,
    diagnostics::RuntimeDiagnostics,
    tools::{
        host_io::HostIo,
        mcp::{config::McpConfig, McpAuthorizationMode},
        plan_exit::PlanExitHost,
    },
};

/// What a host gives every session build of one session: process flags,
/// the agent it launched with, and what it does for the session's tools.
/// Kept by the runtime, so an agent rebind builds the same way.
#[derive(Clone)]
pub(crate) struct SessionBuild {
    pub(crate) config_path: PathBuf,
    pub(crate) no_system_prompt: bool,
    pub(crate) no_tools: bool,
    pub(crate) no_subagents: bool,
    pub(crate) diagnostics: RuntimeDiagnostics,
    /// Whether the host can put questions to a person.
    pub(crate) questions: QuestionSupport,
    /// Who runs the `agent` tool's children.
    pub(crate) children: ChildAgents,
    /// Whether background work that outlives a prompt holds it open, or the
    /// host wakes the idle session when the work finishes.
    pub(crate) background_wait: BackgroundWait,
    /// The agent the host launched with. A delegated rebind stays within its
    /// tools.
    pub(crate) launch_agent: BoundAgent,
    /// What the session's host does for its tools, shared by every tool set
    /// the session builds.
    pub(crate) host_io: HostIo,
    /// The most model steps one run may take; `None` keeps the configured
    /// step limit. `rho run --max-steps` and an agent's step limit set it.
    pub(crate) max_steps: Option<NonZeroUsize>,
    /// Whose work the session's model usage is recorded as.
    pub(crate) usage: UsageAttribution,
    /// Labels every hook envelope of the session carries, such as a
    /// workflow node's.
    pub(crate) hook_labels: HookHostLabels,
    /// Instructions appended to the session's system prompt, kept when a
    /// model prompt replaces the rest, such as a child's contract with its
    /// parent.
    pub(crate) prompt_suffix: Option<&'static str>,
    /// Whether an MCP server's browser login may wait for a person.
    pub(crate) mcp_authorization: McpAuthorizationMode,
    /// Who answers the session's approval requests.
    pub(crate) approvals: ApprovalPolicy,
}

/// Whose work a session's model usage is recorded as. A delegated child's
/// usage and hooks also name the session that asked for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum UsageAttribution {
    /// A session a person or host started, such as ACP's or `rho run`'s.
    Root,
    /// A delegated child. `parent` is the session that started it; a
    /// workflow node has none.
    #[expect(
        dead_code,
        reason = "Rho subagents and workflow nodes build on the runtime in the next PR"
    )]
    Subagent { parent: Option<rho_sdk::SessionId> },
}

impl UsageAttribution {
    /// The usage purpose the session's requests are recorded under.
    pub(super) fn purpose(&self) -> &'static str {
        match self {
            Self::Root => "agent",
            Self::Subagent { .. } => "subagent",
        }
    }

    /// The session that delegated this one, if any.
    pub(super) fn parent(&self) -> Option<rho_sdk::SessionId> {
        match self {
            Self::Root => None,
            Self::Subagent { parent } => parent.clone(),
        }
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChildAgents {
    /// Subagents run in this process.
    InProcess,
    /// The host runs them as its own threads (T3 Code's `--t3code`), so its
    /// UI owns them.
    HostThreads,
}

/// Builds a session as `build` says, with the permission state of
/// `config`'s mode, for an [`super::OpenedSession`] or an agent rebind.
/// `config` is the session's config: the process config plus the host's MCP
/// servers. `added_dirs` is the session's set: launch `--add-dir` values,
/// plus the host's own roots.
pub(crate) async fn build_session(
    build: &SessionBuild,
    agent: &BoundAgent,
    config: &Config,
    workspace: &Path,
    added_dirs: &AddedDirs,
    session_options: impl FnOnce(Arc<dyn ModelProvider>) -> anyhow::Result<SessionOptions>,
) -> anyhow::Result<(BuiltSession, SessionPermission)> {
    ensure_headless_auto_classifier_model(config)?;
    // A session request can follow a terminal login another process saved,
    // even when startup itself succeeded without credentials.
    AppCredentialStore::reread_external_logins();
    let mut host_tools: anyhow::Result<()> = Ok(());
    // A host that cannot ask a person gets neither the questionnaire tool nor
    // the MCP capability, which Rho would then have to decline every time.
    let questions = match build.questions {
        QuestionSupport::Available => true,
        QuestionSupport::Unavailable => false,
    };
    let SessionAssembly {
        built,
        workspace_root,
        session_writes,
    } = assemble_session(SessionAssemblyOptions {
        config,
        config_path: build.config_path.clone(),
        cwd: workspace,
        no_system_prompt: build.no_system_prompt,
        no_tools: build.no_tools,
        no_subagents: build.no_subagents,
        questionnaire_enabled: questions,
        mcp_elicitation: if questions {
            crate::tools::mcp::McpElicitationSupport::Available
        } else {
            crate::tools::mcp::McpElicitationSupport::Unavailable
        },
        mcp_sampling: McpSamplingSupport::Unavailable,
        tool_source_attach: crate::app::tools_prompt::ToolSourceAttach::Connect,
        mcp_authorization: build.mcp_authorization,
        diagnostics: &build.diagnostics,
        agent,
        max_steps: build.max_steps,
        usage_purpose: build.usage.purpose(),
        usage_parent_session_id: build.usage.parent(),
        hook_host_labels: build.hook_labels.clone(),
        added_dirs,
        host_io: build.host_io.clone(),
        extend_tools: |mut tools| {
            match build.children {
                ChildAgents::InProcess => {}
                ChildAgents::HostThreads => {
                    host_tools = crate::tools::agent::install_host_delegation(
                        &mut tools,
                        workspace,
                        config.max_output_bytes,
                    );
                }
            }
            // Plan approval goes through a permission prompt, which every
            // host that answers approvals answers, so leaving plan mode never
            // depends on asking questions. Like the TUI, only Rho-runtime
            // agents get it; like `rho run`, an unattended session does not.
            if !build.no_tools
                && agent.rho_capabilities().is_some()
                && build.approvals.offers_plan_exit()
            {
                tools.with_plan_exit_host(config, PlanExitHost::Permission)
            } else {
                tools
            }
        },
        system_prompt_suffix: build.prompt_suffix,
        approval: |inputs: ApprovalInputs| Ok(build.approvals.wire(inputs)),
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
