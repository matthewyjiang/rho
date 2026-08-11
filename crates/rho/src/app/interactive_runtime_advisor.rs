//! Advisor mode as a runtime state transition.
//!
//! Advisor mode changes the advertised tool list, which the SDK cannot swap on
//! a live runtime. Turning it on or off therefore rebuilds the runtime and
//! rebinds the session so the change lands on the next turn. The session ID and
//! history survive it.
//!
//! The system prompt stays fixed for prompt-cache stability. The model learns
//! about the tool list change from an appended context notice (with the tool
//! schema when enabling) rather than a rewritten system prompt.

use std::sync::Arc;

use rho_sdk::{SessionOptions, SystemPrompt};

use crate::config::InternalAgentModelConfig;

use super::super::{
    policy::AppPolicy,
    runtime_builder::{build_runtime, RuntimeBuildOptions},
};

use super::InteractiveRuntime;

#[cfg(test)]
thread_local! {
    /// When set, the next advisor notice appends model-visible history, then
    /// fails snapshot persistence so rollback must cover the partial commit.
    static FAIL_NEXT_ADVISOR_NOTICE_SNAPSHOT_SAVE: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(crate) fn fail_next_advisor_switch_notice_for_tests() {
    FAIL_NEXT_ADVISOR_NOTICE_SNAPSHOT_SAVE.with(|flag| flag.set(true));
}

#[cfg(test)]
pub(super) fn take_fail_next_advisor_notice_snapshot_save_for_tests() -> bool {
    FAIL_NEXT_ADVISOR_NOTICE_SNAPSHOT_SAVE.with(|flag| flag.replace(false))
}

impl InteractiveRuntime {
    /// Fixed system prompt for this session.
    ///
    /// Mid-session tool list changes keep this value stable and tell the model
    /// through appended context instead.
    pub(super) fn active_system_prompt(&self) -> SystemPrompt {
        self.system_prompt.clone()
    }

    /// Applies an advisor mode or advisor model change to the next turn.
    ///
    /// `model` is the advisor model to use, or `None` when advisor mode is off
    /// or has no model yet; those are the same thing to the executor. The live
    /// tool reads the new model at once. Registering or removing the `advisor`
    /// tool rebuilds the runtime without rewriting the system prompt, then
    /// appends a context notice. A model-only change while advisor stays on
    /// appends a switch notice without rebuilding. Returns display text for a
    /// transcript notice when one was appended.
    pub(crate) async fn set_advisor(
        &mut self,
        model: Option<InternalAgentModelConfig>,
    ) -> anyhow::Result<Option<String>> {
        let Some(store) = self.tools.advisor().cloned() else {
            return Ok(None);
        };
        let registered = model.is_some();
        if registered == self.tools.advisor_registered() {
            // The tool list is unchanged, so nothing rebuilds and nothing else
            // would say the reviewer behind `advisor` is a different model.
            //
            // Compare what the notice reports, not the whole selection: a
            // reasoning-only change would otherwise announce a switch to the
            // model the advisor already used.
            let previous_model = store.model();
            let previous_identity = previous_model
                .as_ref()
                .map(crate::model_identity::PromptModel::from_internal_agent);
            let notice = model
                .as_ref()
                .map(crate::model_identity::PromptModel::from_internal_agent)
                .filter(|identity| previous_identity.as_ref() != Some(identity))
                .map(|identity| {
                    crate::prompt::model_switch_context(
                        crate::prompt::ModelSwitchKind::Advisor,
                        &identity,
                    )
                });
            store.set_model(model);
            let Some((context, display)) = notice else {
                return Ok(None);
            };
            if let Err(error) = self.append_user_context_with_display(context, display.clone()) {
                // Same rule as the transition below: the store must not hold a
                // reviewer the executor was never told about.
                store.set_model(previous_model);
                return Err(error);
            }
            return Ok(Some(display));
        }
        if self.runs.is_active() {
            anyhow::bail!("advisor mode cannot change while a run is active");
        }

        // The model lands only after the rebuild succeeds, so a failed
        // transition leaves both the tool list and the store untouched.
        let previous_registered = self.tools.advisor_registered();
        let previous_model = store.model();
        let history_before = self.sessions.history();
        self.tools.set_advisor_registered(registered);
        match self.rebind_current_session().await {
            Ok(()) => {
                store.set_model(model);
                // After `set_model`, so the enable notice names the model the
                // tool will actually consult.
                match self.append_advisor_switch_notice(registered) {
                    Ok(display) => Ok(Some(display)),
                    Err(error) => {
                        // Mirror edit-tool: a notice failure must not leave the
                        // session advertising a tool list the model was never
                        // told about. Also restore model-visible history when a
                        // partial append-before-save left a notice in place.
                        store.set_model(previous_model);
                        self.tools.set_advisor_registered(previous_registered);
                        if self.sessions.history() != history_before {
                            let _ = self.sessions.session().replace_history(history_before);
                        }
                        let _ = self.rebind_current_session().await;
                        Err(error)
                    }
                }
            }
            Err(error) => {
                self.tools.set_advisor_registered(previous_registered);
                Err(error)
            }
        }
    }

    fn append_advisor_switch_notice(&mut self, enabled: bool) -> anyhow::Result<String> {
        let (model, display) = if enabled {
            let spec = self
                .tools
                .specs()
                .into_iter()
                .find(|spec| spec.name == crate::tools::advisor::TOOL_NAME)
                .ok_or_else(|| {
                    anyhow::anyhow!("advisor tool is missing after it was registered")
                })?;
            let reviewer = self
                .tools
                .advisor()
                .and_then(crate::tools::advisor::AdvisorSessionStore::model)
                .ok_or_else(|| {
                    anyhow::anyhow!("advisor tool is registered without an advisor model")
                })?;
            crate::prompt::advisor_enabled_context(
                &spec,
                &crate::model_identity::PromptModel::from_internal_agent(&reviewer),
            )
        } else {
            crate::prompt::advisor_disabled_context()
        };
        self.append_user_context_with_display(model, display.clone())?;
        Ok(display)
    }

    /// Rebuilds the SDK runtime around the current tools and prompt, then
    /// rebinds the live session onto it. The live runtime is replaced only
    /// after the replacement is ready, so a failure leaves the session intact.
    ///
    /// Callers that change the advertised tool list should keep the system
    /// prompt fixed for prompt-cache stability and tell the model about the
    /// change with an appended context message instead.
    pub(super) async fn rebind_current_session(&mut self) -> anyhow::Result<()> {
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
