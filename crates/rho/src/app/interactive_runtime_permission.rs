//! Permission policy and host-controlled plan handoff for interactive sessions.

use super::*;

impl InteractiveRuntime {
    /// Rebuilds the SDK runtime so the requested permission mode applies to the next turn.
    pub(crate) async fn set_permission_mode(&mut self, mode: PermissionMode) -> anyhow::Result<()> {
        if self.is_session_busy() {
            anyhow::bail!(if self.runs.is_active() {
                "permission mode cannot change while a run is active"
            } else {
                "permission mode cannot change while compaction is active"
            });
        }
        if self.permission_mode == mode {
            return Ok(());
        }
        if mode == PermissionMode::Plan {
            self.revoke_computer_use();
        }

        let session_writes = self
            .session_writes
            .clone()
            .carried_across(self.permission_mode, mode);
        let snapshot = self.sessions.session().snapshot();
        let approval_channel = approval_channel_for(
            mode,
            ApprovalChannelOptions {
                config: self.config.clone(),
                workspace_path: self.workspace.root().to_path_buf(),
                usage_recording: self.usage_recording.clone(),
                session_writes: session_writes.clone(),
            },
        );
        self.tools.set_plan_exit_registered(mode);
        let replacement = async {
            let replacement_runtime = build_runtime(RuntimeBuildOptions {
                provider: Arc::clone(self.provider.provider()),
                tools: &self.tools,
                workspace: self.workspace.clone(),
                workspace_policy: AppPolicy::for_mode(mode, session_writes.clone()),
                approval_session: approval_channel
                    .handler
                    .clone()
                    .map(rho_sdk::ApprovalSession::from_shared),
                system_prompt: self.active_system_prompt(),
                reasoning: self.provider.reasoning(),
                service_tier: self.sessions.session().service_tier(),
                compaction: self.compaction.clone(),
                context_window: self.context_window,
                usage_purpose: "agent",
                usage_parent_session_id: None,
                usage_recording: self.usage_recording.clone(),
                hook_host_labels: rho_sdk::hooks::HookHostLabels::new(),
                hooks: self.hooks.as_ref(),
                diagnostics: self.diagnostics.clone(),
                recall: self.tools.recall_store(),
            })?;
            let replacement_session = replacement_runtime
                .rebind_session(SessionOptions::from_snapshot(snapshot))
                .await?;
            Ok::<_, anyhow::Error>((replacement_runtime, replacement_session))
        }
        .await;
        let (replacement_runtime, replacement_session) = match replacement {
            Ok(replacement) => replacement,
            Err(error) => {
                self.tools.set_plan_exit_registered(self.permission_mode);
                return Err(error);
            }
        };

        let previous_runtime = std::mem::replace(&mut self.runtime, replacement_runtime);
        self.sessions.replace_runtime_session(replacement_session);
        self.computer_runtime_dirty = false;
        self.permission_mode = mode;
        self.config.permission_mode = mode;
        self.session_writes = session_writes;
        self.approval_handler = approval_channel.handler;
        self.approval_receiver = approval_channel.receiver;
        self.classifier_approval_handler = approval_channel.classifier;
        if let Some(manager) = self.tools.subagents() {
            manager.update_permission_mode(mode);
        }
        previous_runtime.shutdown();
        self.remember_tool_list();
        Ok(())
    }

    pub(crate) fn take_plan_exit_decision(
        &self,
    ) -> Option<crate::tools::plan_exit::PlanExitDecision> {
        self.tools.take_plan_exit_decision()
    }

    pub(super) fn permission_for_rebuild(
        &self,
        writes: SessionWriteRetention,
    ) -> RebuiltPermission {
        match writes {
            SessionWriteRetention::Keep => RebuiltPermission {
                workspace_policy: self.workspace_policy(),
                approval_session: self
                    .approval_handler
                    .clone()
                    .map(ApprovalSession::from_shared),
                pending: None,
            },
            SessionWriteRetention::Forget => {
                let session_writes = crate::permission::SessionWriteLog::default();
                let channel = approval_channel_for(
                    self.permission_mode,
                    ApprovalChannelOptions {
                        config: self.config.clone(),
                        workspace_path: self.workspace.root().to_path_buf(),
                        usage_recording: self.usage_recording.clone(),
                        session_writes: session_writes.clone(),
                    },
                );
                RebuiltPermission {
                    workspace_policy: AppPolicy::for_mode(
                        self.permission_mode,
                        session_writes.clone(),
                    ),
                    approval_session: channel.handler.clone().map(ApprovalSession::from_shared),
                    pending: Some((session_writes, channel)),
                }
            }
        }
    }

    pub(super) fn install_rebuilt_permission(
        &mut self,
        pending: Option<(crate::permission::SessionWriteLog, startup::ApprovalChannel)>,
    ) {
        let Some((session_writes, channel)) = pending else {
            return;
        };
        self.session_writes = session_writes;
        self.approval_handler = channel.handler;
        self.approval_receiver = channel.receiver;
        self.classifier_approval_handler = channel.classifier;
    }
}

#[cfg(test)]
#[path = "interactive_runtime_plan_exit_tests.rs"]
mod tests;

pub(super) struct RebuiltPermission {
    pub(super) workspace_policy: AppPolicy,
    pub(super) approval_session: Option<ApprovalSession>,
    pub(super) pending: Option<(crate::permission::SessionWriteLog, startup::ApprovalChannel)>,
}
