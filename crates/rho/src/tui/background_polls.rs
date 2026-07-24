//! Background polling for model metadata, update notices, and idle subagents.

use futures_util::FutureExt;
use ratatui::DefaultTerminal;
use rho_providers::model::models_dev::fetch_model_metadata;
use rho_providers::model::ReasoningRequestSource::PersistedOrDefault;
use tokio::sync::oneshot;

use crate::credential_store::build_provider;

use super::{
    event_adapter, questionnaire::QuestionnaireResponseChannel, reasoning_metadata,
    turn_prompt::TurnPrompt, App, ComposerMode, Entry, InteractiveRuntime,
    PendingSubagentQuestionnaire, QuestionAnswerRequest, QuestionnaireReply, TurnOutcome,
};

#[derive(Clone, Copy)]
enum ParentActivity {
    Idle,
    Running,
}

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

    /// Surfaces delegated questionnaires when the parent can take user input.
    pub(super) async fn poll_subagent_questionnaires(
        &mut self,
        session_id: &rho_sdk::SessionId,
    ) -> anyhow::Result<bool> {
        let mut changed = self.drain_subagent_host_input();
        changed |= self.discard_stale_subagent_questionnaires(session_id);
        changed |= self
            .finish_pending_subagent_questionnaire(ParentActivity::Idle)
            .await?;
        if self.can_present_subagent_questionnaire() {
            changed |= self.present_next_subagent_questionnaire(session_id).await?;
        }
        Ok(changed)
    }

    /// Surfaces delegated questionnaires inside a running TUI wait state.
    pub(super) async fn poll_running_subagent_questionnaires(
        &mut self,
        session_id: &rho_sdk::SessionId,
    ) -> anyhow::Result<bool> {
        let mut changed = self.drain_subagent_host_input();
        changed |= self.discard_stale_subagent_questionnaires(session_id);
        changed |= self
            .finish_pending_subagent_questionnaire(ParentActivity::Running)
            .await?;
        if self.pending_subagent_questionnaire.is_none()
            && matches!(self.input_ui.composer(), ComposerMode::Input)
            && !self.input_ui.has_pending_draft()
        {
            changed |= self.present_next_subagent_questionnaire(session_id).await?;
        }
        Ok(changed)
    }

    fn drain_subagent_host_input(&mut self) -> bool {
        let Some(receiver) = self.subagent_host_input.as_mut() else {
            return false;
        };
        let mut changed = false;
        while let Ok(request) = receiver.try_recv() {
            self.queued_subagent_questionnaires.push_back(request);
            changed = true;
        }
        changed
    }

    fn discard_stale_subagent_questionnaires(&mut self, session_id: &rho_sdk::SessionId) -> bool {
        let mut changed = false;
        let queued = std::mem::take(&mut self.queued_subagent_questionnaires);
        for pending in queued {
            if pending.response.is_closed() {
                changed = true;
                continue;
            }
            if &pending.parent_session_id != session_id {
                let _ = pending.response.send(Err(rho_sdk::Error::Interrupted {
                    message: "parent session changed before the delegated questionnaire was shown"
                        .into(),
                }));
                changed = true;
                continue;
            }
            self.queued_subagent_questionnaires.push_back(pending);
        }
        changed
    }

    async fn finish_pending_subagent_questionnaire(
        &mut self,
        parent_activity: ParentActivity,
    ) -> anyhow::Result<bool> {
        let Some(pending) = self.pending_subagent_questionnaire.as_mut() else {
            return Ok(false);
        };
        if pending.response_tx.is_closed() {
            let pending = self
                .pending_subagent_questionnaire
                .take()
                .expect("pending questionnaire checked above");
            let composer = self.input_ui.take_composer();
            if matches!(composer, ComposerMode::Questionnaire(_)) {
                drop(composer);
                self.clear_submitted_input();
            } else {
                self.input_ui.set_composer(composer);
            }
            self.insert_entry(&Entry::Notice(format!(
                "questionnaire for agent {} ({}) is no longer active",
                pending.run_id, pending.agent_id
            )));
            self.restore_parent_activity_after_questionnaire(parent_activity)
                .await;
            return Ok(true);
        }
        let Some(reply) = (&mut pending.reply_rx).now_or_never() else {
            return Ok(false);
        };
        let pending = self
            .pending_subagent_questionnaire
            .take()
            .expect("pending questionnaire checked above");
        match reply {
            Ok(QuestionnaireReply::Answer(response)) => {
                let _ = pending
                    .response_tx
                    .send(Ok(event_adapter::host_response(response)));
                self.insert_entry(&Entry::Notice(format!(
                    "answered questionnaire for agent {} ({})",
                    pending.run_id, pending.agent_id
                )));
            }
            Ok(QuestionnaireReply::Cancelled(reason)) => {
                let message = match reason {
                    super::QuestionnaireCancelReason::UserCancelled => {
                        "delegated questionnaire cancelled by user"
                    }
                    super::QuestionnaireCancelReason::UiUnavailable => {
                        "delegated questionnaire cancelled because the UI closed"
                    }
                };
                let _ = pending.response_tx.send(Err(rho_sdk::Error::Interrupted {
                    message: message.into(),
                }));
                self.insert_entry(&Entry::Notice(format!(
                    "cancelled questionnaire for agent {} ({})",
                    pending.run_id, pending.agent_id
                )));
            }
            Err(_) => {
                let _ = pending.response_tx.send(Err(rho_sdk::Error::Interrupted {
                    message: "delegated questionnaire reply channel closed".into(),
                }));
            }
        }
        self.restore_parent_activity_after_questionnaire(parent_activity)
            .await;
        Ok(true)
    }

    async fn restore_parent_activity_after_questionnaire(
        &mut self,
        parent_activity: ParentActivity,
    ) {
        match parent_activity {
            ParentActivity::Idle => {
                self.status = "ready".into();
                self.report_resting_herdr_state().await;
            }
            ParentActivity::Running => {
                self.status = "running".into();
                self.report_herdr_working().await;
            }
        }
    }

    async fn present_next_subagent_questionnaire(
        &mut self,
        session_id: &rho_sdk::SessionId,
    ) -> anyhow::Result<bool> {
        let mut changed = false;
        let pending = loop {
            let Some(pending) = self.queued_subagent_questionnaires.pop_front() else {
                return Ok(changed);
            };
            if pending.response.is_closed() {
                changed = true;
                continue;
            }
            if &pending.parent_session_id != session_id {
                let _ = pending.response.send(Err(rho_sdk::Error::Interrupted {
                    message: "parent session changed before the delegated questionnaire was shown"
                        .into(),
                }));
                changed = true;
                continue;
            }
            break pending;
        };
        let (reply_tx, reply_rx) = oneshot::channel();
        let title = pending.request.title().to_string();
        self.open_questionnaire(QuestionAnswerRequest {
            request: event_adapter::questionnaire_request(&pending.request),
            response: QuestionnaireResponseChannel::new(reply_tx),
            notice: Some(format!(
                "agent {} ({}) asks: {title}",
                pending.run_id, pending.agent_id
            )),
        })
        .await?;
        self.pending_subagent_questionnaire = Some(PendingSubagentQuestionnaire {
            run_id: pending.run_id,
            agent_id: pending.agent_id,
            reply_rx,
            response_tx: pending.response,
        });
        Ok(true)
    }

    fn can_present_subagent_questionnaire(&self) -> bool {
        self.pending_subagent_questionnaire.is_none()
            && matches!(self.input_ui.composer(), ComposerMode::Input)
            && !self.input_ui.has_pending_draft()
            && self.allows_idle_subagent_delivery()
            && self.goal.is_none()
            && self.pending.queued_prompts().is_empty()
            && self.pending.steering_prompts().is_empty()
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
        self.allows_idle_subagent_delivery()
            && self.goal.is_none()
            && self.pending.queued_prompts().is_empty()
            && self.pending_subagent_questionnaire.is_none()
            && !matches!(self.input_ui.composer(), ComposerMode::Questionnaire(_))
            && self.queued_subagent_questionnaires.is_empty()
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

#[cfg(test)]
#[path = "background_polls_tests.rs"]
mod tests;
