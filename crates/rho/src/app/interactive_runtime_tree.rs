//! Selecting durable conversation branches without changing the live session
//! until the target history and its refreshed prompt are ready.

use super::permission::RebuiltPermission;
use super::*;

/// Prepared replacement owned by the runtime, not by a UI transaction.
/// Dropping an uncommitted selection shuts down the unused SDK runtime.
pub(super) struct PreparedTreeSelection {
    storage: StoredSession,
    target_id: crate::session::tree::NodeId,
    runtime: Option<Rho>,
    session: rho_sdk::Session,
    resume_omission: Option<rho_sdk::model::handoff::HandoffReport>,
    permission: RebuiltPermission,
    prompt: Option<crate::prompt::SystemPrompt>,
    /// Staged template; adopted only when the selection commits.
    template: Option<crate::prompt::ModelPromptTemplate>,
    prompt_notice: Option<String>,
    /// The target session's added directories and the workspace granting them.
    added_dirs: crate::added_dirs::AddedDirs,
    missing_dirs: Vec<std::path::PathBuf>,
    workspace: Workspace,
}

impl Drop for PreparedTreeSelection {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown();
        }
    }
}

impl InteractiveRuntime {
    pub(crate) async fn select_tree_node(
        &mut self,
        storage: StoredSession,
        target_id: &crate::session::tree::NodeId,
    ) -> anyhow::Result<()> {
        let prepared = self.prepare_tree_selection(storage, target_id).await?;
        self.commit_tree_selection(prepared).await
    }

    pub(super) async fn prepare_tree_selection(
        &self,
        storage: StoredSession,
        target_id: &crate::session::tree::NodeId,
    ) -> anyhow::Result<PreparedTreeSelection> {
        if self.is_session_busy() {
            anyhow::bail!(if self.runs.is_active() {
                "cannot navigate the session tree while a run is active"
            } else {
                "cannot navigate the session tree while compaction is active"
            });
        }
        let identity = self.provider.provider().identity();
        let id = storage.id().to_string();
        let snapshot =
            storage.snapshot_for_node(target_id, identity.clone(), prompt_cache_key(&id))?;
        let resume_omission = resume_omissions_report(&snapshot, &identity);
        let same_session = self
            .sessions
            .storage()
            .is_some_and(|current| current.id() == storage.id());
        // Added directories belong to the session, not a node, and the target
        // node may predate `/add-dir`. Moving within this session keeps the
        // live set; entering another session reads its active leaf, the same
        // source `/resume` uses.
        let (added_dirs, missing_dirs) = if same_session {
            (self.sessions.added_dirs().clone(), Vec::new())
        } else {
            let active = storage.snapshot_for_resume(identity.clone(), prompt_cache_key(&id))?;
            self.starting_added_dirs(Some(&active))
        };
        let workspace = self.workspace_with(&added_dirs)?;
        let prepared =
            self.prepare_session_prompt(self.prompt_session(storage.id()), &added_dirs)?;
        let prepared_prompt = prepared.prompt;
        let permission = self.permission_for_rebuild(if same_session {
            SessionWriteRetention::Keep
        } else {
            SessionWriteRetention::Forget
        });
        let replacement_runtime = build_runtime(RuntimeBuildOptions {
            provider: Arc::clone(self.provider.provider()),
            tools: &self.tools,
            workspace: workspace.clone(),
            workspace_policy: permission.workspace_policy.clone(),
            approval_session: permission.approval_session.clone(),
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
        let replacement_session = match replacement_runtime
            .rebind_session(SessionOptions::from_snapshot(snapshot))
            .await
        {
            Ok(session) => session,
            Err(error) => {
                replacement_runtime.shutdown();
                return Err(error.into());
            }
        };
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
        Ok(PreparedTreeSelection {
            storage,
            target_id: target_id.clone(),
            runtime: Some(replacement_runtime),
            session: replacement_session,
            resume_omission,
            permission,
            prompt: prepared_prompt,
            template: prepared.template,
            prompt_notice,
            added_dirs,
            missing_dirs,
            workspace,
        })
    }

    pub(super) async fn commit_tree_selection(
        &mut self,
        mut prepared: PreparedTreeSelection,
    ) -> anyhow::Result<()> {
        // Only the durable leaf commit can fail here; prompt/snapshot/runtime
        // preparation has already succeeded without touching the live session.
        prepared.storage.set_leaf(&prepared.target_id)?;
        self.revoke_computer_use();
        let replacement_runtime = prepared
            .runtime
            .take()
            .expect("prepared runtime is present");
        let previous_runtime = std::mem::replace(&mut self.runtime, replacement_runtime);
        self.sessions
            .replace_session(prepared.session.clone(), prepared.resume_omission.take());
        self.computer_runtime_dirty = false;
        self.sessions.set_resumed_storage(prepared.storage.clone());
        self.install_rebuilt_permission(prepared.permission.pending.take());
        if let Some(prompt) = prepared.prompt.take() {
            self.adopt_model_prompt(prompt);
        }
        self.prompt_template = prepared.template.take();
        if let Some(notice) = prepared.prompt_notice.take() {
            self.sessions.queue_notice(notice);
        }
        self.workspace = prepared.workspace.clone();
        self.adopt_added_dirs(std::mem::take(&mut prepared.added_dirs));
        let missing_dirs = std::mem::take(&mut prepared.missing_dirs);
        self.queue_missing_added_dirs_notice(&missing_dirs);
        previous_runtime.shutdown();
        self.invalidate_live_context();
        self.refresh_context_usage();
        self.restore_computer_preference(computer::ComputerPreferenceSource::SavedSession)
            .await;
        Ok(())
    }
}
