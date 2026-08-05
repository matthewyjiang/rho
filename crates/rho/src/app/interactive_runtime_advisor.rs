//! Advisor mode as a runtime state transition.
//!
//! Advisor mode changes the tool list and the system prompt, neither of which
//! the SDK can swap on a live runtime. Turning it on or off therefore rebuilds
//! the runtime and rebinds the session, the same move a permission-mode change
//! makes, so the change lands on the next turn and the session ID and history
//! survive it.

use std::sync::Arc;

use rho_sdk::{SessionOptions, SystemPrompt};

use crate::config::InternalAgentModelConfig;

use super::super::{
    policy::AppPolicy,
    runtime_builder::{build_runtime, RuntimeBuildOptions},
};

use super::InteractiveRuntime;

impl InteractiveRuntime {
    /// The system prompt for the tools this run currently offers.
    pub(super) fn active_system_prompt(&self) -> SystemPrompt {
        self.system_prompt
            .for_advisor_mode(self.tools.advisor_registered())
    }

    /// Applies an advisor mode or advisor model change to the next turn.
    ///
    /// `model` is the advisor model to use, or `None` when advisor mode is off
    /// or has no model yet; those are the same thing to the executor. The live
    /// tool reads the new model at once. Registering or removing the `advisor`
    /// tool also changes the tool list and the system prompt, so it needs the
    /// same runtime rebuild as a permission-mode change; the session ID and
    /// history survive it.
    pub(crate) async fn set_advisor(
        &mut self,
        model: Option<InternalAgentModelConfig>,
    ) -> anyhow::Result<()> {
        let Some(store) = self.tools.advisor().cloned() else {
            return Ok(());
        };
        let registered = model.is_some();
        if registered == self.tools.advisor_registered() {
            store.set_model(model);
            return Ok(());
        }
        if self.runs.is_active() {
            anyhow::bail!("advisor mode cannot change while a run is active");
        }

        // The model lands only after the rebuild succeeds, so a failed
        // transition leaves both the tool list and the store untouched.
        self.tools.set_advisor_registered(registered);
        match self.rebind_current_session().await {
            Ok(()) => {
                store.set_model(model);
                Ok(())
            }
            Err(error) => {
                self.tools.set_advisor_registered(!registered);
                Err(error)
            }
        }
    }

    /// Rebuilds the SDK runtime around the current tools and prompt, then
    /// rebinds the live session onto it. The live runtime is replaced only
    /// after the replacement is ready, so a failure leaves the session intact.
    async fn rebind_current_session(&mut self) -> anyhow::Result<()> {
        let snapshot = self.sessions.session().snapshot();
        let replacement_runtime = build_runtime(RuntimeBuildOptions {
            provider: Arc::clone(self.provider.provider()),
            tools: self.tools.tools(),
            workspace: self.workspace.clone(),
            workspace_policy: AppPolicy::for_mode(self.permission_mode),
            approval_session: self
                .approval_handler
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
        })?;
        let replacement_session = replacement_runtime
            .rebind_session(SessionOptions::from_snapshot(snapshot))
            .await?;

        let previous_runtime = std::mem::replace(&mut self.runtime, replacement_runtime);
        self.sessions.replace_runtime_session(replacement_session);
        previous_runtime.shutdown();
        Ok(())
    }
}
