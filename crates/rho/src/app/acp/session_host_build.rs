use std::{path::Path, sync::Arc};

use agent_client_protocol::schema::v1::SessionModeState;
use rho_sdk::{provider::ModelProvider, ApprovalSession, SessionOptions};

use super::super::{permission, AcpStartup};
use crate::{
    app::{
        automation::ensure_headless_auto_classifier_model,
        interactive_runtime::startup::{
            approval_channel_for, ApprovalChannel, ApprovalChannelOptions,
        },
        session_assembly::{
            assemble_session, ApprovalInputs, BuiltSession, SessionApproval, SessionAssembly,
            SessionAssemblyOptions,
        },
        tools_prompt::McpSamplingSupport,
    },
    permission::PermissionMode,
    tools::agent::BackgroundSubagents,
};

pub(super) async fn build_session(
    startup: &AcpStartup,
    workspace: &Path,
    session_options: impl FnOnce(Arc<dyn ModelProvider>) -> anyhow::Result<SessionOptions>,
) -> anyhow::Result<BuiltSession> {
    ensure_headless_auto_classifier_model(&startup.config)?;
    let SessionAssembly { built, .. } = assemble_session(SessionAssemblyOptions {
        config: &startup.config,
        config_path: startup.config_path.clone(),
        cwd: workspace,
        no_system_prompt: startup.no_system_prompt,
        no_tools: startup.no_tools,
        no_subagents: startup.no_subagents,
        questionnaire_enabled: false,
        mcp_elicitation: crate::tools::mcp::McpElicitationSupport::Unavailable,
        mcp_sampling: McpSamplingSupport::Unavailable,
        background_subagents: BackgroundSubagents::Disabled,
        diagnostics: &startup.diagnostics,
        agent: &startup.agent,
        max_steps: None,
        usage_purpose: "agent",
        usage_parent_session_id: None,
        hook_host_labels: rho_sdk::hooks::HookHostLabels::new(),
        extend_tools: |tool_set| tool_set,
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
    Ok(built)
}

pub(super) fn mode_state(mode: PermissionMode) -> SessionModeState {
    SessionModeState::new(permission::mode_id(mode), permission::mode_list())
}
