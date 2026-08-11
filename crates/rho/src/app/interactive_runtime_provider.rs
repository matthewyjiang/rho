//! Conversation provider/model switches on a live interactive runtime.
//!
//! Replacing the provider is a multi-step transition: hand off the session,
//! rebuild compaction, tell the model when the conversation model changed, and
//! keep MCP sampling on the live selection. Post-replace failures roll the
//! provider back so `Err` means the active provider is unchanged whenever
//! restore itself succeeds.

use std::sync::Arc;

use rho_sdk::{provider::ModelProvider, Error};

use super::{
    active_run_disposition, startup, ActiveRunCommand, ActiveRunDisposition, InteractiveRuntime,
};

impl InteractiveRuntime {
    pub(crate) fn replace_provider(
        &mut self,
        provider: Arc<dyn ModelProvider>,
        reasoning: rho_sdk::ReasoningLevel,
        auth: &str,
    ) -> Result<rho_sdk::model::handoff::HandoffReport, Error> {
        if self.runs.is_active() {
            debug_assert_eq!(
                active_run_disposition(ActiveRunCommand::ReplaceProvider),
                ActiveRunDisposition::DeferUntilFinished
            );
            return Err(Error::SessionBusy);
        }
        self.runs.begin_provider_switch()?;
        // Capture prior identity so post-replace failures can roll back and keep
        // `Err` meaning "active provider unchanged" for callers.
        let previous_provider = Arc::clone(self.provider.provider());
        let previous_reasoning = self.provider.reasoning();
        let previous_prompt_model =
            crate::model_identity::PromptModel::from_sdk_identity(&previous_provider.identity());
        // A first selection on an empty session is not a switch: the system
        // prompt has yet to be built and will name the chosen model itself.
        let session_started = !self.history().is_empty();
        let report = match self
            .provider
            .replace(self.sessions.session(), provider, reasoning)
        {
            Ok(report) => report,
            Err(error) => {
                self.runs.finish_transition();
                return Err(error);
            }
        };
        if let Err(error) = self.refresh_compaction() {
            let error = self.fail_after_provider_restore(
                previous_provider,
                previous_reasoning,
                error,
                RestoreCompaction::Skip,
            );
            self.runs.finish_transition();
            return Err(error);
        }

        let identity = self.provider.provider().identity();
        let current_prompt_model = crate::model_identity::PromptModel::from_sdk_identity(&identity);
        // The system prompt named the model this session started on and then
        // stayed fixed, so a later switch has to reach the model as context.
        // Owned here (not in the TUI) so every conversation model change is
        // honest, and a failed notice rolls the provider back.
        if session_started && current_prompt_model != previous_prompt_model {
            let (context, display) = crate::prompt::model_switch_context(
                crate::prompt::ModelSwitchKind::Conversation,
                &current_prompt_model,
            );
            if let Err(error) = self.append_user_context_with_display(context, display) {
                let error = Error::InvalidConfiguration {
                    message: format!(
                        "could not record the conversation model switch for the model: {error}"
                    ),
                };
                // Compaction was rebuilt for the new provider; put it back with
                // the restored provider, or report that rollback is incomplete.
                let error = self.fail_after_provider_restore(
                    previous_provider,
                    previous_reasoning,
                    error,
                    RestoreCompaction::Required,
                );
                self.runs.finish_transition();
                return Err(error);
            }
        }

        if let Some(manager) = self.tools.subagents() {
            manager.update_selection(&identity.provider, &identity.model, reasoning, auth);
        }
        // MCP sampling must follow the user's current model, never the one that
        // happened to be selected when the servers connected.
        startup::bind_mcp_sampling(
            &self.mcp_sampling,
            self.provider.provider(),
            self.sessions.session().id(),
            self.workspace.root(),
        );
        self.invalidate_live_context();
        self.runs.finish_transition();
        Ok(report)
    }

    /// Rolls the provider back after a post-replace step failed, optionally
    /// rebuilding compaction for the restored provider.
    ///
    /// Always returns an error for the caller to surface. When restore succeeds,
    /// that error is `primary` (active provider unchanged). When restore or the
    /// optional compaction rebuild fails, the error describes the incomplete
    /// rollback.
    fn fail_after_provider_restore(
        &mut self,
        previous_provider: Arc<dyn ModelProvider>,
        previous_reasoning: rho_sdk::ReasoningLevel,
        primary: Error,
        compaction: RestoreCompaction,
    ) -> Error {
        if let Err(rollback_error) = self.provider.replace(
            self.sessions.session(),
            previous_provider,
            previous_reasoning,
        ) {
            return Error::InvalidConfiguration {
                message: format!(
                    "{primary}; also failed to restore the previous provider: {rollback_error}"
                ),
            };
        }
        if matches!(compaction, RestoreCompaction::Required) {
            if let Err(refresh_error) = self.refresh_compaction() {
                return Error::InvalidConfiguration {
                    message: format!(
                        "{primary}; could not restore compaction for the previous provider: {refresh_error}"
                    ),
                };
            }
        }
        primary
    }
}

/// Whether a failed post-replace step must rebuild compaction after the
/// provider is restored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RestoreCompaction {
    /// Compaction never left the previous provider (e.g. the first rebuild
    /// failed before the new one was installed).
    Skip,
    /// Compaction was rebuilt for the rejected provider and must follow the
    /// restore.
    Required,
}
