//! Prepare one turn-boundary delivery without coupling display policy to scheduling.

use crate::{
    app::{interactive_runtime::ComputerNotice, subagent_messaging::NoticeDelivery},
    display_transcript::{DisplayRow, DisplayTranscript},
    presentation::{
        NotificationCard, NotificationDelivery, NotificationPreview, NotificationTone,
        NotificationVisibility,
    },
    subagent::RunState,
};

use super::InteractiveRuntime;

/// Drained work held until an accepted provider start or runtime checkpoint.
#[derive(Default)]
pub(super) struct TurnBoundaryBatch {
    pub(super) runtime_context: Option<ComputerNotice>,
    pub(super) subagent_notifications: Vec<crate::tools::agent::SubagentNotification>,
    pub(super) notices: Vec<crate::app::subagent_messaging::SubagentNotice>,
    pub(super) workflow_notifications: Vec<crate::tools::workflow_tracker::WorkflowNotification>,
    pub(super) process_notifications: Vec<crate::tools::process::ProcessNotification>,
}

impl TurnBoundaryBatch {
    pub(super) fn requires_parent_action(&self) -> bool {
        self.notices
            .iter()
            .any(|notice| notice.delivery.requires_parent_action())
    }

    pub(super) fn is_empty(&self) -> bool {
        self.runtime_context.is_none()
            && self.notices.is_empty()
            && self.subagent_notifications.is_empty()
            && self.workflow_notifications.is_empty()
            && self.process_notifications.is_empty()
    }

    /// Resolve task identity once so saved and live cards describe the same delivery.
    pub(super) fn prepare(self, agent: &InteractiveRuntime) -> TurnBoundaryDelivery {
        let mut model = Vec::new();
        let mut rows = Vec::new();
        if !self.notices.is_empty() {
            let mut notices = crate::app::subagent_messaging::notice_prompt(&self.notices);
            notices.insert_str(0, "Earlier child messages in send order. Any terminal result below supersedes that child's planning and progress; preserve substantive findings.\n\n");
            model.push(notices);
            for notice in &self.notices {
                let event = match notice.delivery {
                    NoticeDelivery::NextTurn => AgentEvent::Update,
                    NoticeDelivery::ParentActionRequired => AgentEvent::ActionRequired,
                };
                rows.push(DisplayRow::Notification(Box::new(agent_card(
                    agent,
                    &notice.run_id,
                    &notice.agent_id,
                    /*title*/ None,
                    event,
                    notice.message.clone(),
                ))));
            }
        }
        if !self.subagent_notifications.is_empty() {
            model.push(crate::tools::agent::notification_prompt(
                &self.subagent_notifications,
            ));
            for notification in &self.subagent_notifications {
                let snapshot = &notification.snapshot;
                let event = match snapshot.status.state {
                    RunState::Starting | RunState::Running => AgentEvent::Update,
                    RunState::Ok => AgentEvent::Completed,
                    RunState::Error => AgentEvent::Failed,
                    RunState::Stopped => AgentEvent::Stopped,
                };
                let body = snapshot
                    .status
                    .error
                    .iter()
                    .chain(snapshot.status.result.iter())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let mut card = agent_card(
                    agent,
                    &snapshot.id,
                    &snapshot.agent_id,
                    snapshot.title.as_deref(),
                    event,
                    body,
                );
                card.details.push(format!(
                    "elapsed: {:.1}s · turns: {}",
                    snapshot.elapsed.as_secs_f64(),
                    snapshot.status.turns
                ));
                if let Some(model) =
                    crate::model_identity::PromptModel::from_run_status(&snapshot.status)
                {
                    card.details.push(format!("model: {}", model.describe()));
                }
                rows.push(DisplayRow::Notification(Box::new(card)));
            }
        }
        if !self.workflow_notifications.is_empty() {
            let (input, display) =
                crate::tools::workflow_tracker::notification_prompts(&self.workflow_notifications);
            model.push(input);
            rows.push(DisplayRow::Notice(display));
        }
        if !self.process_notifications.is_empty() {
            model.push(crate::tools::process::notification_prompt(
                &self.process_notifications,
            ));
            rows.extend(
                self.process_notifications
                    .iter()
                    .map(|notification| DisplayRow::Notification(Box::new(notification.card()))),
            );
        }
        if let Some(notice) = &self.runtime_context {
            model.push(notice.model.clone());
            rows.push(DisplayRow::Notice(notice.display.clone()));
        }
        TurnBoundaryDelivery {
            model: model.join("\n\n"),
            transcript: DisplayTranscript(rows),
            batch: self,
        }
    }

    pub(super) fn notice_count(&self) -> usize {
        self.notices.len()
    }
}

/// Agent-specific policy becomes explicit, generic display data at this boundary.
enum AgentEvent {
    Update,
    ActionRequired,
    Completed,
    Failed,
    Stopped,
}

fn agent_card(
    agent: &InteractiveRuntime,
    run_id: &str,
    sender: &str,
    title: Option<&str>,
    event: AgentEvent,
    body: String,
) -> NotificationCard {
    let task = title
        .filter(|title| !title.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| {
            agent
                .subagents()
                .and_then(|manager| manager.task_identity(run_id))
                .map(|identity| identity.task)
        })
        .unwrap_or_else(|| "Delegated task".into());
    let (label, tone, preview) = match event {
        AgentEvent::Update => (
            "Update",
            NotificationTone::Accent,
            NotificationPreview::Truncated,
        ),
        AgentEvent::ActionRequired => (
            "Action requested",
            NotificationTone::Warning,
            NotificationPreview::Full,
        ),
        AgentEvent::Completed => (
            "Completed",
            NotificationTone::Success,
            NotificationPreview::Truncated,
        ),
        AgentEvent::Failed => ("Failed", NotificationTone::Error, NotificationPreview::Full),
        AgentEvent::Stopped => (
            "Stopped",
            NotificationTone::Accent,
            NotificationPreview::Truncated,
        ),
    };
    NotificationCard {
        title: format!("{label} · {task}"),
        sender: sender.into(),
        recipient: "parent".into(),
        delivery: NotificationDelivery::Received,
        tone,
        preview,
        visibility: NotificationVisibility::Conversation,
        reference: Some(run_id.into()),
        subtitle: None,
        body,
        details: vec![format!("task: {task}")],
    }
}

/// Prepared prompts and display plus the restorable drained work for one boundary.
pub(super) struct TurnBoundaryDelivery {
    pub(super) model: String,
    pub(super) transcript: DisplayTranscript,
    pub(super) batch: TurnBoundaryBatch,
}
