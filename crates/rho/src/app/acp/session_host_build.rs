use std::{path::Path, sync::Arc};

use agent_client_protocol::schema::v1::SessionModeState;
use rho_sdk::{provider::ModelProvider, ApprovalSession, SessionOptions};

use super::super::{host_updates::Forms, permission, AcpHost, AcpStartup};
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
    permission::PermissionMode,
    tools::plan_exit::PlanExitHost,
};

/// `config` is the session's config: the process config plus host-supplied
/// MCP servers. `added_dirs` is the session's set: launch `--add-dir` values,
/// plus the host's `additionalDirectories`.
pub(super) async fn build_session(
    startup: &AcpStartup,
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
    // Questions reach a person only through form elicitation, so a host
    // without it gets neither the questionnaire tool nor the MCP capability,
    // which Rho would then have to decline every time.
    let questions = matches!(startup.host_updates.forms, Forms::Available(_));
    let assembly = assemble_session(SessionAssemblyOptions {
        config,
        config_path: startup.config_path.clone(),
        cwd: workspace,
        no_system_prompt: startup.no_system_prompt,
        no_tools: startup.no_tools,
        no_subagents: startup.no_subagents,
        questionnaire_enabled: questions,
        mcp_elicitation: if questions {
            crate::tools::mcp::McpElicitationSupport::Available
        } else {
            crate::tools::mcp::McpElicitationSupport::Unavailable
        },
        mcp_sampling: McpSamplingSupport::Unavailable,
        mcp_attach: crate::app::tools_prompt::McpAttach::Connect,
        diagnostics: &startup.diagnostics,
        agent,
        max_steps: None,
        usage_purpose: "agent",
        usage_parent_session_id: None,
        hook_host_labels: rho_sdk::hooks::HookHostLabels::new(),
        added_dirs,
        // `--t3code`: the `agent` tool delegates to T3 so its UI owns children.
        extend_tools: |mut tools| {
            match startup.host {
                AcpHost::Generic => {}
                AcpHost::T3Code => {
                    host_tools = crate::tools::agent::install_host_delegation(
                        &mut tools,
                        workspace,
                        config.max_output_bytes,
                    );
                }
            }
            // Plan approval goes through `session/request_permission`, which
            // every host answers, so leaving plan mode never depends on
            // elicitation. Like the TUI, only Rho-runtime agents get it.
            if !startup.no_tools && agent.rho_capabilities().is_some() {
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

pub(super) fn mode_state(mode: PermissionMode) -> SessionModeState {
    SessionModeState::new(permission::mode_id(mode), permission::mode_list())
}

/// The process config with the host's `mcpServers` merged in. A host entry
/// replaces a config server of the same name for this session only, and an
/// invalid host entry suppresses it.
pub(super) fn session_config(
    process: &Config,
    host_servers: &[agent_client_protocol::schema::v1::McpServer],
) -> Config {
    let mut config = process.clone();
    super::super::host_mcp::overlay_host_mcp(&mut config.mcp, host_servers);
    config
}
