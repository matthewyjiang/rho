//! Conversation provider/model switches on a live interactive runtime.
//!
//! Session-level replace, reasoning, compaction, notice, and delegated
//! selection live in [`crate::app::conversation_switch`]. This wrapper adds
//! TUI display history, MCP sampling, and run-transition. Post-replace
//! failures roll the provider back so `Err` means the active provider is
//! unchanged whenever restore itself succeeds.

use std::sync::Arc;

use rho_sdk::{provider::ModelProvider, Error};

use crate::app::conversation_switch;

use super::{
    active_run_disposition, startup, ActiveRunCommand, ActiveRunDisposition, InteractiveRuntime,
};

impl InteractiveRuntime {
    pub(crate) fn model_prompt_notice(&self) -> Option<String> {
        self.prompt_template.as_ref().map(|_| {
            crate::app::model_prompt_metadata::provenance_notice(
                self.sessions.prompt.provenance.as_ref(),
            )
        })
    }

    pub(crate) fn replace_provider(
        &mut self,
        provider: Arc<dyn ModelProvider>,
        reasoning: rho_sdk::ReasoningLevel,
        auth: &str,
    ) -> Result<rho_sdk::model::handoff::HandoffReport, Error> {
        if self.is_session_busy() {
            if self.runs.is_active() {
                debug_assert_eq!(
                    active_run_disposition(ActiveRunCommand::ReplaceProvider),
                    ActiveRunDisposition::DeferUntilFinished
                );
            }
            return Err(Error::SessionBusy);
        }
        let prepared_prompt = self.prepare_model_prompt(&provider)?;
        self.runs.begin_provider_switch()?;
        let previous_prompt = self.sessions.prompt.clone();
        let previous_hydration = self.may_rewrite_startup_prompt;
        // The notice persists the switched history. Stage the same typed prompt
        // before that save, and roll it back with the conversation on failure.
        if let Some(prompt) = prepared_prompt.as_ref() {
            self.adopt_prompt(crate::app::active_prompt::ActivePrompt::new(
                rho_sdk::SystemPrompt::Custom(prompt.text.clone()),
                prompt.model_prompt.clone(),
                prompt.sources.clone(),
            ));
        }
        let previous_provider = Arc::clone(self.provider.provider());
        let context_window = self.context_window;
        let mut record_notice = |context: String, display: String| {
            InteractiveRuntime::record_user_context_with_display(&self.sessions, context, display)
                .map_err(|error| Error::InvalidConfiguration {
                    message: error.to_string(),
                })
        };
        let result = conversation_switch::apply_conversation_switch(
            conversation_switch::ConversationSwitch {
                prepared_prompt: prepared_prompt.as_ref(),
                session: self.sessions.session(),
                tools: &self.tools,
                previous_provider,
                new_provider: Arc::clone(&provider),
                new_reasoning: reasoning,
                auth,
                compaction: self.compaction.clone(),
                context_window,
                previous_context_window: context_window,
                usage_recording: self.usage_recording.clone(),
            },
            conversation_switch::SwitchNotice::WithDisplay(&mut record_notice),
        );
        match result {
            Ok(report) => {
                self.provider.adopt(provider, reasoning);
                self.refresh_context_usage();
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
            Err(error) => {
                self.adopt_prompt(previous_prompt);
                self.may_rewrite_startup_prompt = previous_hydration;
                self.runs.finish_transition();
                Err(error)
            }
        }
    }

    pub(super) fn prepare_model_prompt(
        &self,
        provider: &Arc<dyn ModelProvider>,
    ) -> Result<Option<crate::prompt::SystemPrompt>, Error> {
        self.prompt_template
            .as_ref()
            .map(|template| {
                template.build(&crate::model_identity::PromptModel::from_sdk_identity(
                    &provider.identity(),
                ))
            })
            .transpose()
            .map_err(|error| Error::InvalidConfiguration {
                message: format!("could not load model prompt: {error:#}"),
            })
    }

    pub(super) fn adopt_model_prompt(&mut self, prompt: crate::prompt::SystemPrompt) {
        self.adopt_prompt(crate::app::active_prompt::ActivePrompt::from_prepared(
            prompt,
        ));
    }

    pub(super) fn adopt_prompt(&mut self, prompt: crate::app::active_prompt::ActivePrompt) {
        self.sessions
            .prompt
            .adopt(prompt, &self.diagnostics, self.tools.advisor());
        // A resume, switch or recovery ends startup hydration's authority to
        // rewrite history. In particular, restored provenance has no file body.
        self.may_rewrite_startup_prompt = false;
    }
}
