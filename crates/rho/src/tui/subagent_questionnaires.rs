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

pub(super) enum SubagentCompletionTurn {
    NoDelivery,
    PendingConfirmation,
    Completed(TurnOutcome),
}

fn subagent_completion_changed(outcome: &SubagentCompletionTurn) -> bool {
    !matches!(outcome, SubagentCompletionTurn::NoDelivery)
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
        self.subagent_inbox.drain();
        if !self.should_deliver_idle_subagent_completions() {
            return Ok(false);
        }
        // Completions and child notices share one idle delivery path. Opening a
        // confirmation modal is itself a visible state change and needs redraw.
        let outcome = self.run_subagent_completion_turn(terminal, agent).await?;
        Ok(subagent_completion_changed(&outcome))
    }

    /// Surfaces delegated questionnaires when the parent can take user input.
    pub(super) async fn poll_subagent_questionnaires(
        &mut self,
        session_id: &rho_sdk::SessionId,
    ) -> anyhow::Result<bool> {
        let mut changed = self.subagent_inbox.drain();
        changed |= self.subagent_inbox.discard_stale(session_id);
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
        let mut changed = self.subagent_inbox.drain();
        changed |= self.subagent_inbox.discard_stale(session_id);
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
        let mut changed = self.subagent_inbox.drain();
        changed |= self.subagent_inbox.discard_stale(session_id);
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

    /// Collects everything owed to the model at a turn boundary: background
    /// completions, delegated-child notices, workflow notifications, and
    /// finished process-tool jobs.
    ///
    /// Returns the joined model prompt, display summary, and the drained batch
    /// so callers can restore on setup failure, or `None` when nothing is
    /// pending. Real prompt turns fold this into the outgoing message; an idle
    /// parent sends it as a turn of its own. Removal is committed only after
    /// the provider turn starts successfully.
    pub(super) fn collect_turn_boundary_prompts(
        &mut self,
        agent: &mut InteractiveRuntime,
    ) -> Option<TurnBoundaryDelivery> {
        let mut model_parts = Vec::new();
        let mut display_parts = Vec::new();
        let mut push = |(model, display): (String, String)| {
            model_parts.push(model);
            display_parts.push(display);
        };
        let mut batch = TurnBoundaryBatch::default();
        if let Some(manager) = agent.subagents().cloned() {
            batch.subagent_notifications = manager.take_notifications(agent.session_id().as_str());
            if !batch.subagent_notifications.is_empty() {
                push(crate::tools::agent::notification_prompts(
                    &batch.subagent_notifications,
                ));
            }
        }
        self.subagent_inbox.drain();
        batch.notices = self.subagent_inbox.take_notices(agent.session_id());
        if !batch.notices.is_empty() {
            push(crate::app::subagent_messaging::notice_prompts(
                &batch.notices,
            ));
        }
        batch.workflow_notifications = agent
            .workflow_tracker()
            .take_notifications(agent.session_id().as_str());
        if !batch.workflow_notifications.is_empty() {
            push(crate::tools::workflow_tracker::notification_prompts(
                &batch.workflow_notifications,
            ));
        }
        if let Some(processes) = agent.processes() {
            batch.process_notifications = processes.take_notifications();
            if !batch.process_notifications.is_empty() {
                push(crate::tools::process::notification_prompts(
                    &batch.process_notifications,
                ));
            }
        }
        if model_parts.is_empty() {
            return None;
        }
        Some(TurnBoundaryDelivery {
            model: model_parts.join("\n\n"),
            display: display_parts.join("\n"),
            batch,
        })
    }

    /// Puts a drained turn-boundary batch back when provider start never began.
    pub(super) fn restore_turn_boundary_batch(
        &mut self,
        agent: &mut InteractiveRuntime,
        batch: TurnBoundaryBatch,
    ) {
        if let Some(manager) = agent.subagents() {
            manager.restore_notifications(&batch.subagent_notifications);
        }
        self.subagent_inbox.return_notices(batch.notices);
        agent
            .workflow_tracker()
            .restore_notifications(&batch.workflow_notifications);
        if let Some(processes) = agent.processes() {
            processes.restore_notifications(&batch.process_notifications);
        }
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
            request: pending.request,
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
            let Some(pending) = self.subagent_inbox.next_questionnaire() else {
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
    ) -> anyhow::Result<SubagentCompletionTurn> {
        let Some(delivery) = self.collect_turn_boundary_prompts(agent) else {
            return Ok(SubagentCompletionTurn::NoDelivery);
        };
        // The whole drained batch is one message and one model request, no
        // matter how many runs finished while the parent was busy. The send
        // gate owns the drained batch until confirmation; provider start owns
        // restoration after that point.
        let submission = super::send_confirm::SendSubmission::turn_boundary(
            TurnPrompt::standard(delivery.model, delivery.display),
            delivery.batch,
        );
        let Some(submission) = self.gate_send(submission, agent) else {
            return Ok(SubagentCompletionTurn::PendingConfirmation);
        };
        let (payload, authorization, _allow_auto_compact) = submission.into_authorized();
        let super::send_confirm::SendPayload::TurnBoundary { turn, batch } = payload else {
            unreachable!("subagent delivery is a turn-boundary submission");
        };
        self.run_turn_boundary_prompt_turn(turn, batch, authorization, terminal, agent)
            .await
            .map(SubagentCompletionTurn::Completed)
    }

    pub(super) fn should_deliver_idle_subagent_completions(&self) -> bool {
        self.allows_idle_subagent_delivery()
            && self.goal.is_none()
            && self.pending.queued_prompts().is_empty()
            && self.pending_subagent_questionnaire.is_none()
            && matches!(self.input_ui.composer(), ComposerMode::Input)
            && !self.subagent_inbox.has_queued_questionnaires()
    }
}

#[cfg(test)]
#[path = "subagent_questionnaires_tests.rs"]
mod tests;

/// Drained turn-boundary work held until provider start commits delivery.
#[derive(Default)]
pub(super) struct TurnBoundaryBatch {
    subagent_notifications: Vec<crate::tools::agent::SubagentNotification>,
    notices: Vec<crate::app::subagent_messaging::SubagentNotice>,
    workflow_notifications: Vec<crate::tools::workflow_tracker::WorkflowNotification>,
    process_notifications: Vec<crate::tools::process::ProcessNotification>,
}

impl TurnBoundaryBatch {
    pub(super) fn notice_count(&self) -> usize {
        self.notices.len()
    }
}

/// Joined prompts plus the restorable drained batch for one turn boundary.
pub(super) struct TurnBoundaryDelivery {
    pub(super) model: String,
    pub(super) display: String,
    pub(super) batch: TurnBoundaryBatch,
}
