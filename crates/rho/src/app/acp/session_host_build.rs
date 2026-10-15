use std::{path::Path, sync::Arc};

use agent_client_protocol::schema::v1::SessionModeState;
use rho_sdk::{provider::ModelProvider, ApprovalSession, SessionOptions};

use super::super::{permission, AcpStartup};
use crate::{
    added_dirs::AddedDirs,
    app::{
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
    permission::PermissionMode,
};

/// `config` is the session's config: the process config plus host-supplied
/// MCP servers. `added_dirs` is the session's set: launch `--add-dir` values,
/// plus any a loaded session saved.
pub(super) async fn build_session(
    startup: &AcpStartup,
    config: &Config,
    workspace: &Path,
    added_dirs: &AddedDirs,
    session_options: impl FnOnce(Arc<dyn ModelProvider>) -> anyhow::Result<SessionOptions>,
) -> anyhow::Result<SessionAssembly> {
    ensure_headless_auto_classifier_model(config)?;
    assemble_session(SessionAssemblyOptions {
        config,
        config_path: startup.config_path.clone(),
        cwd: workspace,
        no_system_prompt: startup.no_system_prompt,
        no_tools: startup.no_tools,
        no_subagents: startup.no_subagents,
        questionnaire_enabled: false,
        mcp_elicitation: crate::tools::mcp::McpElicitationSupport::Unavailable,
        mcp_sampling: McpSamplingSupport::Unavailable,
        mcp_attach: crate::app::tools_prompt::McpAttach::Connect,
        diagnostics: &startup.diagnostics,
        agent: &startup.agent,
        max_steps: None,
        usage_purpose: "agent",
        usage_parent_session_id: None,
        hook_host_labels: rho_sdk::hooks::HookHostLabels::new(),
        added_dirs,
        extend_tools: std::convert::identity,
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
    .await
}

pub(super) fn mode_state(mode: PermissionMode) -> SessionModeState {
    SessionModeState::new(permission::mode_id(mode), permission::mode_list())
}

/// The process config with the host's `mcpServers` merged in. A host entry
/// replaces a config server of the same name for this session only.
pub(super) fn session_config(
    process: &Config,
    host_servers: &[agent_client_protocol::schema::v1::McpServer],
) -> Config {
    let mut config = process.clone();
    config
        .mcp
        .merge(super::super::host_mcp::host_mcp_config(host_servers));
    config
}
