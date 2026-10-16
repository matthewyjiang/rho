//! Background results delivered at one turn boundary, rendered once for both
//! the model and the transcript.
//!
//! The TUI and the headless hosts (ACP, `rho run`) deliver the same finished
//! work: subagent messages and results, workflows, and background processes.
//! Rendering both views here keeps the saved display rows describing exactly
//! the input the model received, whichever host delivered it.

use crate::{
    app::subagent_messaging::{NoticeDelivery, SubagentNotice},
    display_transcript::{DisplayRow, DisplayTranscript},
    presentation::{
        NotificationCard, NotificationDelivery, NotificationPreview, NotificationTone,
        NotificationVisibility,
    },
    subagent::RunState,
    tools::{
        agent::{SubagentManager, SubagentNotification},
        process::ProcessNotification,
        workflow_tracker::WorkflowNotification,
    },
};

/// Finished background work drained for one boundary.
pub(crate) struct BoundaryResults<'a> {
    pub(crate) notices: &'a [SubagentNotice],
    pub(crate) subagents: &'a [SubagentNotification],
    pub(crate) workflows: &'a [WorkflowNotification],
    pub(crate) processes: &'a [ProcessNotification],
}

/// Model input sections and display rows for one boundary, in delivery order.
/// Hosts may append their own sections before joining them.
pub(crate) struct BoundaryMessage {
    pub(crate) model: Vec<String>,
    pub(crate) rows: Vec<DisplayRow>,
}

impl BoundaryMessage {
    /// `None` when nothing was delivered.
    pub(crate) fn finish(self) -> Option<(String, DisplayTranscript)> {
        (!self.model.is_empty()).then(|| (self.model.join("\n\n"), DisplayTranscript(self.rows)))
    }
}

impl BoundaryResults<'_> {
    /// Renders every result. `manager` resolves subagent task titles so saved
    /// and live cards name the same task.
    pub(crate) fn render(&self, manager: Option<&SubagentManager>) -> BoundaryMessage {
        let mut model = Vec::new();
        let mut rows = Vec::new();
        if !self.notices.is_empty() {
            model.push(format!(
                "Earlier child messages in send order. Any terminal result below supersedes that child's planning and progress; preserve substantive findings.\n\n{}",
                crate::app::subagent_messaging::notice_prompt(self.notices),
            ));
            for notice in self.notices {
                let event = match notice.delivery {
                    NoticeDelivery::NextTurn => AgentEvent::Update,
                    NoticeDelivery::ParentActionRequired => AgentEvent::ActionRequired,
                };
                rows.push(DisplayRow::Notification(Box::new(agent_card(
                    manager,
                    &notice.run_id,
                    &notice.agent_id,
                    /*title*/ None,
                    event,
                    notice.message.clone(),
                ))));
            }
        }
        if !self.subagents.is_empty() {
            model.push(crate::tools::agent::notification_prompt(self.subagents));
            for notification in self.subagents {
                rows.push(DisplayRow::Notification(Box::new(subagent_result_card(
                    manager,
                    notification,
                ))));
            }
        }
        if !self.workflows.is_empty() {
            let (input, display) =
                crate::tools::workflow_tracker::notification_prompts(self.workflows);
            model.push(input);
            rows.push(DisplayRow::Notice(display));
        }
        if !self.processes.is_empty() {
            model.push(crate::tools::process::notification_prompt(self.processes));
            rows.extend(
                self.processes
                    .iter()
                    .map(|notification| DisplayRow::Notification(Box::new(notification.card()))),
            );
        }
        BoundaryMessage { model, rows }
    }
}

fn subagent_result_card(
    manager: Option<&SubagentManager>,
    notification: &SubagentNotification,
) -> NotificationCard {
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
        manager,
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
    if let Some(model) = crate::model_identity::PromptModel::from_run_status(&snapshot.status) {
        card.details.push(format!("model: {}", model.describe()));
    }
    card
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
    manager: Option<&SubagentManager>,
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
            manager
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
        output: None,
        details: vec![format!("task: {task}")],
    }
}
