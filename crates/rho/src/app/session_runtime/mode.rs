//! Permission mode changes for one session: swaps the mode by rebuilding the
//! SDK runtime around the live conversation, the same way the interactive
//! host does.

use std::path::PathBuf;

use rho_sdk::{SessionOptions, SessionSnapshot};

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
        // `exit_plan_mode` is offered only in plan mode; the rebuilt runtime
        // must see the tool list of the mode it serves.
        self.built.tools.set_plan_exit_registered(mode);
        if let Err(error) = self.rebuild_runtime(&config, mode, writes.clone()).await {
            self.built
                .tools
                .set_plan_exit_registered(self.permission.mode);
            return Err(error);
        }
        if let Some(manager) = self.built.tools.subagents() {
            manager.update_permission_mode(mode);
        }
        self.permission = SessionPermission {
            mode,
            writes,
            workspace_root: std::mem::take(&mut self.permission.workspace_root),
        };
        Ok(())
    }

    /// Rebuilds the SDK runtime around the live conversation and the tool
    /// set as it stands, with `mode`'s policy and approvals over `writes`
    /// whatever mode `config` names, keeping the session's run shape. The
    /// live runtime is replaced only once the new one holds the session, so
    /// on error the session keeps its runtime. Callers that change the tool
    /// list keep the system prompt and tell the model with an appended notice instead.
    pub(super) async fn rebuild_runtime(
        &mut self,
        config: &Config,
        mode: PermissionMode,
        writes: SessionWriteLog,
    ) -> Result<(), SettingChangeError> {
        let conversation = self.built.session.snapshot();
        self.rebuild_runtime_onto(config, mode, writes, conversation)
            .await
    }

    /// [`Self::rebuild_runtime`], around `conversation` in place of the
    /// live one.
    pub(super) async fn rebuild_runtime_onto(
        &mut self,
        config: &Config,
        mode: PermissionMode,
        writes: SessionWriteLog,
        conversation: SessionSnapshot,
    ) -> Result<(), SettingChangeError> {
        // Approvals follow the session's mode, which a caller's config,
        // built from the process config, may not carry.
        let mut config = config.clone();
        config.permission_mode = mode;
        let config = &config;
        let approval = self.build.host.approvals.wire(ApprovalInputs {
            config: config.clone(),
            workspace_root: self.permission.workspace_root.clone(),
            usage_recording: self.built.runtime.usage_recording(),
            session_writes: writes.clone(),
            credentials: self.build.host.environment.credentials(),
        });
        let workspace = WorkspaceOptions {
            root: self.permission.workspace_root.clone(),
        }
        .build_workspace(&self.added_dirs)
        .map_err(|error| SettingChangeError::Apply(error.into()))?;
        // The whole run shape, as when the session opened: a rebuild keeps
        // its step limit, attribution and hook labels.
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
                    writes,
                    self.build.host.environment.current().path(),
                ),
                approval_session: approval.session,
                system_prompt: self.built.prompt.system.clone(),
                reasoning: self.built.session.reasoning_level(),
                service_tier: self.built.session.service_tier(),
                compaction: CompactionConfig::from(config),
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
        let session = match runtime
            .rebind_session(SessionOptions::from_snapshot(conversation))
            .await
        {
            Ok(session) => session,
            Err(error) => {
                runtime.shutdown();
                return Err(SettingChangeError::Apply(error.into()));
            }
        };
        let previous = std::mem::replace(&mut self.built.runtime, runtime);
        self.built.session = session;
        self.built.approval_receiver = approval.receiver;
        if let Some(advisor) = self.built.tools.advisor() {
            advisor.bind_session(self.built.session.clone());
        }
        previous.shutdown();
        Ok(())
    }
}

#[cfg(test)]
#[path = "mode_tests.rs"]
mod tests;
