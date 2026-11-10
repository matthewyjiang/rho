//! Permission mode changes for one session: swaps the mode by rebuilding the
//! SDK runtime around the live conversation, the same way the interactive
//! host does.

use std::path::PathBuf;

use rho_sdk::SessionOptions;

use super::{
    ensure_auto_classifier_model, model_context_window, model_scoped_config, RunShape,
    SessionRuntime, SettingChangeError,
};
use crate::{
    app::{
        policy::AppPolicy,
        runtime_builder::{build_runtime_with_max_steps, RuntimeBuildOptions},
        sdk_config::WorkspaceOptions,
        session_assembly::ApprovalInputs,
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

impl SessionRuntime {
    /// Applies `mode` to later prompts. The host holds the runtime between
    /// prompts, so none is in flight. On error the session keeps its old
    /// mode: a mode this session's config cannot run, such as auto without
    /// a classifier model, is [`SettingChangeError::Unsupported`].
    pub(crate) async fn set_permission_mode(
        &mut self,
        mode: PermissionMode,
        process_config: &Config,
    ) -> Result<(), SettingChangeError> {
        if self.permission.mode == mode {
            return Ok(());
        }
        let mut config = model_scoped_config(&self.current_model(), process_config);
        config.permission_mode = mode;
        let credentials = self.build.host.environment.credentials();
        ensure_auto_classifier_model(&config, credentials.as_ref())
            .map_err(|error| SettingChangeError::Unsupported(error.to_string()))?;

        let writes = self
            .permission
            .writes
            .clone()
            .carried_across(self.permission.mode, mode);
        let approval = self.build.host.approvals.wire(ApprovalInputs {
            config: config.clone(),
            workspace_root: self.permission.workspace_root.clone(),
            usage_recording: self.built.runtime.usage_recording(),
            session_writes: writes.clone(),
            credentials,
        });
        // `exit_plan_mode` is offered only in plan mode; the rebuilt runtime
        // must see the tool list of the mode it serves.
        self.built.tools.set_plan_exit_registered(mode);
        let rebuilt = async {
            let workspace = WorkspaceOptions {
                root: self.permission.workspace_root.clone(),
            }
            .build_workspace(&self.added_dirs)
            .map_err(|error| SettingChangeError::Apply(error.into()))?;
            // The whole run shape, as when the session opened: a mode change
            // keeps its step limit, attribution and hook labels.
            let RunShape {
                role,
                max_steps,
                hook_labels,
            } = &self.build.run;
            let runtime = build_runtime_with_max_steps(
                RuntimeBuildOptions {
                    provider: std::sync::Arc::clone(&self.built.provider),
                    tools: &self.built.tools,
                    workspace,
                    workspace_policy: AppPolicy::for_mode(
                        mode,
                        writes.clone(),
                        self.build.host.environment.current().path(),
                    ),
                    approval_session: approval.session,
                    system_prompt: self.built.prompt.system.clone(),
                    reasoning: self.built.session.reasoning_level(),
                    service_tier: self.built.session.service_tier(),
                    compaction: CompactionConfig::from(&config),
                    context_window: model_context_window(&config.provider, &config.model),
                    usage_purpose: role.usage_purpose(),
                    usage_parent_session_id: role.parent().cloned(),
                    usage_recording: self.built.runtime.usage_recording(),
                    hook_host_labels: hook_labels.clone(),
                    hooks: self.built.hooks.as_ref(),
                    diagnostics: self.built.diagnostics.clone(),
                    recall: self.built.tools.recall_store(),
                },
                *max_steps,
            )
            .map_err(|error| SettingChangeError::Apply(error.into()))?;
            match runtime
                .rebind_session(SessionOptions::from_snapshot(self.built.session.snapshot()))
                .await
            {
                Ok(session) => Ok((runtime, session)),
                Err(error) => {
                    runtime.shutdown();
                    Err(SettingChangeError::Apply(error.into()))
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
        self.built.approval_receiver = approval.receiver;
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
