use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use rho_sdk::{provider::ModelProvider, ApprovalSession, SessionOptions};

use crate::{
    added_dirs::AddedDirs,
    app::{
        agent_binding::BoundAgent,
        automation::ensure_headless_auto_classifier_model,
        interactive_runtime::startup::{
            approval_channel_for, ApprovalChannel, ApprovalChannelOptions,
        },
        session_assembly::{
            assemble_session, ApprovalInputs, SessionApproval, SessionAssembly,
            SessionAssemblyOptions,
        },
        tools_prompt::McpSamplingSupport,
    },
    config::Config,
    credential_store::AppCredentialStore,
    diagnostics::RuntimeDiagnostics,
    tools::{host_io::HostIo, mcp::config::McpConfig, plan_exit::PlanExitHost},
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
    /// The agent the host launched with. A delegated rebind stays within its
    /// tools.
    pub(crate) launch_agent: BoundAgent,
    /// What the session's host does for its tools, shared by every tool set
    /// the session builds.
    pub(crate) host_io: HostIo,
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
) -> anyhow::Result<SessionAssembly> {
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
    let assembly = assemble_session(SessionAssemblyOptions {
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
        // A runtime has no terminal for a person to finish an MCP login in;
        // ACP's stdin, for one, is its protocol pipe.
        mcp_authorization: crate::tools::mcp::McpAuthorizationMode::NonInteractive,
        diagnostics: &build.diagnostics,
        agent,
        max_steps: None,
        usage_purpose: "agent",
        usage_parent_session_id: None,
        hook_host_labels: rho_sdk::hooks::HookHostLabels::new(),
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
            // host answers, so leaving plan mode never depends on asking
            // questions. Like the TUI, only Rho-runtime agents get it.
            if !build.no_tools && agent.rho_capabilities().is_some() {
                tools.with_plan_exit_host(config, PlanExitHost::Permission)
            } else {
                tools
            }
        },
        system_prompt_suffix: None,
        approval: |inputs: ApprovalInputs| {
            let ApprovalChannel {
                handler, receiver, ..
            } = approval_channel_for(
                inputs.config.permission_mode,
                ApprovalChannelOptions {
                    config: inputs.config,
                    workspace_path: inputs.workspace_root,
                    usage_recording: inputs.usage_recording,
                    session_writes: inputs.session_writes,
                },
            );
            Ok(SessionApproval {
                session: handler.map(ApprovalSession::from_shared),
                receiver,
            })
        },
        session_options,
    })
    .await?;
    if let Err(error) = host_tools {
        assembly.built.teardown().await;
        return Err(error);
    }
    Ok(assembly)
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
pub(crate) fn overlay_host_mcp(process: &mut McpConfig, host: McpConfig) {
    for invalid in &host.invalid_servers {
        if process.servers.remove(&invalid.identity).is_some() {
            tracing::warn!(server = %invalid.identity, "host MCP entry is invalid; not connecting the configured server it replaces");
        }
    }
    process.merge(host);
}
