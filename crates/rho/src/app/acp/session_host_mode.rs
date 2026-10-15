//! `session/set_mode` for one ACP session: swaps the permission mode by
//! rebuilding the runtime around the live conversation, the same way the
//! interactive host does.

use std::path::PathBuf;

use agent_client_protocol::Error as AcpError;
use rho_sdk::{ApprovalSession, SessionOptions};

use super::{host_apply_error, model_context_window, model_scoped_config, SessionHost};
use crate::{
    app::{
        automation::ensure_headless_auto_classifier_model,
        interactive_runtime::startup::{approval_channel_for, ApprovalChannelOptions},
        policy::AppPolicy,
        runtime_builder::{build_runtime, RuntimeBuildOptions},
        sdk_config::WorkspaceOptions,
    },
    compaction::CompactionConfig,
    config::Config,
    permission::{PermissionMode, SessionWriteLog},
};

/// Permission state one ACP session owns. Starts from the process config's
/// mode; `session/set_mode` changes it for this session only.
pub(in crate::app::acp) struct SessionPermission {
    pub(in crate::app::acp) mode: PermissionMode,
    /// Write grants the current policy remembers.
    pub(in crate::app::acp) writes: SessionWriteLog,
    /// Primary workspace root the session's tools resolve against.
    pub(in crate::app::acp) workspace_root: PathBuf,
}

impl SessionHost {
    /// Applies `mode` to later prompts. The caller holds the host slot lock,
    /// so no prompt is in flight. On error the session keeps its old mode.
    pub(in crate::app::acp) async fn set_permission_mode(
        &mut self,
        mode: PermissionMode,
        process_config: &Config,
    ) -> Result<(), AcpError> {
        if self.permission.mode == mode {
            return Ok(());
        }
        let mut config = model_scoped_config(&self.current_model(), process_config);
        config.permission_mode = mode;
        ensure_headless_auto_classifier_model(&config)
            .map_err(|error| AcpError::invalid_params().data(error.to_string()))?;

        let writes = self
            .permission
            .writes
            .clone()
            .carried_across(self.permission.mode, mode);
        let channel = approval_channel_for(
            mode,
            ApprovalChannelOptions {
                config: config.clone(),
                workspace_path: self.permission.workspace_root.clone(),
                usage_recording: self.built.runtime.usage_recording(),
                session_writes: writes.clone(),
            },
        );
        let workspace = WorkspaceOptions {
            root: self.permission.workspace_root.clone(),
        }
        .build_workspace(&self.added_dirs)
        .map_err(host_apply_error)?;
        let runtime = build_runtime(RuntimeBuildOptions {
            provider: std::sync::Arc::clone(&self.built.provider),
            tools: &self.built.tools,
            workspace,
            workspace_policy: AppPolicy::for_mode(mode, writes.clone()),
            approval_session: channel.handler.map(ApprovalSession::from_shared),
            system_prompt: self.built.prompt.system.clone(),
            reasoning: self.built.session.reasoning_level(),
            service_tier: self.built.session.service_tier(),
            compaction: CompactionConfig::from(&config),
            context_window: model_context_window(&config.provider, &config.model),
            usage_purpose: "agent",
            usage_parent_session_id: None,
            usage_recording: self.built.runtime.usage_recording(),
            hook_host_labels: rho_sdk::hooks::HookHostLabels::new(),
            hooks: self.built.hooks.as_ref(),
            diagnostics: self.built.diagnostics.clone(),
            recall: self.built.tools.recall_store(),
        })
        .map_err(host_apply_error)?;
        let session = match runtime
            .rebind_session(SessionOptions::from_snapshot(self.built.session.snapshot()))
            .await
        {
            Ok(session) => session,
            Err(error) => {
                runtime.shutdown();
                return Err(host_apply_error(error));
            }
        };

        let previous = std::mem::replace(&mut self.built.runtime, runtime);
        self.built.session = session;
        self.built.approval_receiver = channel.receiver;
        if let Some(advisor) = self.built.tools.advisor() {
            advisor.bind_session(self.built.session.clone());
        }
        if let Some(manager) = self.built.tools.subagents() {
            manager.update_permission_mode(mode);
        }
        previous.shutdown();
        self.permission = SessionPermission {
            mode,
            writes,
            workspace_root: std::mem::take(&mut self.permission.workspace_root),
        };
        Ok(())
    }
}

#[cfg(test)]
#[path = "session_host_mode_tests.rs"]
mod tests;
