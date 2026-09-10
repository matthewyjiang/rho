//! Selecting durable conversation branches without changing the live session
//! until the target history and its refreshed prompt are ready.

use super::*;

impl InteractiveRuntime {
    pub(crate) async fn select_tree_node(
        &mut self,
        storage: StoredSession,
        target_id: &crate::session::tree::NodeId,
    ) -> anyhow::Result<()> {
        if self.is_session_busy() {
            anyhow::bail!(if self.runs.is_active() {
                "cannot navigate the session tree while a run is active"
            } else {
                "cannot navigate the session tree while compaction is active"
            });
        }
        let identity = self.provider.provider().identity();
        let id = storage.id().to_string();
        let prepared_prompt = self.prepare_model_prompt(self.provider.provider())?;
        let snapshot =
            storage.snapshot_for_node(target_id, identity.clone(), prompt_cache_key(&id))?;
        let resume_omission = resume_omissions_report(&snapshot, &identity);
        let same_session = self
            .sessions
            .storage()
            .is_some_and(|current| current.id() == storage.id());
        let permission = self.permission_for_rebuild(if same_session {
            SessionWriteRetention::Keep
        } else {
            SessionWriteRetention::Forget
        });
        let replacement_runtime = build_runtime(RuntimeBuildOptions {
            provider: Arc::clone(self.provider.provider()),
            tools: self.tools.tools(),
            workspace: self.workspace.clone(),
            workspace_policy: permission.workspace_policy,
            approval_session: permission.approval_session,
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
        })?;
        let replacement_session = replacement_runtime
            .rebind_session(SessionOptions::from_snapshot(snapshot))
            .await?;
        let prompt_notice = prepared_prompt.as_ref().and_then(|prompt| {
            crate::app::model_prompt_metadata::change_notice(
                &replacement_session.snapshot(),
                prompt.model_prompt.as_ref(),
            )
        });
        if let Some(prompt) = prepared_prompt.as_ref() {
            if let Err(error) = crate::app::conversation_switch::replace_system_prompt(
                &replacement_session,
                &prompt.text,
            ) {
                replacement_runtime.shutdown();
                return Err(error.into());
            }
        }
        // Do not change the live runtime until the selected leaf is durable.
        if let Err(error) = storage.set_leaf(target_id) {
            replacement_runtime.shutdown();
            return Err(error);
        }
        self.revoke_computer_use();
        let previous_runtime = std::mem::replace(&mut self.runtime, replacement_runtime);
        self.sessions
            .replace_session(replacement_session, resume_omission);
        self.computer_runtime_dirty = false;
        self.sessions.set_resumed_storage(storage);
        self.install_rebuilt_permission(permission.pending);
        if let Some(prompt) = prepared_prompt {
            self.adopt_model_prompt(prompt);
        }
        if let Some(notice) = prompt_notice {
            self.sessions.queue_notice(notice);
        }
        previous_runtime.shutdown();
        self.invalidate_live_context();
        self.refresh_context_usage();
        self.restore_computer_preference(computer::ComputerPreferenceSource::SavedSession)
            .await;
        Ok(())
    }
}
