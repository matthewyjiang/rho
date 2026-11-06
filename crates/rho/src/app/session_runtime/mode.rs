//! Permission mode changes for one session: swaps the mode by rebuilding the
//! SDK runtime around the live conversation, the same way the interactive
//! host does.

use std::path::PathBuf;

use rho_sdk::{ApprovalSession, SessionOptions};

use super::{model_context_window, model_scoped_config, SessionRuntime};
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

/// Permission state one session owns. Starts from the process config's
/// mode; a host's mode change applies to this session only.
pub(crate) struct SessionPermission {
    pub(crate) mode: PermissionMode,
    /// Write grants the current policy remembers.
    pub(crate) writes: SessionWriteLog,
    /// Primary workspace root the session's tools resolve against.
    pub(crate) workspace_root: PathBuf,
}

/// Why a permission mode change did not apply. The session keeps its old
/// mode either way.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ModeChangeError {
    /// The mode cannot run with this session's config, such as auto mode
    /// without a classifier model. The requester can fix it.
    #[error("{0}")]
    Unsupported(anyhow::Error),
    /// Rebuilding the SDK runtime for the mode failed.
    #[error("{0}")]
    Rebuild(anyhow::Error),
}

impl SessionRuntime {
    /// Applies `mode` to later prompts. The host holds the runtime between
    /// prompts, so none is in flight. On error the session keeps its old mode.
    pub(crate) async fn set_permission_mode(
        &mut self,
        mode: PermissionMode,
        process_config: &Config,
    ) -> Result<(), ModeChangeError> {
        if self.permission.mode == mode {
            return Ok(());
        }
        let mut config = model_scoped_config(&self.current_model(), process_config);
        config.permission_mode = mode;
        ensure_headless_auto_classifier_model(&config).map_err(ModeChangeError::Unsupported)?;

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
        // `exit_plan_mode` is offered only in plan mode; the rebuilt runtime
        // must see the tool list of the mode it serves.
        self.built.tools.set_plan_exit_registered(mode);
        let rebuilt = async {
            let workspace = WorkspaceOptions {
                root: self.permission.workspace_root.clone(),
            }
            .build_workspace(&self.added_dirs)
            .map_err(|error| ModeChangeError::Rebuild(error.into()))?;
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
            .map_err(|error| ModeChangeError::Rebuild(error.into()))?;
            match runtime
                .rebind_session(SessionOptions::from_snapshot(self.built.session.snapshot()))
                .await
            {
                Ok(session) => Ok((runtime, session)),
                Err(error) => {
                    runtime.shutdown();
                    Err(ModeChangeError::Rebuild(error.into()))
                }
            }
        }
        .await;
        let (runtime, session) = match rebuilt {
            Ok(rebuilt) => rebuilt,
            Err(error) => {
                self.built
                    .tools
                    .set_plan_exit_registered(self.permission.mode);
                return Err(error);
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
#[path = "mode_tests.rs"]
mod tests;
