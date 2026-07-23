//! Background polling for model metadata, update notices, and idle subagents.

use futures_util::FutureExt;
use ratatui::DefaultTerminal;
use rho_providers::model::models_dev::fetch_model_metadata;
use rho_providers::model::ReasoningRequestSource::PersistedOrDefault;

use crate::credential_store::build_provider;

use super::{
    reasoning_metadata, turn_prompt::TurnPrompt, App, Entry, InteractiveRuntime, TurnOutcome,
};

impl App {
    pub(super) fn poll_update_notice(&mut self) {
        let Some(handle) = self.pending_update_notice.as_mut() else {
            return;
        };
        let Some(result) = handle.now_or_never() else {
            return;
        };
        self.pending_update_notice = None;
        if let Ok(Some(notice)) = result {
            self.info.services.update_notice = Some(notice);
        }
    }

    /// Wakes an idle session with a turn for finished background subagents.
    /// Real prompt turns drain these notifications themselves, while active
    /// goals deliver them before evaluating the goal again.
    pub(super) async fn poll_subagent_completions(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        if !self.should_deliver_idle_subagent_completions() {
            return Ok(false);
        }
        Ok(self
            .run_subagent_completion_turn(terminal, agent)
            .await?
            .is_some())
    }

    pub(super) async fn run_subagent_completion_turn(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<Option<TurnOutcome>> {
        let Some(manager) = agent.subagents().cloned() else {
            return Ok(None);
        };
        let notifications = manager.take_notifications(agent.session_id().as_str());
        if notifications.is_empty() {
            return Ok(None);
        }
        // The whole drained batch is one message and one model request, no
        // matter how many runs finished while the parent was busy.
        let (model_prompt, display_prompt) =
            crate::tools::agent::notification_prompts(&notifications);
        self.run_prompt_turn(
            TurnPrompt::standard(model_prompt, display_prompt),
            Vec::new(),
            terminal,
            agent,
        )
        .await
        .map(Some)
    }

    pub(super) fn should_deliver_idle_subagent_completions(&self) -> bool {
        !self.is_ui_busy() && self.goal.is_none() && self.queued_prompts.is_empty()
    }

    pub(super) fn start_model_metadata_fetch(&mut self, agent: &mut InteractiveRuntime) {
        if let Some(handle) = self.pending_model_metadata.take() {
            handle.abort();
        }
        self.pending_model_metadata_reasoning = None;
        if let Some((metadata, metadata_is_current)) = reasoning_metadata::cached_metadata(
            &self.info.runtime.provider,
            &self.info.runtime.model,
        ) {
            agent.set_context_window(metadata.display_context_window());
            let reasoning_metadata_complete = metadata.reasoning_metadata_complete;
            self.model_metadata = Some(metadata);
            if reasoning_metadata_complete && metadata_is_current {
                return;
            }
        } else {
            agent.set_context_window(None);
            self.model_metadata = None;
        }
        let provider = self.info.runtime.provider.clone();
        let model = self.info.runtime.model.clone();
        self.pending_model_metadata_reasoning = Some((
            self.info.runtime.reasoning,
            self.info.runtime.reasoning_source,
        ));
        self.pending_model_metadata = Some(tokio::spawn(async move {
            fetch_model_metadata(&provider, &model).await
        }));
    }

    pub(super) fn poll_model_metadata_fetch(&mut self, agent: &mut InteractiveRuntime) {
        let Some(handle) = self.pending_model_metadata.as_mut() else {
            return;
        };
        if !handle.is_finished() {
            return;
        }
        if let Some(handle) = self.pending_model_metadata.take() {
            let reasoning_at_fetch_start = self.pending_model_metadata_reasoning.take();
            if let Some(Ok(Some(metadata))) = handle.now_or_never() {
                agent.set_context_window(metadata.display_context_window());
                let capabilities = metadata.reasoning_capabilities();
                let resolved = reasoning_metadata::resolve_fetched_reasoning(
                    &capabilities,
                    self.info.runtime.reasoning,
                    reasoning_at_fetch_start,
                );
                let reasoning = resolved.effective;
                if let Some(requested) = resolved.rejected {
                    self.insert_entry(&Entry::Error(format!(
                        "reasoning level '{requested}' is not supported by {}/{}; restored '{reasoning}'",
                        self.info.runtime.provider, self.info.runtime.model
                    )));
                }
                let provider_updated = match build_provider(
                    &self.info.runtime.provider,
                    &self.info.runtime.model,
                    reasoning,
                ) {
                    Ok(provider) => match agent.replace_provider(provider, reasoning) {
                        Ok(_) => true,
                        Err(err) => {
                            self.insert_entry(&Entry::Error(format!(
                                "could not apply model reasoning metadata: {err}"
                            )));
                            false
                        }
                    },
                    Err(err) => {
                        self.insert_entry(&Entry::Error(format!(
                            "could not apply model reasoning metadata: {err}"
                        )));
                        false
                    }
                };
                if provider_updated && reasoning != self.info.runtime.reasoning {
                    self.info.set_reasoning(reasoning, PersistedOrDefault);
                    if let Err(err) = self.info.services.config_repository.update(|config| {
                        config.reasoning = reasoning;
                    }) {
                        self.insert_entry(&Entry::Error(format!(
                            "could not save normalized reasoning: {err}"
                        )));
                    }
                }
                self.model_metadata = Some(metadata);
            }
        }
    }
}
