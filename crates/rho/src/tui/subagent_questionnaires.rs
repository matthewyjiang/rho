//! Delegated-agent questionnaire and completion coordination.

use futures_util::FutureExt;
use ratatui::DefaultTerminal;
use tokio::sync::oneshot;

use super::{
    event_adapter, questionnaire::QuestionnaireResponseChannel, turn_prompt::TurnPrompt, App,
    ComposerMode, Entry, InteractiveRuntime, PendingSubagentQuestionnaire, QuestionAnswerRequest,
    QuestionnaireReply, TurnOutcome,
};

#[derive(Clone, Copy)]
enum ParentActivity {
    Idle,
    Working(&'static str),
}

impl App {
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

    /// Updates delegated questionnaire state without presenting another request.
    /// The active turn uses this while its shared interaction queue owns ordering.
    pub(super) async fn poll_running_subagent_questionnaire_state(
        &mut self,
        session_id: &rho_sdk::SessionId,
    ) -> anyhow::Result<bool> {
        let mut changed = self.drain_subagent_host_input();
        changed |= self.discard_stale_subagent_questionnaires(session_id);
        changed |= self
            .finish_pending_subagent_questionnaire(ParentActivity::Working("running"))
            .await?;
        Ok(changed)
    }

    /// Surfaces delegated questionnaires while a goal waits for its children.
    pub(super) async fn poll_waiting_subagent_questionnaires(
        &mut self,
        session_id: &rho_sdk::SessionId,
    ) -> anyhow::Result<bool> {
        let mut changed = self.drain_subagent_host_input();
        changed |= self.discard_stale_subagent_questionnaires(session_id);
        changed |= self
            .finish_pending_subagent_questionnaire(ParentActivity::Working(
                "waiting for delegated agents",
            ))
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
                self.set_status("ready");
                self.report_resting_herdr_state().await;
            }
            ParentActivity::Working(status) => {
                self.set_status(status);
                self.report_herdr_working().await;
            }
        }
    }

    pub(super) async fn present_subagent_questionnaire(
        &mut self,
        pending: crate::app::subagent_host_input::SubagentHostInputRequest,
    ) -> anyhow::Result<bool> {
        if pending.response.is_closed() {
            return Ok(false);
        }
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
        changed |= self.present_subagent_questionnaire(pending).await?;
        Ok(changed)
    }

    fn can_present_subagent_questionnaire(&self) -> bool {
        self.pending_subagent_questionnaire.is_none()
            && matches!(self.input_ui.composer(), ComposerMode::Input)
            && !self.input_ui.has_pending_draft()
            && self.allows_idle_subagent_delivery()
            && self
                .goal
                .as_ref()
                .is_none_or(crate::tui::goal::GoalState::is_blocked)
            && self.pending.queued_prompts().is_empty()
            && self.pending.steering_prompts().is_empty()
    }

    pub(super) async fn run_subagent_completion_turn(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<Option<TurnOutcome>> {
        let mut model_parts = Vec::new();
        let mut display_parts = Vec::new();
        if let Some(manager) = agent.subagents().cloned() {
            let notifications = manager.take_notifications(agent.session_id().as_str());
            if !notifications.is_empty() {
                let (model, display) = crate::tools::agent::notification_prompts(&notifications);
                model_parts.push(model);
                display_parts.push(display);
            }
        }
        let workflow_notifications = agent
            .workflow_tracker()
            .take_notifications(agent.session_id().as_str());
        if !workflow_notifications.is_empty() {
            let (model, display) =
                crate::tools::workflow_tracker::notification_prompts(&workflow_notifications);
            model_parts.push(model);
            display_parts.push(display);
        }
        if model_parts.is_empty() {
            return Ok(None);
        }
        // The whole drained batch is one message and one model request, no
        // matter how many runs finished while the parent was busy.
        let model_prompt = model_parts.join("\n\n");
        let display_prompt = display_parts.join("\n");
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
}

#[cfg(test)]
#[path = "subagent_questionnaires_tests.rs"]
mod tests;
